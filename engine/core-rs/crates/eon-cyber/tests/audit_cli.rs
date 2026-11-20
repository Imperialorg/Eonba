use serde_json::Value;
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn audit(mode: &str, input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_eon-audit"))
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn reports_byte_ranges_but_never_reflects_stdin() {
    let src = "def sample(db, val):\n    db.execute(f'private_marker {val}')\n";
    let output = audit("python", src.as_bytes());
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        !std::str::from_utf8(&output.stdout)
            .unwrap()
            .contains("private_marker")
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let sql = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["cwe"] == "CWE-89")
        .unwrap();
    assert_eq!(sql["status"], "observed_syntax");
    assert_eq!(
        &src[sql["start_byte"].as_u64().unwrap() as usize
            ..sql["end_byte"].as_u64().unwrap() as usize],
        "db.execute(f'private_marker {val}')"
    );
}

#[test]
fn malformed_source_is_not_reported_clean() {
    let output = audit("rust", b"fn private_marker(");
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["status"] == "unverified")
    );
    for data in [vec![0xff; 4], vec![b'a'; 16 * 1024 + 1]] {
        let refused = audit("rust", &data);
        assert_eq!(refused.status.code(), Some(2));
        assert!(refused.stdout.is_empty());
    }
}

#[test]
fn self_io_reports_only_numeric_evidence_or_a_gap() {
    let output = audit("self-io", &[]);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success());
    assert_eq!(report["source"], "proc_self_io");
    if cfg!(target_os = "linux") && std::fs::read_to_string("/proc/self/io").is_ok() {
        assert_eq!(report["status"], "observed");
        assert!(report["read_bytes"].is_u64() && report["write_bytes"].is_u64());
    } else {
        assert!(report["status"] == "unverified" || report["status"] == "observed");
    }
}
