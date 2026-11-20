//! Linux/glibc backend. The host OS installation and procfs are trusted, the
//! workspace and caller are not. No PATH lookup, interpreter, or runtime directory grant.
use std::fs::File;
use std::io::{self, ErrorKind, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, FileTypeExt, MetadataExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use rustix::fs::{self, Mode, OFlags};
use rustix::process::{self, Resource, Rlimit, Signal};

use super::PtySignal;
use crate::sandbox::{ConfinementPolicy, WorkerIo, confine_command};

// Walk from a trusted root using directory handles, never following symlinks.
// Checking the file alone would also accept a link to a different root-owned binary.
fn host_elf(path: &Path) -> io::Result<File> {
    let mut parts = path.components();
    if parts.next() != Some(Component::RootDir) {
        return Err(io::Error::from(ErrorKind::InvalidInput));
    }
    let mut file = File::open("/")?;
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(io::Error::from(ErrorKind::InvalidInput));
        };
        let m = file.metadata()?;
        if !m.is_dir() || m.uid() != 0 || m.mode() & 0o022 != 0 {
            return Err(io::Error::from(ErrorKind::PermissionDenied));
        }
        file = fs::openat(
            &file,
            name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )?
        .into();
    }
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != 0 || m.mode() & 0o6022 != 0 || file.as_raw_fd() < 3 {
        return Err(io::Error::from(ErrorKind::PermissionDenied));
    }
    let mut magic = [0; 4];
    file.read_exact_at(&mut magic, 0)?;
    if &magic != b"\x7fELF" {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "expected host ELF image",
        ));
    }
    Ok(file)
}

fn fd_path(f: &File) -> PathBuf {
    format!("/proc/self/fd/{}", f.as_raw_fd()).into()
}

pub(super) struct Image {
    utility: Utility,
    program: File,
    libc: File,
    loader: File,
}

#[derive(Clone, Copy)]
pub(super) enum Utility {
    Cat,
    Tr,
    Wc,
}

impl Utility {
    fn name(self) -> &'static str {
        match self {
            Self::Cat => "cat",
            Self::Tr => "tr",
            Self::Wc => "wc",
        }
    }
}

impl Image {
    pub(super) fn pin(utility: Utility) -> io::Result<Self> {
        if fs::fstatfs(File::open("/proc/self/fd")?)?.f_type != fs::PROC_SUPER_MAGIC {
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "PTY requires procfs",
            ));
        }
        let program = host_elf(&Path::new("/usr/bin").join(utility.name()))?;
        if program.metadata()?.mode() & 0o111 == 0 {
            return Err(io::Error::from(ErrorKind::PermissionDenied));
        }
        let (multiarch, ld) = match std::env::consts::ARCH {
            "x86_64" => ("x86_64-linux-gnu", "ld-linux-x86-64.so.2"),
            "aarch64" => ("aarch64-linux-gnu", "ld-linux-aarch64.so.1"),
            "riscv64" => ("riscv64-linux-gnu", "ld-linux-riscv64-lp64d.so.1"),
            _ => {
                return Err(io::Error::new(
                    ErrorKind::Unsupported,
                    "unsupported native PTY ABI",
                ));
            }
        };
        // Only individual files in the host glibc installation. Neither the
        // workspace nor /etc, /proc, /usr or a library tree is readable.
        let lib = Path::new("/usr/lib");
        for dir in [lib.join(multiarch), "/usr/lib64".into(), lib.into()] {
            let pin_lib = |name| host_elf(&dir.join(name));
            match (pin_lib("libc.so.6"), pin_lib(ld)) {
                (Ok(libc), Ok(loader)) => {
                    return Ok(Self {
                        utility,
                        program,
                        libc,
                        loader,
                    });
                }
                (Err(e), _) | (_, Err(e)) if e.kind() != ErrorKind::NotFound => return Err(e),
                _ => continue,
            }
        }
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "missing trusted PTY runtime",
        ))
    }

    fn command(&self, args: &[&'static str]) -> io::Result<Command> {
        // An auto-reaped leader cannot pin its process-group ID for cleanup.
        // SAFETY: null act is a read-only query; success initializes previous.
        let previous = unsafe {
            let mut previous = MaybeUninit::uninit();
            cvt(libc::sigaction(
                libc::SIGCHLD,
                std::ptr::null(),
                previous.as_mut_ptr(),
            ))?;
            previous.assume_init()
        };
        if previous.sa_sigaction == libc::SIG_IGN || previous.sa_flags & libc::SA_NOCLDWAIT != 0 {
            return Err(io::Error::other(
                "provider requires waitable child processes",
            ));
        }
        let mut cmd = Command::new(fd_path(&self.program));
        cmd.arg0(self.utility.name()).args(args);
        Ok(cmd)
    }

    fn confine(&self, cmd: &mut Command) -> io::Result<()> {
        let policy = ConfinementPolicy {
            read_only: vec![
                fd_path(&self.program),
                fd_path(&self.loader),
                fd_path(&self.libc),
            ],
            deny_network: true,
        };
        // SAFETY: child-only rlimit syscalls, before installing seccomp.
        unsafe {
            cmd.pre_exec(|| {
                limit(Resource::Core, 0)?;
                limit(Resource::As, 64 * 1024 * 1024)
            });
        }
        confine_command(cmd, &policy, WorkerIo::StdioProtocol).map_err(io::Error::other)
    }

    pub(super) fn batch(&self, args: &[&'static str], max_file: u64) -> io::Result<Command> {
        let mut cmd = self.command(args)?;
        let parent = process::getpid();
        // SAFETY: child-only syscalls before confinement; the synchronous batch
        // child must not outlive its calling thread if the host exits abruptly.
        unsafe {
            cmd.pre_exec(move || {
                process::set_parent_process_death_signal(Some(Signal::KILL))?;
                if process::getppid() != Some(parent) {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                limit(Resource::Fsize, max_file)
            });
        }
        self.confine(&mut cmd)?;
        Ok(cmd)
    }

    pub(super) fn launch(
        &self,
        args: &[&'static str],
        probe: &[u8],
        reply: &[u8],
    ) -> io::Result<Terminal> {
        let mut cmd = self.command(args)?;
        let (master, slave) = pair()?;
        // SAFETY: in the child stdio is already installed. setsid makes a new
        // process group and session before claiming the slave as controlling
        // terminal. Only async-signal-safe native operations before confinement.
        unsafe {
            cmd.pre_exec(|| {
                cvt(libc::setsid())?;
                cvt(libc::ioctl(0, libc::TIOCSCTTY, 0))?;
                let mut signals = MaybeUninit::uninit();
                cvt(libc::sigemptyset(signals.as_mut_ptr()))?;
                let mut signals = signals.assume_init();
                for sig in [libc::SIGINT, libc::SIGTERM] {
                    if libc::signal(sig, libc::SIG_DFL) == libc::SIG_ERR {
                        return Err(io::Error::last_os_error());
                    }
                    cvt(libc::sigaddset(&mut signals, sig))?;
                }
                cvt(libc::sigprocmask(
                    libc::SIG_UNBLOCK,
                    &signals,
                    std::ptr::null_mut(),
                ))?;
                Ok(())
            });
        }
        self.confine(&mut cmd)?;
        // Replace just the default stdio pipes, retaining all confinement hooks,
        // the empty environment and the fixed working directory.
        cmd.stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave);
        let mut terminal = Terminal {
            master,
            child: cmd.spawn()?,
        };
        drop(cmd);
        terminal.verify(probe, reply)?;
        Ok(terminal)
    }
}

fn limit(resource: Resource, cap: u64) -> io::Result<()> {
    // Never raise an inherited limit, even if the host has the privilege to do so.
    let value = Some(process::getrlimit(resource).current.unwrap_or(cap).min(cap));
    Ok(process::setrlimit(
        resource,
        Rlimit {
            current: value,
            maximum: value,
        },
    )?)
}

fn cvt(n: libc::c_int) -> io::Result<libc::c_int> {
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n)
    }
}

const TERMINAL_FLAGS: i32 = libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOCTTY;

fn peer(master: &File, flags: i32) -> io::Result<File> {
    // SAFETY: on success the ioctl returns a new descriptor for this exact
    // master's peer on its actual devpts mount, without resolving a /dev path.
    unsafe {
        let fd = cvt(libc::ioctl(master.as_raw_fd(), libc::TIOCGPTPEER, flags))?;
        Ok(File::from_raw_fd(fd))
    }
}

pub(super) fn private_locked_pair() -> io::Result<(File, File)> {
    // SAFETY: posix_openpt returns an owned master; the two query ioctls
    // initialize the supplied integers without changing the new slave's lock.
    let (master, number) = unsafe {
        let fd = cvt(libc::posix_openpt(TERMINAL_FLAGS | libc::O_NONBLOCK))?;
        let master = File::from_raw_fd(fd);
        let mut locked: libc::c_int = 0;
        let mut number: libc::c_uint = 0;
        cvt(libc::ioctl(fd, libc::TIOCGPTLCK, &mut locked))?;
        cvt(libc::ioctl(fd, libc::TIOCGPTN, &mut number))?;
        if locked != 1 {
            return Err(io::Error::from(ErrorKind::PermissionDenied));
        }
        (master, number)
    };
    // O_PATH does not open the terminal for I/O. The slave may initially be
    // 0620 and owned by group tty. Do not unlock until it is operator-only.
    let pin = peer(&master, libc::O_PATH | libc::O_CLOEXEC)?;
    let meta = pin.metadata()?;
    let uid = rustix::process::geteuid();
    let gid = rustix::process::getegid();
    if fs::fstatfs(&pin)?.f_type != libc::DEVPTS_SUPER_MAGIC
        || !meta.file_type().is_char_device()
        || libc::major(meta.rdev()) != 136 // UNIX98_PTY_SLAVE_MAJOR
        || libc::minor(meta.rdev()) != number
        || meta.uid() != uid.as_raw()
    {
        return Err(io::Error::from(ErrorKind::PermissionDenied));
    }
    // fchmod cannot operate on O_PATH. Follow only the kernel's magic link
    // to our live descriptor, relative to a verified procfs directory.
    let procfd = File::open("/proc/self/fd")?;
    if fs::fstatfs(&procfd)?.f_type != fs::PROC_SUPER_MAGIC {
        return Err(io::Error::from(ErrorKind::PermissionDenied));
    }
    fs::chmodat(
        &procfd,
        pin.as_raw_fd().to_string(),
        Mode::RUSR | Mode::WUSR,
        fs::AtFlags::empty(),
    )?;
    fs::chownat(&pin, "", None, Some(gid), fs::AtFlags::EMPTY_PATH)?;
    let meta = pin.metadata()?;
    if meta.mode() & 0o7777 != 0o600 || meta.uid() != uid.as_raw() || meta.gid() != gid.as_raw() {
        return Err(io::Error::from(ErrorKind::PermissionDenied));
    }
    Ok((master, pin))
}

fn pair() -> io::Result<(File, File)> {
    let (master, pin) = private_locked_pair()?;
    // SAFETY: only open this verified peer. Re-lock against future data opens;
    // the already-open slave can be configured and passed directly to the worker.
    unsafe {
        let fd = master.as_raw_fd();
        cvt(libc::unlockpt(fd))?;
        let slave = peer(&master, TERMINAL_FLAGS)?;
        cvt(libc::ioctl(fd, libc::TIOCSPTLCK, &1_i32))?;
        let expected = pin.metadata()?;
        let actual = slave.metadata()?;
        if (actual.dev(), actual.ino()) != (expected.dev(), expected.ino()) {
            return Err(io::Error::from(ErrorKind::PermissionDenied));
        }
        let peer = slave.as_raw_fd();
        let mut attr = MaybeUninit::uninit();
        cvt(libc::tcgetattr(peer, attr.as_mut_ptr()))?;
        let mut attr = attr.assume_init();
        libc::cfmakeraw(&mut attr); // No line-discipline echo, signals, or byte translations.
        cvt(libc::tcsetattr(peer, libc::TCSANOW, &attr))?;
        resize(peer, 24, 80)?;
        Ok((master, slave))
    }
}

fn resize(fd: i32, rows: u16, cols: u16) -> io::Result<()> {
    if rows == 0 || cols == 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "zero PTY dimension",
        ));
    }
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ reads this initialized winsize while the fd is live.
    unsafe {
        cvt(libc::ioctl(fd, libc::TIOCSWINSZ, &size))?;
    }
    Ok(())
}

pub(super) struct Terminal {
    pub(super) master: File,
    child: Child,
}

impl Terminal {
    // exec success alone doesn't mean the loader or filter worked. Consume the
    // entire private response (possibly split across reads) with kernel echo off.
    fn verify(&mut self, probe: &[u8], mut reply: &[u8]) -> io::Result<()> {
        self.master.write_all(probe)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut pfd = libc::pollfd {
            fd: self.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        while !reply.is_empty() {
            let ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as i32;
            if ms == 0 {
                return Err(io::Error::from(ErrorKind::TimedOut));
            }
            // SAFETY: pfd is a single initialized pollfd for the live master.
            match cvt(unsafe { libc::poll(&mut pfd, 1, ms) }) {
                Ok(0) => return Err(io::Error::from(ErrorKind::TimedOut)),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                result => {
                    result?;
                }
            }
            let mut answer = [0; 8];
            match self.master.read(&mut answer) {
                Ok(n) if n != 0 && reply.starts_with(&answer[..n]) => reply = &reply[n..],
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    continue;
                }
                _ => return Err(io::Error::other("confined PTY failed its startup probe")),
            }
        }
        Ok(())
    }

    fn signal_group(&self, sig: i32) -> io::Result<()> {
        // SAFETY: setsid established a private group; the leader is our child
        // and has not been waited on. The host must not independently reap it.
        unsafe {
            cvt(libc::kill(-(self.child.id() as i32), sig))?;
        }
        Ok(())
    }

    pub(super) fn signal(&self, sig: PtySignal) -> io::Result<()> {
        match sig {
            PtySignal::Interrupt => self.signal_group(libc::SIGINT),
            PtySignal::Terminate => self.signal_group(libc::SIGTERM),
            PtySignal::Resize { rows, cols } => resize(self.master.as_raw_fd(), rows, cols),
        }
    }

    #[cfg(test)]
    pub(super) fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.signal_group(libc::SIGKILL);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
