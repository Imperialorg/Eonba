//! Local daemon over a Unix domain socket (docs/implementation/05, section 5).
//!
//! The socket is created with mode 0600 and owned by the daemon's user. Each connection is checked against the
//! socket owner's UID with `SO_PEERCRED`, so another local user cannot read journal state even if file permissions
//! are changed later. Protocol: one request line, one response, then close.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use eon_cli::{read_status, render_text};
use eon_primitives::{EonError, EonResult};

fn io_err(context: &str, e: std::io::Error) -> EonError {
    EonError::OperationFailed {
        operation: format!("daemon.{context}"),
        reason: e.to_string(),
    }
}

/// Binds the socket. A stale socket left by a previous run is replaced. Any other file at the path is refused,
/// so the daemon never deletes data it did not create.
pub fn bind(path: &Path) -> EonResult<UnixListener> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_socket() {
            fs::remove_file(path).map_err(|e| io_err("remove_stale", e))?;
        } else {
            return Err(EonError::ValidationError(format!(
                "{} exists and is not a socket; refusing to replace it",
                path.display()
            )));
        }
    }
    let listener = UnixListener::bind(path).map_err(|e| io_err("bind", e))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| io_err("chmod", e))?;
    Ok(listener)
}

/// UID of the process on the other end, read with SO_PEERCRED. std keeps `peer_cred` unstable.
fn peer_uid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for the duration of the call and sized for `ucred`.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(cred.uid)
}

/// Serves one connection. Peers whose UID differs from the socket owner's are refused before any data is read.
pub fn handle_connection(stream: UnixStream, journal_dir: &Path, owner_uid: u32) -> EonResult<()> {
    let peer = peer_uid(&stream).map_err(|e| io_err("peer_cred", e))?;
    let mut writer = stream.try_clone().map_err(|e| io_err("clone", e))?;
    if peer != owner_uid {
        writer
            .write_all(b"ERR unauthorized\n")
            .map_err(|e| io_err("write", e))?;
        return Err(EonError::SecurityViolation(format!("refused peer uid {peer}")));
    }
    let mut request = String::new();
    BufReader::new(stream)
        .read_line(&mut request)
        .map_err(|e| io_err("read", e))?;
    let response = match request.trim() {
        "STATUS" => match read_status(journal_dir) {
            Ok(status) => render_text(&status),
            Err(e) => format!("ERR {e}\n"),
        },
        _ => "ERR unknown command\n".to_string(),
    };
    writer
        .write_all(response.as_bytes())
        .map_err(|e| io_err("write", e))
}

/// Accepts `count` connections sequentially. Returns the first handler error, if any, after serving all of them.
pub fn serve(listener: &UnixListener, journal_dir: &Path, count: usize) -> EonResult<()> {
    let owner_uid = fs::metadata(
        listener
            .local_addr()
            .map_err(|e| io_err("local_addr", e))?
            .as_pathname()
            .ok_or_else(|| EonError::InternalError("listener has no pathname".into()))?,
    )
    .map_err(|e| io_err("owner", e))?
    .uid();
    let mut first_error = None;
    for _ in 0..count {
        let (stream, _) = listener.accept().map_err(|e| io_err("accept", e))?;
        if let Err(e) = handle_connection(stream, journal_dir, owner_uid) {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Client side: sends one command and returns the full response.
pub fn request(path: &Path, command: &str) -> EonResult<String> {
    let mut stream = UnixStream::connect(path).map_err(|e| io_err("connect", e))?;
    stream
        .write_all(format!("{command}\n").as_bytes())
        .map_err(|e| io_err("send", e))?;
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response).map_err(|e| io_err("recv", e))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::thread;

    use eon_foundation::journal::JournalWriter;

    use super::*;

    fn journal_with(frames: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = JournalWriter::open(dir.path()).unwrap();
        for i in 0..frames {
            writer.append_batch(&[format!("e{i}").as_bytes()]).unwrap();
        }
        dir
    }

    #[test]
    fn status_round_trip_over_the_socket() {
        let journal = journal_with(5);
        let runtime = tempfile::tempdir().unwrap();
        let sock = runtime.path().join("eon.sock");
        let listener = bind(&sock).unwrap();
        let journal_path = journal.path().to_path_buf();
        let server = thread::spawn(move || serve(&listener, &journal_path, 1));

        let response = request(&sock, "STATUS").unwrap();
        server.join().unwrap().unwrap();
        assert!(response.contains("frames: 5"), "response: {response}");
        assert!(response.contains("last durable seq: 5"));
    }

    #[test]
    fn socket_is_only_accessible_to_its_owner() {
        let runtime = tempfile::tempdir().unwrap();
        let sock = runtime.path().join("eon.sock");
        let _listener = bind(&sock).unwrap();
        let mode = fs::metadata(&sock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn unknown_commands_are_rejected_without_touching_the_journal() {
        let journal = journal_with(1);
        let runtime = tempfile::tempdir().unwrap();
        let sock = runtime.path().join("eon.sock");
        let listener = bind(&sock).unwrap();
        let journal_path = journal.path().to_path_buf();
        let server = thread::spawn(move || serve(&listener, &journal_path, 1));
        let response = request(&sock, "APPEND evil").unwrap();
        server.join().unwrap().unwrap();
        assert_eq!(response, "ERR unknown command\n");
    }

    #[test]
    fn peer_uid_reports_the_connecting_process_uid() {
        let runtime = tempfile::tempdir().unwrap();
        let sock = runtime.path().join("eon.sock");
        let listener = bind(&sock).unwrap();
        let client = UnixStream::connect(&sock).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        let our_uid = fs::metadata(&sock).unwrap().uid();
        assert_eq!(peer_uid(&server_side).unwrap(), our_uid);
        drop(client);
    }

    #[test]
    fn a_regular_file_at_the_socket_path_is_never_deleted() {
        let runtime = tempfile::tempdir().unwrap();
        let path = runtime.path().join("eon.sock");
        fs::write(&path, b"precious").unwrap();
        assert!(bind(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"precious");
    }

    #[test]
    fn a_stale_socket_from_a_previous_run_is_replaced() {
        let runtime = tempfile::tempdir().unwrap();
        let path = runtime.path().join("eon.sock");
        drop(bind(&path).unwrap());
        assert!(bind(&path).is_ok());
    }
}
