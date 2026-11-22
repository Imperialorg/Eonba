"""Verify and join the two shipping CLI reports from one timed WAN fixture run."""

import argparse
from collections import Counter
from pathlib import Path
import re
from typing import NamedTuple


LIMIT = 1500  # 30-second fixture, 50 nominal send slots/second
PHASES = ("before", "during", "after")
CLASSES = ("proven_under_5ms", "bound_ge_5ms_inconclusive", "decoded_without_receipt", "peer_missing")


class JoinedReport(NamedTuple):
    text: str
    deadline_proven: bool

    def exit_code(self, require_deadline):
        return 3 if require_deadline and not self.deadline_proven else 0


def require(condition, message):
    if not condition:
        raise ValueError(message)


def split_report(text, header):
    lines = text.splitlines()
    require(len(lines) >= 2 and lines[0] == header, f"expected {header}")
    summary = {}
    for token in lines[-1].split():
        key, sep, value = token.partition("=")
        require(sep and key not in summary, "malformed or duplicate summary field")
        summary[key] = value
    return lines[1:-1], summary


def check_summary(role, actual, **expected):
    require(actual.keys() == expected.keys(), f"{role}: missing or unexpected summary field")
    for key, value in expected.items():
        require(actual[key] == str(value), f"{role}: inconsistent {key} (expected {value})")


def sender_bounds(lines):
    bounds = {}
    for line in lines:
        row = re.fullmatch(r"seq=([0-9]+) decode_receipt_rtt_upper_us=([0-9]+|unavailable)", line)
        require(row, "malformed sender sample")
        seq = int(row[1])
        require(1 <= seq <= LIMIT and seq not in bounds, f"invalid or duplicate sender seq={seq}")
        bounds[seq] = None if row[2] == "unavailable" else int(row[2])
    require(bounds and bounds.keys() == set(range(1, len(bounds) + 1)), "missing sender sequence")
    return bounds


def receiver_decodes(lines, queued, media):
    require(len(lines) >= 2, "missing receiver marks")
    marks = []
    for line, name in zip(lines, ("begin", "end")):
        mark = re.fullmatch(rf"mark={name} rx_us=([0-9]+)", line)
        require(mark, f"missing or invalid {name} mark")
        marks.append(int(mark[1]))
    begin, end = marks
    require(begin < end <= 10_000_000, "unordered marks or insufficient time for recovery")

    decodes = {}
    last_at = None
    last_phase = "before"
    max_gap = marked_gap = 0
    for line in lines[2:]:
        row = re.fullmatch(r"seq=([0-9]+) rx_us=([0-9]+) gap_us=([0-9]+|unknown) "
                           r"phase=(before|during|after)(?: via=(primary_copy|standby_copy))?", line)
        require(row, "malformed receiver decode")
        seq, at, gap, phase, via = int(row[1]), int(row[2]), row[3], row[4], row[5]
        require(1 <= seq <= queued and seq not in decodes, f"invalid or duplicate receiver seq={seq}")
        require(media == (via is not None), f"seq={seq}: first-decode lane disagrees with copy mode")
        # The CLI labels the phase BEFORE flooring marks and decodes to us.
        in_phase = {"before": at <= begin, "during": begin <= at <= end, "after": end <= at}
        require(in_phase[phase] and PHASES.index(last_phase) <= PHASES.index(phase),
                f"seq={seq}: phase disagrees with marks or arrival order")
        if last_at is None:
            require(gap == "unknown", "first decode cannot have a gap")
        else:
            # floor(at - last_at) may be one less than floor(at) - floor(last_at).
            require(gap != "unknown" and at - last_at - int(gap) in (0, 1),
                    f"seq={seq}: inconsistent receiver gap")
            max_gap = max(max_gap, int(gap))
            if last_phase != "after" and phase != "before":
                marked_gap = max(marked_gap, int(gap))
        decodes[seq] = (phase, via or "primary")
        last_at, last_phase = at, phase
    return decodes, max_gap, marked_gap


def timing_report(sender, receiver, media):
    tx_rows, tx = split_report(sender, "eon_continuity_timing_v1 clock=sender_monotonic")
    rx_rows, rx = split_report(receiver, "eon_continuity_v1 clock=receiver_monotonic")
    bounds = sender_bounds(tx_rows)
    queued = len(bounds)
    decodes, max_gap, marked_gap = receiver_decodes(rx_rows, queued, media)
    before = [seq for seq, (phase, _) in decodes.items() if phase == "before"]
    ack = {seq for seq, bound in bounds.items() if bound is not None}
    require(ack <= decodes.keys(), "acknowledgment without a receiver decode for that sequence")
    require(ack.intersection(before), "no acknowledged decode before removal")
    highest_before = max(before)
    require(any(seq > highest_before and decodes[seq][0] == "after" for seq in ack),
            "no fresh acknowledged decode after removal")

    unique = len(decodes)
    missing = queued - unique
    max_bound = max(bounds[s] for s in ack)
    if media:
        require(not missing, "redundant run has missing decodes")
        require(("after", "standby_copy") in decodes.values(), "no standby-first decode after removal")

    common = dict(status="complete", limit=LIMIT, queued=queued, skipped=LIMIT - queued)
    # Raw duplicate packets are not additional first-decode rows.
    require(re.fullmatch("[0-9]+", rx.pop("duplicates", "")), "invalid receiver duplicates count")
    check_summary("receiver", rx, **common, bracketed="true", timing="see_sender", unique=unique,
                  missing=missing, max_gap_us=max_gap, max_mark_gap_us=marked_gap)
    check_summary("sender", tx, **common, timing="observed", acknowledged=len(ack),
                  unacknowledged=queued - len(ack), peer_missing=missing,
                  decoded_without_receipt=unique - len(ack), max_decode_receipt_rtt_upper_us=max_bound,
                  invalid_returns=0)

    counts = {phase: Counter() for phase in (*PHASES, "no_decode", "all")}
    out = [f"eon_field_timing_join_v2 threshold_us=5000 redundant_media={int(media)}"]
    for seq in range(1, queued + 1):
        phase, via = decodes.get(seq, ("no_decode", "none"))
        bound = bounds[seq]
        if phase == "no_decode":
            kind = "peer_missing"
        elif bound is None:
            kind = "decoded_without_receipt"
        else:
            kind = "proven_under_5ms" if bound < 5000 else "bound_ge_5ms_inconclusive"
        counts[phase][kind] += 1
        counts["all"][kind] += 1
        out.append(f"seq={seq} phase={phase} via={via} "
                   f"decode_receipt_rtt_upper_us={bound if bound is not None else 'unavailable'} deadline={kind}")
    for phase, tally in counts.items():
        out.append(f"phase={phase} sequences={sum(tally.values())} " +
                   " ".join(f"{kind}={tally[kind]}" for kind in CLASSES))
    proven = counts["all"]["proven_under_5ms"]
    all_queued = "proven" if proven == queued else "not_proven"
    all_slots = proven == LIMIT
    out.append(f"evidence_verification=verified limit={LIMIT} queued={queued} skipped={LIMIT - queued} "
               f"acknowledged={len(ack)} unacknowledged={queued - len(ack)} "
               f"max_decode_receipt_rtt_upper_us={max_bound} invalid_returns=0 all_queued_under_5ms={all_queued}")
    out.append(f"deadline_verdict={'proven' if all_slots else 'not_proven'} scope=all_nominal_slots "
               f"threshold_us=5000 proven_slots={proven} total_slots={LIMIT} unproven_slots={LIMIT - proven} "
               f"skipped={LIMIT - queued} unacknowledged={queued - len(ack)} "
               f"bound_ge_5ms_inconclusive={counts['all']['bound_ge_5ms_inconclusive']}")
    return JoinedReport("\n".join(out), all_slots)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="one fixture evidence directory")
    parser.add_argument("--redundant-media", action="store_true", help="require bound-copy evidence")
    parser.add_argument("--require-voice-deadline", action="store_true",
                        help="exit 3 unless every nominal slot has a bound strictly below 5000 us")
    args = parser.parse_args()
    try:
        report = timing_report((args.directory / "field-sent.log").read_text(encoding="ascii"),
                               (args.directory / "field-receipts.log").read_text(encoding="ascii"),
                               args.redundant_media)
    except (OSError, ValueError) as error:
        print("evidence_verification=invalid deadline_verdict=not_evaluated")
        parser.exit(1, f"invalid field timing evidence: {error}\n")
    print(report.text)
    parser.exit(report.exit_code(args.require_voice_deadline))
