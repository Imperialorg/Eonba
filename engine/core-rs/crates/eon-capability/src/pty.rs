//! Host-only, workflow-owned PTYs. The sole program is the system `cat` with
//! no operands: an interactive byte loopback, not a shell or command runner.
//! Use one provider per host and pass the authenticated workflow identity,
//! never an owner supplied by a tool argument. Call `close_owner` on workflow
//! teardown. No agent dispatch, workspace access, or UI integration is implied.
//! The host must leave this provider responsible for waiting on its own children.
mod native;
#[cfg(test)]
mod pty_tests;

use std::collections::{BTreeMap, VecDeque, btree_map::Entry};
use std::io::{self, ErrorKind, Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eon_primitives::{SessionId, WorkflowId};
use native::{Image, Terminal};

pub const MAX_SESSIONS: usize = 8;
pub const MAX_WRITE: usize = 4096;
pub const MAX_READ: usize = 16 * 1024;
pub const MAX_RING_LINES: usize = 10_000;
pub const MAX_RING_BYTES: usize = 256 * 1024;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Opaque, untrusted output. Never send `bytes` directly to a terminal,
/// logger or markup interpreter. `escaped` produces inert ASCII for plain text UIs.
#[derive(Debug)]
pub struct RingPage {
    /// Absolute byte cursor; greater than the requested cursor after eviction.
    pub start: u64,
    pub next: u64,
    pub bytes: Vec<u8>,
}

impl RingPage {
    pub fn escaped(&self) -> String {
        self.bytes.escape_ascii().to_string()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PtySignal {
    Interrupt,
    Terminate,
    Resize { rows: u16, cols: u16 },
}

#[derive(Default)]
struct Ring {
    bytes: VecDeque<u8>,
    start: u64,
    newlines: usize,
}

impl Ring {
    fn push(&mut self, data: &[u8]) {
        for &byte in data {
            if self.bytes.len() == MAX_RING_BYTES {
                self.evict_byte();
            }
            self.bytes.push_back(byte);
            self.newlines += usize::from(byte == b'\n');
            // Count an unterminated last line as a line. Bound even a single
            // line of arbitrary length, retaining its most recent suffix.
            while self.newlines + usize::from(byte != b'\n') > MAX_RING_LINES {
                self.evict_byte();
            }
        }
    }

    fn evict_byte(&mut self) {
        self.newlines -= usize::from(self.bytes.pop_front() == Some(b'\n'));
        self.start += 1;
    }

    fn page(&self, cursor: u64, limit: usize) -> io::Result<RingPage> {
        let start = cursor.max(self.start);
        let offset = start - self.start;
        if !(1..=MAX_READ).contains(&limit) || offset > self.bytes.len() as u64 {
            return Err(io::Error::new(ErrorKind::InvalidInput, "invalid PTY page"));
        }
        let bytes: Vec<_> = self
            .bytes
            .range(offset as usize..)
            .take(limit)
            .copied()
            .collect();
        Ok(RingPage {
            start,
            next: start + bytes.len() as u64,
            bytes,
        })
    }
}

struct Session {
    owner: WorkflowId,
    terminal: Terminal,
    ring: Ring,
    last_io: Instant,
}

impl Session {
    // At most one page per pass per PTY; no busy writer can monopolize the lock.
    fn drain(&mut self) -> io::Result<()> {
        let mut buf = [0; MAX_READ];
        match self.terminal.master.read(&mut buf) {
            Ok(0) => Err(io::Error::from(ErrorKind::UnexpectedEof)),
            Ok(n) => {
                self.ring.push(&buf[..n]);
                self.last_io = Instant::now();
                Ok(())
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<SessionId, Session>,
    stopping: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn watch(&self, idle: Duration) {
        let mut state = self.lock();
        while !state.stopping {
            state
                .sessions
                .retain(|_, s| s.last_io.elapsed() < idle && s.drain().is_ok());
            state = if state.sessions.is_empty() {
                self.wake.wait(state).unwrap_or_else(|e| e.into_inner())
            } else {
                self.wake
                    .wait_timeout(state, idle.min(Duration::from_millis(25)))
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            };
        }
        state.sessions.clear(); // Kill and wait before the watchdog returns.
    }
}

pub struct PtyProvider {
    image: Image,
    shared: Arc<Shared>,
    watchdog: Option<JoinHandle<()>>,
    idle: Duration,
}

impl PtyProvider {
    /// Pin the trusted host installation. Unsupported layouts and missing
    /// kernel protections fail closed; there is no unconfined fallback.
    pub fn new() -> io::Result<Self> {
        Self::with_timeout(IDLE_TIMEOUT)
    }

    fn with_timeout(idle: Duration) -> io::Result<Self> {
        let image = Image::pin()?;
        let shared = Arc::new(Shared::default());
        let watcher = shared.clone();
        let watchdog = thread::Builder::new()
            .name("eon-pty".into())
            .spawn(move || watcher.watch(idle))?;
        Ok(Self {
            image,
            shared,
            watchdog: Some(watchdog),
            idle,
        })
    }

    /// `&mut self` serializes admission, but launch never holds the watchdog lock.
    pub fn pty_open(&mut self, owner: &WorkflowId) -> io::Result<SessionId> {
        if self.shared.lock().sessions.len() >= MAX_SESSIONS {
            return Err(io::Error::new(ErrorKind::ResourceBusy, "PTY limit reached"));
        }
        let terminal = self.image.launch()?;
        let id = SessionId::new();
        self.shared.lock().sessions.insert(
            id.clone(),
            Session {
                owner: owner.clone(),
                terminal,
                ring: Ring::default(),
                last_io: Instant::now(),
            },
        );
        self.shared.wake.notify_one();
        Ok(id)
    }

    fn access<T>(
        &self,
        owner: &WorkflowId,
        id: &SessionId,
        op: impl FnOnce(&mut Session) -> io::Result<T>,
    ) -> io::Result<T> {
        let absent = || io::Error::new(ErrorKind::NotFound, "unknown PTY");
        let mut state = self.shared.lock();
        let Entry::Occupied(mut entry) = state.sessions.entry(id.clone()) else {
            return Err(absent());
        };
        if &entry.get().owner != owner {
            return Err(absent());
        }
        if entry.get().last_io.elapsed() >= self.idle {
            entry.remove();
            return Err(absent());
        }
        let result = op(entry.get_mut());
        if let Err(e) = &result
            && !matches!(
                e.kind(),
                ErrorKind::InvalidInput | ErrorKind::WouldBlock | ErrorKind::Interrupted
            )
        {
            entry.remove(); // A failed worker/transport must not outlive the session.
        }
        result
    }

    /// Nonblocking. A short count consumes only that prefix; retry the suffix.
    pub fn pty_write(&self, owner: &WorkflowId, id: &SessionId, data: &[u8]) -> io::Result<usize> {
        self.access(owner, id, |s| {
            if !(1..=MAX_WRITE).contains(&data.len()) {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "invalid PTY write length",
                ));
            }
            let n = s.terminal.master.write(data)?;
            if n == 0 {
                return Err(io::Error::from(ErrorKind::WriteZero));
            }
            s.last_io = Instant::now();
            Ok(n)
        })
    }

    /// Nonblocking byte pagination, including incomplete lines. An empty poll
    /// is not I/O and does not keep a session alive.
    pub fn pty_read_ring(
        &self,
        owner: &WorkflowId,
        id: &SessionId,
        cursor: u64,
        limit: usize,
    ) -> io::Result<RingPage> {
        self.access(owner, id, |s| {
            let page = s.ring.page(cursor, limit)?;
            if !page.bytes.is_empty() {
                s.last_io = Instant::now();
            }
            Ok(page)
        })
    }

    /// Only typed signals to this session's own terminal/group, never a caller PID.
    pub fn pty_signal(
        &self,
        owner: &WorkflowId,
        id: &SessionId,
        signal: PtySignal,
    ) -> io::Result<()> {
        self.access(owner, id, |s| s.terminal.signal(signal))
    }

    /// Synchronously revoke and reap all sessions belonging to this workflow.
    pub fn close_owner(&mut self, owner: &WorkflowId) {
        self.shared.lock().sessions.retain(|_, s| &s.owner != owner);
    }
}

impl Drop for PtyProvider {
    fn drop(&mut self) {
        self.shared.lock().stopping = true;
        self.shared.wake.notify_one();
        if let Some(worker) = self.watchdog.take() {
            let _ = worker.join();
        }
    }
}
