"""Run the Linux VAD acceptance test without discovering or opening a physical audio device."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--private-monitor", required=True, action="store_true")
    parser.parse_args()
    if sys.platform != "linux":
        parser.error("this gate requires Linux ALSA, PulseAudio and CPAL")
    workspace = Path(__file__).resolve().parents[3]
    target = Path(os.environ.get("CARGO_TARGET_DIR", workspace / "target")).resolve()
    root = target / ("pulse-" + uuid.uuid4().hex[:12])
    root.mkdir(mode=0o700, parents=True)
    server = "unix:" + str(root / "native")
    if '"' in server or "\n" in server or len(os.fsencode(root / "native")) >= 108:
        root.rmdir()
        raise ValueError("use a shorter, quote-free CARGO_TARGET_DIR for the private socket")
    audio = dict(os.environ, XDG_CONFIG_HOME=str(root), PULSE_STATE_PATH=str(root),
                 PULSE_RUNTIME_PATH=str(root), XDG_RUNTIME_DIR=str(root), PULSE_COOKIE=str(root / "cookie"),
                 DBUS_SESSION_BUS_ADDRESS=f"unix:path={root / 'no-bus'}",
                 PULSE_SERVER=server, PULSE_SOURCE="eon_vad.monitor", PULSE_SINK="eon_vad",
                 ALSA_CONFIG_PATH=str(root / "asound.conf"), EON_VOICE_TEST_VIRTUAL_MONITOR="1")
    try:
        # No system ALSA include, no fallback PCM, no Pulse default startup file/hardware discovery.
        (root / "asound.conf").write_text(f'pcm.!default {{ type pulse server "{server}" }}\n')
        with (root / "pulse.log").open("w+") as log:
            pulse = subprocess.Popen([
                "pulseaudio", "-n", "--daemonize=no", "--use-pid-file=no", "--exit-idle-time=-1",
                "--disallow-exit", "--disallow-module-loading", "--disable-shm", "--log-target=stderr",
                "--load=module-null-sink sink_name=eon_vad rate=48000 channels=1",
                f"--load=module-native-protocol-unix socket={root / 'native'} auth-anonymous=1",
            ], env=audio, stdout=log, stderr=log)
            try:
                for _ in range(100):
                    if pulse.poll() is not None:
                        log.seek(0)
                        raise RuntimeError("private Pulse failed to start:\n" + log.read())
                    if (root / "native").is_socket():
                        break
                    time.sleep(0.05)
                subprocess.run([
                    "cargo", "test", "--locked", "--release", "-j4", "-p", "eon-voice", "--features", "native", "--test", "vad_e2e",
                    "vad_tests_private_monitor", "--", "--ignored", "--exact", "--nocapture", "--test-threads=1",
                ], env=audio, cwd=workspace, check=True)
            finally:
                pulse.terminate()
                try:
                    pulse.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pulse.kill()
                    pulse.wait()
    finally:
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
