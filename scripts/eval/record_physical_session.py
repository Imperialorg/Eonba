"""Acquire an operator-authorized stereo TTFA session on a dedicated Linux measurement rig."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import wave

import physical_session as session
from stereo_pcm import endpoint


def record(path, take, rate, seconds):
    with open(path, "xb", opener=lambda p, flags: os.open(p, flags, 0o600)) as output:
        with wave.open(output, "wb") as wav:
            wav.setparams((2, 2, rate, 0, "NONE", "not compressed"))
            take(seconds, wav.writeframesraw)
    digest = hashlib.sha256()
    frames = rate * seconds
    with wave.open(str(path), "rb") as wav:
        layout = (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getnframes(), wav.getcomptype())
        if layout != (2, 2, rate, frames, "NONE"):
            raise ValueError("recorder did not deliver the requested stereo PCM clock and frame count")
        while frames:
            count = min(frames, rate)
            data = wav.readframes(count)
            if len(data) != count * 4:
                raise ValueError("truncated recording")
            digest.update(data)
            frames -= count
    return digest.hexdigest()


def acquire(args):
    if sys.platform != "linux" or sys.byteorder != "little":
        raise ValueError("supported acquisition hosts are little-endian Linux")
    if not sys.stdin.isatty():
        raise ValueError("operator terminal required; no unattended recording")
    if not re.fullmatch(r"hw:[0-9]{1,3},[0-9]{1,3}", args.device):
        raise ValueError("select one dedicated hardware capture PCM as hw:CARD_NUMBER,DEVICE_NUMBER")
    if not args.route.strip() or not args.gains.strip() or not 1 <= args.threshold <= 32767:
        raise ValueError("provide the common route, fixed gains and calibrated threshold")
    if not args.directory.is_absolute():
        raise ValueError("use a new absolute session directory under your private directory")
    parent = args.directory.parent.stat()
    if parent.st_uid != os.geteuid() or parent.st_mode & 0o022:
        raise ValueError("session parent must be owned by you and not group/other writable")

    labels = ("control", *session.SCENES) if args.noisy else ("control",)
    turns = [scene for scene in labels for _ in range(session.acoustic.MIN_TRIALS // len(labels))]
    print(f"{args.device}: input reference on LEFT, isolated acoustic speaker on RIGHT, same clock.\n"
          f"This records a muted control and {len(turns)} turns ({', '.join(labels)}) to {args.directory}.\n"
          "Use only a dedicated authorized rig, not a host microphone, digital loopback or generated WAV.\n"
          "Keep at least 50 ms of quiet before each stimulus. Ctrl-C stops the session; no automatic retakes.")
    if input("Type RECORD to confirm physical routing and permission: ") != "RECORD":
        raise ValueError("recording not authorized")

    parent_fd = os.open(args.directory.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        actual = os.fstat(parent_fd)
        if not os.path.samestat(actual, parent) or actual.st_uid != os.geteuid() or actual.st_mode & 0o022:
            raise ValueError("session parent changed")
        os.mkdir(args.directory.name, 0o700, dir_fd=parent_fd)
        fd = os.open(args.directory.name, os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent_fd)
        try:
            with endpoint(args.device, args.rate) as take:
                collect(args, Path(f"/proc/self/fd/{fd}"), turns, take)
        finally:
            os.close(fd)
    finally:
        os.close(parent_fd)


def collect(args, directory, turns, take):
    # Use the pinned private directory, not a path that can be redirected between takes.
    trials = directory / "trials"
    trials.mkdir(mode=0o700)
    control = directory / "muted-control.wav"
    if input("Mute the speaker, prepare the included stimuli and type MUTED to record the 30 s control: ") != "MUTED":
        raise ValueError("control cancelled")
    control_hash = record(control, take, args.rate, 30)
    session.check_control(session.checked_capture(control, args.threshold, control_hash))
    info = {"origin": "authorized-single-clock-physical", "route": args.route, "gains": args.gains,
            "threshold_pcm": args.threshold, "control_pcm_sha256": control_hash, "trials": {}}
    print("Unmute the speaker. Keep the same clock, channel routing, gains and threshold.")
    seen = {control_hash}
    for i, scene in enumerate(turns, 1):
        token = f"TAKE {i}"
        if input(f"Turn {i}/{len(turns)}, {scene}. Type {token} to record {args.seconds} s: ") != token:
            raise ValueError("turn cancelled")
        name = f"turn-{i:03d}.wav"
        digest = record(trials / name, take, args.rate, args.seconds)
        # Validate the recorded stimulus, never condition inclusion on the latency or a reply.
        session.checked_capture(trials / name, args.threshold, digest)
        if digest in seen:
            raise ValueError("duplicate PCM recording")
        seen.add(digest)
        info["trials"][name] = {"scene": scene, "pcm_sha256": digest}
    # Do not select trials by outcome. Even a turn with no speaker response is in the inventory.
    pending = directory / "session.json.partial"
    with pending.open("x") as f:
        json.dump(info, f, indent=2)
        f.write("\n")
    pending.rename(directory / "session.json")


def main(argv=None):
    cli = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    cli.add_argument("directory", type=Path, help="new private directory; original WAVs never leave this host")
    cli.add_argument("--device", required=True, help="dedicated stereo ALSA hw:CARD_NUMBER,DEVICE_NUMBER")
    cli.add_argument("--route", required=True, help="common capture clock and acoustic routing")
    cli.add_argument("--gains", required=True, help="fixed gains used for both channels")
    cli.add_argument("--threshold", required=True, type=int, help="calibrated RMS threshold in PCM units")
    cli.add_argument("--rate", type=int, choices=(16000, 44100, 48000, 96000), default=48000)
    cli.add_argument("--seconds", type=int, choices=range(2, 31), default=5, metavar="2..30")
    cli.add_argument("--noisy", action="store_true", help="25 turns each of control, babble, traffic and TV")
    args = cli.parse_args(argv)
    try:
        acquire(args)
    except (EOFError, KeyboardInterrupt, OSError, ValueError, wave.Error) as error:
        print(f"Acquisition stopped ({type(error).__name__}): {error}", file=sys.stderr)
        return 2
    print("Acquisition complete, not a latency verdict. Supply this directory to the full evaluator.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
