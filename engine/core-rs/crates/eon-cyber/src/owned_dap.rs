//! Opt-in, Linux x86-64 LLVM 18 stdio probe for one compiled-in counting fixture.
//! This is not a debugger service. There is no target, PID, path, source text,
//! command, expression, socket address, or raw DAP argument in the public API.
//! The trusted system compiler and adapter must be installed locally. The
//! workspace (including `target/`) must not be writable by other principals.
//! Child tracing, seccomp and `/proc/thread-self/children` are required. The
//! protocol is bounded to 15 seconds, 48 incoming messages and 64 KiB/message.
//! Host filesystem latency and the kernel reaping a killed task are not timed.
//! Same-user hostile code and compromised system tools are outside this model.

mod wire;

use serde_json::{Value, json};
use std::fs::{self, DirBuilder, File};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use wire::Wire;

const COUNT: &str = include_str!("../fixtures/owned_dap/count.c");
const LAUNCH: &str = include_str!("../fixtures/owned_dap/launch.c");
const CLANG: &str = "/usr/lib/llvm-18/bin/clang";
const ADAPTER: &str = "/usr/lib/llvm-18/bin/lldb-dap";
const SERVER: &str = "/usr/lib/llvm-18/bin/lldb-server";
static RUN: Mutex<()> = Mutex::new(());

unsafe extern "C" {
    fn geteuid() -> u32;
    fn kill(pid: i32, sig: i32) -> i32;
}

/// Two distinct callers of the same instruction. Neither accepts expression text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visit {
    Second,
    Third,
}

impl Visit {
    fn condition(self) -> &'static str {
        match self {
            Self::Second => "visit == 2",
            Self::Third => "visit == 3",
        }
    }

    fn locations(self) -> [(&'static str, &'static str); 3] {
        let (caller, call, from_main) = match self {
            Self::Second => ("second", "return count(2);", "int b = second();"),
            Self::Third => ("third", "return count(3);", "int c = third();"),
        };
        [
            ("count", "return visit + 1;"),
            (caller, call),
            ("main", from_main),
        ]
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct SourceFrame {
    /// One of the fixture's allowlisted function names, never adapter text.
    pub function: &'static str,
    /// One-based physical line in the in-tree `count.c`.
    pub line: u32,
}

/// No adapter error strings, process identifiers, debuggee output or host paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Busy,
    Unavailable,
    Build,
    Transport,
    Protocol,
    UnexpectedFixture,
    Deadline,
}
type Result<T> = std::result::Result<T, Error>;

fn line(statement: &str) -> Result<u32> {
    COUNT
        .lines()
        .position(|s| s.trim() == statement)
        .map(|n| n as u32 + 1)
        .ok_or(Error::UnexpectedFixture)
}

struct Workspace(PathBuf);

impl Workspace {
    fn create() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .ok_or(Error::Unavailable)?
            .join("target");
        // SAFETY: geteuid has no arguments or preconditions.
        let uid = unsafe { geteuid() };
        for path in root.ancestors() {
            let m = path.symlink_metadata().map_err(|_| Error::Unavailable)?;
            if !m.is_dir() || m.mode() & 0o022 != 0 || (m.uid() != 0 && m.uid() != uid) {
                return Err(Error::Unavailable);
            }
        }
        let mut nonce = [0; 16];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut nonce))
            .map_err(|_| Error::Unavailable)?;
        let dir = root.join(format!("owned-dap-{:032x}", u128::from_ne_bytes(nonce)));
        DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|_| Error::Unavailable)?;
        Ok(Self(dir))
    }

    fn command(&self, exe: &Path) -> Command {
        let mut cmd = Command::new(exe);
        cmd.env_clear()
            .env("HOME", &self.0)
            .env("TMPDIR", &self.0)
            .env("XDG_CACHE_HOME", &self.0)
            .env("PATH", "/usr/bin")
            .env("PYTHONNOUSERSITE", "1")
            .env("LLDB_DEBUGSERVER_PATH", SERVER)
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        cmd
    }

    fn start(&self, end: Instant) -> Result<(ChildGuard, Wire)> {
        self.build(end)?;
        // Both stdio descriptors use an anonymous pair, not a filesystem
        // socket, port, listener or externally connectable endpoint.
        let (client, adapter) = UnixStream::pair().map_err(|_| Error::Transport)?;
        let stdin = adapter.try_clone().map_err(|_| Error::Transport)?;
        let child = self
            .command(&self.0.join("launch"))
            .stdin(Stdio::from(OwnedFd::from(stdin)))
            .stdout(Stdio::from(OwnedFd::from(adapter)))
            .spawn()
            .map_err(|_| Error::Unavailable)?;
        Ok((
            ChildGuard {
                child: Some(child),
                supervisor: true,
            },
            Wire::new(client, end),
        ))
    }

    fn build(&self, end: Instant) -> Result<()> {
        for path in [CLANG, ADAPTER, SERVER, "/usr/bin/ld"] {
            if !Path::new(path).is_file() {
                return Err(Error::Unavailable);
            }
        }
        for (src, body, bin) in [("count.c", COUNT, "count"), ("launch.c", LAUNCH, "launch")] {
            let src = self.0.join(src);
            fs::write(&src, body).map_err(|_| Error::Build)?;
            let child = self
                .command(Path::new(CLANG))
                .args([
                    "--no-default-config",
                    "--ld-path=/usr/bin/ld",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-g",
                    "-O0",
                    "-o",
                ])
                .arg(self.0.join(bin))
                .arg(src)
                .spawn()
                .map_err(|_| Error::Build)?;
            ChildGuard {
                child: Some(child),
                supervisor: false,
            }
            .finish(end, Error::Build)?;
        }
        Ok(())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard {
    child: Option<Child>,
    supervisor: bool,
}

impl ChildGuard {
    fn stop(&self) {
        if let Some(child) = &self.child {
            let id = child.id() as i32;
            // SAFETY: this is our unreaped direct child. The compiler has a
            // private group; the supervisor handles TERM and reaps even
            // helpers that form their own process groups.
            unsafe {
                if self.supervisor {
                    kill(id, 15)
                } else {
                    kill(-id, 9)
                }
            };
        }
    }

    fn finish(mut self, end: Instant, failed: Error) -> Result<()> {
        let child = self.child.as_mut().ok_or(failed)?;
        loop {
            if let Some(status) = child.try_wait().map_err(|_| failed)? {
                self.child.take();
                return if status.success() {
                    Ok(())
                } else {
                    Err(failed)
                };
            }
            if Instant::now() >= end {
                return Err(Error::Deadline);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }
}

fn source_stack(frames: &Value, src: &str, visit: Visit) -> Result<[SourceFrame; 3]> {
    let rows = frames
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or(Error::UnexpectedFixture)?;
    let mut result = Vec::with_capacity(3);
    for (row, (function, statement)) in rows.iter().zip(visit.locations()) {
        let n = line(statement)?;
        if row["name"] != function || row["source"]["path"] != src || row["line"] != n {
            return Err(Error::UnexpectedFixture);
        }
        result.push(SourceFrame { function, line: n });
    }
    result.try_into().map_err(|_| Error::UnexpectedFixture)
}

fn paused_stack(wire: &mut Wire, work: &Path, visit: Visit) -> Result<(u64, [SourceFrame; 3])> {
    let src = work.join("count.c");
    let src = src.to_str().ok_or(Error::Unavailable)?;
    let caps = wire.call(
        "initialize",
        json!({
            "adapterID":"lldb", "pathFormat":"path", "linesStartAt1":true, "columnsStartAt1":true,
            "supportsRunInTerminalRequest":false, "supportsStartDebuggingRequest":false
        }),
    )?;
    if caps["supportsConditionalBreakpoints"] != true
        || caps["supportsConfigurationDoneRequest"] != true
    {
        return Err(Error::Unavailable);
    }
    wire.call(
        "launch",
        json!({
            "program":work.join("count"), "cwd":work, "disableASLR":false, "stopOnEntry":false
        }),
    )?;
    wire.wait_for(|e| e.initialized)?;
    let stop_line = line(visit.locations()[0].1)?;
    let set = wire.call(
        "setBreakpoints",
        json!({
            "source":{"path":src}, "breakpoints":[{"line":stop_line, "condition":visit.condition()}]
        }),
    )?;
    let bp = set["breakpoints"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or(Error::Protocol)?;
    let id = bp[0]["id"].as_u64().ok_or(Error::Protocol)?;
    if bp[0]["verified"] != true || bp[0]["line"] != stop_line {
        return Err(Error::UnexpectedFixture);
    }
    wire.call("configurationDone", json!({}))?;
    let (thread, hit) = wire
        .wait_for(|e| e.stop.is_some())?
        .stop
        .ok_or(Error::Protocol)?;
    if id != hit {
        return Err(Error::UnexpectedFixture);
    }
    let trace = wire.call(
        "stackTrace",
        json!({"threadId":thread, "startFrame":0, "levels":3}),
    )?;
    Ok((thread, source_stack(&trace["stackFrames"], src, visit)?))
}

/// Run the complete protocol on an owned child. Success requires a verified
/// conditional breakpoint, the matching three live source frames and exit 0.
/// Only one probe at a time; no automatic installation or less confined retry.
pub fn inspect_counting_fixture(visit: Visit) -> Result<[SourceFrame; 3]> {
    let _run = RUN.try_lock().map_err(|_| Error::Busy)?;
    let end = Instant::now() + Duration::from_secs(15);
    let work = Workspace::create()?;
    let (guard, mut wire) = work.start(end)?;
    let (thread, frames) = paused_stack(&mut wire, &work.0, visit)?;
    wire.call("continue", json!({"threadId":thread}))?;
    wire.wait_for(|e| e.exited)?;
    wire.call("disconnect", json!({"terminateDebuggee":true}))?;
    // Disconnect was acknowledged and the fixture already exited normally.
    // Stop the persistent adapter while stdio is still open, then require
    // the supervisor to confirm there are no descendants left.
    guard.stop();
    guard.finish(end, Error::Transport)?;
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires provisioned LLVM 18 clang, lldb-dap, lldb-server and local child tracing"]
    fn cancellation_reaps_a_real_paused_fixture() {
        let end = Instant::now() + Duration::from_secs(15);
        let work = Workspace::create().unwrap();
        let (guard, mut wire) = work.start(end).unwrap();
        let (thread, _) = paused_stack(&mut wire, &work.0, Visit::Third).unwrap();
        // The fixed fixture is single-threaded. Hold its procfs inode, not a
        // numeric PID to look up after shutdown (PIDs can be reused).
        let status = File::open(format!("/proc/{thread}/status")).unwrap();
        let mut before = String::new();
        (&status).read_to_string(&mut before).unwrap();
        assert!(before.lines().any(|s| s == "Name:\tcount"));
        assert!(before.lines().any(|s| s == "Seccomp:\t2"));
        assert!(before.lines().any(|s| s == "NoNewPrivs:\t1"));
        guard.stop();
        guard.finish(end, Error::Transport).unwrap();
        use std::os::unix::fs::FileExt;
        // A live (including zombie) task would still have readable status.
        assert_eq!(
            status.read_at(&mut [0], 0).unwrap_err().raw_os_error(),
            Some(3)
        ); // ESRCH
    }

    #[test]
    fn only_exact_fixture_source_frames_can_leave_the_client() {
        let src = "/fixed/count.c";
        let rows: Vec<_> = Visit::Second
            .locations()
            .into_iter()
            .map(|(name, statement)| {
                json!({"name":name, "line":line(statement).unwrap(),
                "source":{"path":src},"instructionPointerReference":"private-marker"})
            })
            .collect();
        let frames = json!(rows);
        assert!(
            !format!("{:?}", source_stack(&frames, src, Visit::Second).unwrap())
                .contains("private-marker")
        );
        assert_eq!(
            source_stack(&frames, src, Visit::Third),
            Err(Error::UnexpectedFixture)
        );
        for (field, value) in [
            ("name", json!("foreign")),
            ("source", json!({"path":"/other/count.c"})),
            ("line", json!(1)),
        ] {
            let mut wrong = frames.clone();
            wrong[0][field] = value;
            assert_eq!(
                source_stack(&wrong, src, Visit::Second),
                Err(Error::UnexpectedFixture)
            );
        }
    }
}
