//! Local operator tool. The only endpoint constructor here requires the
//! pre-existing pair/provider-specific Linux grant, including in private labs.
#[cfg(target_os = "linux")]
#[path = "continuity/profile.rs"]
mod profile;
#[cfg(target_os = "linux")]
mod linux {
    use super::profile::Profile;
    use eon_mesh::wan::{
        N0Provider, WanEndpoint,
        continuity::{CapturePlan, receive, transmit},
    };
    use std::{
        io::IsTerminal,
        net::Ipv4Addr,
        num::NonZeroU32,
        os::fd::AsFd,
        path::Path,
        sync::mpsc,
        time::{Duration, Instant},
    };

    const USAGE: &str =
        "usage: eon-wan-continuity <send|receive> <profile-directory> <seconds:1..120>";

    fn fresh_enter(input: &impl AsFd, prompt: &str) -> Option<Instant> {
        eprintln!("{prompt}");
        // Discard input queued through issuance of *this* prompt. Read the fd
        // directly: a buffered stdin reader could retain the next mark in userspace.
        rustix::termios::tcflush(input, rustix::termios::QueueSelector::IFlush).ok()?;
        let mut byte = [0];
        (rustix::io::read(input, &mut byte[..]).ok()? == 1 && byte[0] == b'\n').then(Instant::now)
    }

    fn arm() -> mpsc::Receiver<Instant> {
        let (tx, rx) = mpsc::sync_channel(2);
        // A missing operator must not block capture, runtime shutdown or close.
        std::thread::spawn(move || {
            let input = std::io::stdin();
            for prompt in [
                "Press Enter at the BEGINNING of the actual transition.",
                "Press Enter when the actual transition ENDS.",
            ] {
                let Some(at) = fresh_enter(&input, prompt) else {
                    eprintln!("Could not read a fresh empty line from the terminal.");
                    break;
                };
                if tx.send(at).is_err() {
                    break;
                }
            }
        });
        rx
    }

    pub async fn run() -> Result<(), &'static str> {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let [role, dir, seconds] = args.as_slice() else {
            return Err(USAGE);
        };
        let plan = seconds
            .to_str()
            .and_then(|s| s.parse().ok())
            .and_then(CapturePlan::new)
            .ok_or(USAGE)?;
        let receiver = match role.to_str() {
            Some("send") => false,
            Some("receive") if std::io::stdin().is_terminal() => true,
            Some("receive") => return Err("receiver_requires_local_terminal_for_marks"),
            _ => return Err(USAGE),
        };
        if ["SSLKEYLOGFILE", "QLOGDIR"]
            .iter()
            .any(|k| std::env::var_os(k).is_some())
        {
            return Err("transport_trace_environment_not_permitted");
        }
        let profile = Profile::open(Path::new(dir))?;
        let relay: Ipv4Addr = profile.parse("relay.ip")?;
        let pkarr: Ipv4Addr = profile.parse("pkarr.ip")?;
        if !relay.is_private() || !pkarr.is_private() {
            return Err("expected_private_mirror_literals");
        }
        let endpoint = WanEndpoint::open_managed_n0(
            profile.parse("identity.key")?,
            profile.parse("peer.pin")?,
            N0Provider::PrivateMirror {
                relay,
                pkarr,
                root_der: profile.read("ca.der", 64 * 1024)?,
            },
            NonZeroU32::new(64 * 1024).unwrap(),
            Duration::from_secs(30),
        )
        .await
        .map_err(|err| match err {
            eon_mesh::EonError::SecurityViolation(_) => "managed_grant_or_peer_denied",
            _ => "managed_open_failed",
        })?;
        if receiver {
            let report = receive(endpoint, plan, arm).await;
            print!("{report}");
            if !report.bracketed() {
                return Err("transition_not_bracketed");
            }
        } else {
            println!("queued={}", transmit(endpoint, plan).await?);
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use rustix::{
            io::ioctl_fionread,
            pty::{OpenptFlags, ioctl_tiocgptpeer, openpt, unlockpt},
        };
        use std::{fs::File, io::Write, thread};

        #[test]
        fn each_mark_discards_buffered_pty_enters() {
            let flags = OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC;
            let mut terminal = File::from(openpt(flags).unwrap());
            unlockpt(&terminal).unwrap();
            let input = File::from(ioctl_tiocgptpeer(&terminal, flags).unwrap());
            let wait_queue = |empty| {
                let stop = Instant::now() + Duration::from_secs(3);
                while (ioctl_fionread(&input).unwrap() == 0) != empty {
                    assert!(Instant::now() < stop, "PTY queue did not change");
                    thread::sleep(Duration::from_millis(1));
                }
            };
            terminal.write_all(b"\n\n").unwrap(); // typed before first decode
            for (prompt, fresh) in [("BEGIN", &b"\n\n\n"[..]), ("END", &b"\n"[..])] {
                // The extra BEGIN newlines are stale by the END prompt.
                wait_queue(false);
                let tty = input.try_clone().unwrap();
                let reader = thread::spawn(move || fresh_enter(&tty, prompt));
                wait_queue(true);
                let not_before = Instant::now();
                terminal.write_all(fresh).unwrap();
                assert!(reader.join().unwrap().unwrap() >= not_before);
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match linux::run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("Requires a managed Linux network namespace.");
    std::process::ExitCode::FAILURE
}
