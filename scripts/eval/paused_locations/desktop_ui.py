"""Click real workbench controls and check their visible text in a private Wayland display."""
import csv
import io
import itertools
import os
from pathlib import Path
import re
import subprocess as sp
import time


def run(args, **kw):
    return sp.run(args, check=True, stdout=sp.PIPE, stderr=sp.DEVNULL, timeout=8, **kw).stdout


def screen():
    png = run(["grim", "-"])
    data = run(["tesseract", "stdin", "stdout", "--psm", "11", "tsv"], input=png).decode()
    words = [r for r in csv.DictReader(io.StringIO(data), delimiter="\t") if r["level"] == "5" and r["text"]]
    lines = []
    for _, group in itertools.groupby(words, lambda r: (r["block_num"], r["par_num"], r["line_num"])):
        row = list(group)
        lines.append((" ".join(w["text"] for w in row), int(row[0]["left"]),
                      int(row[0]["top"]) + int(row[0]["height"]) // 2))
    return lines


def find(pattern, stage, *, last=False):
    end = time.monotonic() + 30
    while time.monotonic() < end:
        rows = screen()
        found = [r for r in rows if re.search(pattern, r[0], re.I)]
        if found:
            return (found[-1] if last else found[0]), rows
        time.sleep(0.3)
    print("Fixed desktop diagnostic:", [r[0] for r in rows][:60], flush=True)
    raise RuntimeError(stage)


def click(row):
    _, x, y = row
    assert 0 < x < 1440 and 0 < y < 1000
    run(["swaymsg", "seat", "seat0", "cursor", "set", str(x + 5), str(y)])
    run(["swaymsg", "seat", "seat0", "cursor", "press", "button1"])
    run(["swaymsg", "seat", "seat0", "cursor", "release", "button1"])
    time.sleep(0.3)


def verify():
    os.chdir("/home/ui")
    os.umask(0o077)
    assert os.geteuid() == 1001 and not Path("/home/runner").exists()
    assert re.search(r"^CapEff:\s*0+$", Path("/proc/self/status").read_text(), re.M)
    assert len(Path("/proc/net/dev").read_text().splitlines()) == 3
    with sp.Popen(["sway", "-c", "/home/ui/sway.conf"], stderr=sp.DEVNULL, stdout=sp.DEVNULL) as sway:
        try:
            for _ in range(60):
                xs = [p for p in Path("runtime").glob("wayland-*") if p.is_socket()]
                ipc = list(Path("runtime").glob("sway-ipc.*.sock"))
                if xs and ipc:
                    break
                assert sway.poll() is None, "no private compositor"
                time.sleep(0.1)
            os.environ["WAYLAND_DISPLAY"] = xs[0].name
            os.environ["SWAYSOCK"] = str(ipc[0].resolve())
            with sp.Popen([
                "/opt/code/code", "--ozone-platform=wayland", "--enable-features=UseOzonePlatform",
                "--no-sandbox", "--disable-gpu", "--disable-crash-reporter", "--skip-welcome",
                "--skip-release-notes", "--disable-updates", "--disable-telemetry", "--locale=en",
                "--user-data-dir=/home/ui/profile", "--extensions-dir=/home/ui/extensions",
                "/home/ui/project",
            ], stdout=sp.DEVNULL, stderr=sp.DEVNULL) as code:
                try:
                    for stage in ["both", "caller"]:
                        cmd, _ = find(r"EON:\s*Show Paused Project Locations", "no palette on real pause", last=True)
                        click(cmd)
                        find(r"Show locations once for my local debug session", "file picker not visible")
                        if stage == "both":
                            click(find(r"src/endpoint\.rs", "no callee choice")[0])
                        click(find(r"src/pre.cognition\.rs", "no caller choice")[0])
                        click(find(r"^OK$", "no picker confirmation")[0])
                        _, rows = find(r"src/pre.cognition\.rs:\s*22", "no caller in result dialog")
                        modal = "\n".join(r[0] for r in rows)
                        assert re.search("Paused project locations", modal, re.I)
                        assert bool(re.search(r"src/endpoint\.rs:\s*28", modal)) == (stage == "both")
                        click(find(r"^OK$", "no modal dismissal")[0])
                        Path(f"runtime/{stage}.seen").touch()
                    for _ in range(60):
                        if Path("runtime/done").is_file():
                            break
                        time.sleep(0.2)
                    assert Path("runtime/done").is_file(), "debug session did not finish"
                    print("VS Code: real DebugStackFrame; both: src/endpoint.rs:28, src/pre_cognition.rs:22;"
                          " caller only: src/pre_cognition.rs:22.")
                finally:
                    if not Path("runtime/caller.seen").exists():
                        phase = Path("runtime/phase")
                        print("Test phase:", phase.read_text() if phase.exists() else "before extension host")
                        for log in Path("profile/logs").glob("**/*.log"):
                            errs = [s for s in log.read_text(errors="replace").splitlines() if "[error]" in s]
                            if errs:
                                print(log.name, errs[-2:])
                    if code.poll() is None:
                        code.terminate()
        finally:
            sway.terminate()


if __name__ == "__main__":
    verify()
