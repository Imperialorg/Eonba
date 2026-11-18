//! Firecracker microVM runs (docs/implementation/03, microVM sandbox).
//!
//! A guest is booted from a kernel and an initramfs by the `firecracker` binary on KVM. The serial console is the guest's
//! stdin and stdout. The script is typed only after the shell prints its prompt, so no input is lost during boot. The
//! script must end with `reboot -f`: the guest's `reboot=k` turns that into a KVM reset, which makes the VMM exit. A run
//! that does not exit by its deadline is killed, and the error carries the serial output so far.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eon_primitives::{EonError, EonResult};

/// The shell prompt of the guest's initramfs. Input typed before it is lost during boot.
const PROMPT: &str = "/ # ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicroVm {
    pub firecracker: PathBuf,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub vcpus: u8,
    pub mem_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub serial: String,
    pub exit_code: Option<i32>,
}

fn vm_err(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("microvm.{operation}"),
        reason: reason.to_string(),
    }
}

fn config_json(vm: &MicroVm) -> String {
    serde_json::json!({
        "boot-source": {
            "kernel_image_path": vm.kernel,
            "initrd_path": vm.initrd,
            "boot_args": "console=ttyS0 reboot=k panic=1 pci=off",
        },
        "drives": [],
        "machine-config": { "vcpu_count": vm.vcpus, "mem_size_mib": vm.mem_mib },
    })
    .to_string()
}

/// Boots `vm`, types `script` into its serial console, and waits for the guest to power off.
pub fn run_script(vm: &MicroVm, script: &str, deadline: Duration) -> EonResult<Outcome> {
    for (label, path) in [("firecracker", &vm.firecracker), ("kernel", &vm.kernel), ("initrd", &vm.initrd)] {
        if !path.exists() {
            return Err(vm_err("missing", format!("{label} not found at {}", path.display())));
        }
    }
    let dir = std::env::temp_dir().join(format!("eon-microvm-{}-{}", std::process::id(), nanos()));
    std::fs::create_dir_all(&dir).map_err(|e| vm_err("workdir", e))?;
    let config_path = dir.join("vm.json");
    std::fs::write(&config_path, config_json(vm)).map_err(|e| vm_err("config", e))?;

    let mut child = Command::new(Path::new(&vm.firecracker))
        .arg("--no-api")
        .arg("--config-file")
        .arg(&config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| vm_err("spawn", e))?;

    let mut stdin = child.stdin.take().ok_or_else(|| vm_err("console", "no stdin pipe"))?;
    let mut stdout = child.stdout.take().ok_or_else(|| vm_err("console", "no stdout pipe"))?;
    let serial = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&serial);
    let reader = std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut out) = sink.lock() {
                        out.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    }
                }
            }
        }
    });

    let started = Instant::now();
    let snapshot = |serial: &Arc<Mutex<String>>| serial.lock().map(|s| s.clone()).unwrap_or_default();
    while !snapshot(&serial).contains(PROMPT) {
        if started.elapsed() >= deadline || child.try_wait().map_err(|e| vm_err("wait", e))?.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stdin
        .write_all(script.as_bytes())
        .and_then(|()| stdin.flush())
        .map_err(|e| vm_err("console", e))?;

    let (exit_code, timed_out) = loop {
        match child.try_wait().map_err(|e| vm_err("wait", e))? {
            Some(status) => break (status.code(), false),
            None if started.elapsed() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let _ = reader.join();
    let serial = snapshot(&serial);
    let _ = std::fs::remove_dir_all(&dir);
    if timed_out {
        return Err(vm_err("timeout", format!("guest still running after {deadline:?}; serial so far: {serial}")));
    }
    Ok(Outcome { serial, exit_code })
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_names_the_kernel_initrd_and_sizes() {
        let vm = MicroVm {
            firecracker: "/bin/false".into(),
            kernel: "/k/vmlinux".into(),
            initrd: "/k/initramfs.cpio".into(),
            vcpus: 2,
            mem_mib: 256,
        };
        let json: serde_json::Value = serde_json::from_str(&config_json(&vm)).unwrap();
        assert_eq!(json["boot-source"]["kernel_image_path"], "/k/vmlinux");
        assert_eq!(json["boot-source"]["initrd_path"], "/k/initramfs.cpio");
        assert_eq!(json["machine-config"]["vcpu_count"], 2);
        assert_eq!(json["machine-config"]["mem_size_mib"], 256);
    }

    #[test]
    fn a_missing_firecracker_binary_is_refused_before_anything_starts() {
        let vm = MicroVm {
            firecracker: "/nonexistent/firecracker".into(),
            kernel: "/nonexistent/vmlinux".into(),
            initrd: "/nonexistent/initramfs".into(),
            vcpus: 1,
            mem_mib: 128,
        };
        let err = run_script(&vm, "reboot -f\n", Duration::from_secs(1)).unwrap_err().to_string();
        assert!(err.contains("firecracker not found"), "{err}");
    }
}
