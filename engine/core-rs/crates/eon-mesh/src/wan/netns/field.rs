//! Run the field executable unmodified, with stdin on a real PTY at the receiver.
use super::{Duration, fs, mark, sleep, timeout, wait, write_stale_record};
use rustix::{
    io::ioctl_fionread,
    pty::{OpenptFlags, ioctl_tiocgptpeer, openpt, unlockpt},
};
use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Child, Command, Stdio},
};

struct Receiver(Child);

impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Receiver {
    async fn prompt(&mut self, text: &str, tty: &File) {
        timeout(Duration::from_secs(40), async {
            loop {
                assert!(self.0.try_wait().unwrap().is_none(), "receiver exited");
                if fs::read_to_string("field-prompts.log")
                    .unwrap()
                    .contains(text)
                    && ioctl_fionread(tty).unwrap() == 0
                {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("missing prompt or terminal input not flushed");
    }
}

pub(super) async fn run(receiver: bool) {
    let (side, other, role) = if receiver {
        ("a", "b", "receive")
    } else {
        ("b", "a", "send")
    };
    let profile = Path::new("keys").canonicalize().unwrap().join(side);
    fs::create_dir(&profile).unwrap();
    fs::copy(format!("keys/{side}.key"), profile.join("identity.key")).unwrap();
    fs::copy(format!("keys/{other}.pub"), profile.join("peer.pin")).unwrap();
    fs::copy("keys/ca.der", profile.join("ca.der")).unwrap();
    fs::write(profile.join("relay.ip"), "10.77.0.1").unwrap();
    fs::write(profile.join("pkarr.ip"), "10.77.0.4").unwrap();
    let key = fs::read_to_string(profile.join("identity.key")).unwrap();
    write_stale_record(side, &key.trim().parse().unwrap());

    let mut cmd = Command::new(std::env::var_os("EON_FIELD_EXE").unwrap());
    cmd.arg(role).arg(profile).arg("30");
    if !receiver {
        let status = cmd
            .stdin(Stdio::null())
            .stdout(File::create("field-sent.log").unwrap())
            .status()
            .unwrap();
        assert!(status.success(), "field sender: {status}");
        return;
    }
    let flags = OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC;
    let mut terminal = File::from(openpt(flags).unwrap());
    unlockpt(&terminal).unwrap();
    let tty = File::from(ioctl_tiocgptpeer(&terminal, flags).unwrap());
    // A stale newline at each prompt lets us observe its tcflush before
    // sending the real mark. No line is accepted as a substitute for the cut.
    terminal.write_all(b"\n").unwrap();
    let mut child = Receiver(
        cmd.stdin(tty.try_clone().unwrap())
            .stdout(File::create("field-receipts.log").unwrap())
            .stderr(File::create("field-prompts.log").unwrap())
            .spawn()
            .unwrap(),
    );
    child.prompt("BEGINNING", &tty).await;
    wait("direct-traffic").await;
    terminal.write_all(b"\n\n").unwrap();
    child.prompt("ENDS", &tty).await;
    mark("field-begin");
    wait("cut").await;
    terminal.write_all(b"\n").unwrap();
    let status = child.0.wait().unwrap();
    assert!(status.success(), "field receiver: {status}");
}
