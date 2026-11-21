//! A single opt-in CPython counting fixture, not a general debugger client.

use crate::owned_dap::{Error, SourceFrame, Visit};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/owned_debugpy");
const DRIVER: &str = include_str!("../fixtures/owned_debugpy/probe.py");
static RUN: Mutex<()> = Mutex::new(());

// This fixed script runs only after unshare has successfully created all
// namespaces. It has no caller-supplied command, shell fragment or path.
const INIT: &str = r#"
set -eu
/usr/sbin/ip link set dev lo up
/usr/bin/mount -t tmpfs -o size=48m,nosuid,nodev,noexec,mode=700 tmpfs "$1"
cd "$1"
exec /usr/bin/setpriv --no-new-privs --bounding-set=-all --inh-caps=-all --ambient-caps=-all /usr/bin/python3 -ISB -c "$2"
"#;

unsafe extern "C" {
    fn close_range(first: u32, last: u32, flags: i32) -> i32;
    fn prctl(option: i32, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> i32;
    fn getppid() -> i32;
}

struct Namespace(Child);

impl Drop for Namespace {
    fn drop(&mut self) {
        // unshare --kill-child ties PID 1 to this unreaped direct child.
        // Losing PID 1 kills every descendant, including separate groups.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(visit: Visit) -> Result<Namespace, Error> {
    let parent = std::process::id() as i32;
    let net = std::fs::metadata("/proc/self/ns/net").map_err(|_| Error::Unavailable)?;
    let mut cmd = Command::new("/usr/bin/unshare");
    cmd.args([
        "--user",
        "--map-root-user",
        "--net",
        "--pid",
        "--fork",
        "--kill-child=SIGKILL",
        "--mount-proc",
        "--propagation",
        "private",
        "/bin/sh",
        "-c",
        INIT,
        "owned-debugpy",
        DIR,
        DRIVER,
    ])
    .env_clear()
    .env("HOME", DIR)
    .env("TMPDIR", DIR)
    .env("PATH", "/usr/bin")
    .env("OWNED_PARENT_NET", net.ino().to_string())
    .env(
        "OWNED_VISIT",
        match visit {
            Visit::Second => "2",
            Visit::Third => "3",
        },
    )
    .current_dir("/")
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    // SAFETY: Linux syscalls only, no allocation or locks after fork. Mark
    // inherited descriptors CLOEXEC (including Rust's exec error pipe).
    unsafe {
        cmd.pre_exec(move || {
            if close_range(3, u32::MAX, 4) != 0 || prctl(1, 9, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(1));
            }
            Ok(())
        });
    }
    cmd.spawn().map(Namespace).map_err(|_| Error::Unavailable)
}

/// Returns only the top three verified physical source locations. Requires
/// unprivileged user/net/PID/mount namespaces and the SHA-256-pinned PyPI
/// `debugpy-1.8.22-py2.py3-none-any.whl` in `engine/core-rs/target/`.
/// There is no download, sudo, host-network fallback, or persistent workspace.
pub fn inspect_counting_fixture(visit: Visit) -> Result<[SourceFrame; 3], Error> {
    let _run = RUN.try_lock().map_err(|_| Error::Busy)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut ns = launch(visit)?;
    let status = loop {
        if Instant::now() >= deadline {
            return Err(Error::Deadline);
        }
        if let Some(status) = ns.0.try_wait().map_err(|_| Error::Transport)? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return Err(match status.code() {
            Some(65) => Error::Protocol,
            Some(66) => Error::UnexpectedFixture,
            _ => Error::Unavailable,
        });
    }
    let mut bytes = Vec::new();
    ns.0.stdout
        .take()
        .ok_or(Error::Transport)?
        .take(65)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Transport)?;
    let (caller, caller_line, main_line) = match visit {
        Visit::Second => ("second", 9, 16),
        Visit::Third => ("third", 12, 17),
    };
    let frames = [
        SourceFrame {
            function: "count",
            line: 3,
        },
        SourceFrame {
            function: caller,
            line: caller_line,
        },
        SourceFrame {
            function: "main",
            line: main_line,
        },
    ];
    let expected = format!("count:3\n{caller}:{caller_line}\nmain:{main_line}\n");
    if bytes != expected.as_bytes() {
        return Err(Error::UnexpectedFixture);
    }
    Ok(frames)
}
