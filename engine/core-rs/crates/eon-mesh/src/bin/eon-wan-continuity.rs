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
        io::{IsTerminal, Read},
        net::Ipv4Addr,
        num::NonZeroU32,
        path::Path,
        sync::mpsc,
        time::{Duration, Instant},
    };

    const USAGE: &str =
        "usage: eon-wan-continuity <send|receive> <profile-directory> <seconds:1..120>";

    fn arm() -> mpsc::Receiver<Instant> {
        let (tx, rx) = mpsc::sync_channel(2);
        // A missing operator must not block capture, runtime shutdown or close.
        std::thread::spawn(move || {
            let mut input = std::io::stdin().lock();
            for prompt in [
                "Press Enter at the BEGINNING of the actual transition.",
                "Press Enter when the actual transition ENDS.",
            ] {
                eprintln!("{prompt}");
                let mut byte = [0];
                if input.read_exact(&mut byte).is_err() || byte[0] != b'\n' {
                    eprintln!("Marker input ended or was not an empty line.");
                    break;
                }
                if tx.send(Instant::now()).is_err() {
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
