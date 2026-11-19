use std::fs;
use std::io::Write;
use std::path::Path;

use eon_capability::{EonError, EonResult, wasm_tool::call_i32_export};

fn assert_missing_import(outcome: EonResult<i32>, namespace: &str, name: &str) {
    let Err(EonError::OperationFailed { operation, reason }) = outcome else {
        panic!("expected missing {namespace}::{name}, got {outcome:?}");
    };
    assert_eq!(operation, "wasm.instantiate");
    assert!(
        reason.contains(&format!("unknown import: `{namespace}::{name}`")),
        "{reason}"
    );
}

#[test]
fn host_io_and_socket_creation_are_not_linked() {
    let fd_signature = "(param i32 i32 i32 i32) (result i32)";
    for (namespace, name, signature) in [
        ("wasi_snapshot_preview1", "fd_read", fd_signature),
        ("wasi_snapshot_preview1", "fd_write", fd_signature),
        (
            "wasi:sockets/tcp-create-socket@0.2.0",
            "create-tcp-socket",
            "(param i32 i32)",
        ),
    ] {
        // Even an unused import must be resolved before the guest can start.
        let module = format!(
            r#"(module (import "{namespace}" "{name}" (func {signature}))
            (func (export "run") (param i32) (result i32) local.get 0))"#
        );
        assert_missing_import(
            call_i32_export(module.as_bytes(), "run", 0, 1000),
            namespace,
            name,
        );
    }
}

#[test]
fn path_open_is_not_linked_and_the_host_sentinel_is_unchanged() {
    const CONTENTS: &[u8] = b"host-side fixture\n";
    let name = format!("wasm-sentinel-{:016x}", getrandom::u64().unwrap());
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::File::create_new(&path)
        .unwrap()
        .write_all(CONTENTS)
        .unwrap();
    let bytes = path.as_os_str().as_encoded_bytes();
    let guest_path: String = bytes.iter().map(|b| format!(r"\{b:02x}")).collect();
    let module = format!(
        r#"(module
        (import "wasi_snapshot_preview1" "path_open" (func $open
            (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 16) "{guest_path}")
        (func (export "run") (param i32) (result i32)
            ;; dirfd, flags, path, length, TRUNC, FD_READ|FD_WRITE, inheriting rights, flags, result slot
            i32.const 3 i32.const 0 i32.const 16 i32.const {} i32.const 8
            i64.const 66 i64.const 0 i32.const 0 i32.const 0 call $open))"#,
        bytes.len()
    );
    let outcome = call_i32_export(module.as_bytes(), "run", 0, 1000);
    let observed = fs::read(&path);
    fs::remove_file(path).unwrap();
    assert_eq!(observed.unwrap(), CONTENTS);
    assert_missing_import(outcome, "wasi_snapshot_preview1", "path_open");
}
