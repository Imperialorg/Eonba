//! Boots a real Firecracker microVM on KVM and runs a command in it. Needs `/dev/kvm` and these files, which
//! `scripts/fetch_firecracker.sh` puts in place:
//!   EON_FC_BIN, EON_FC_KERNEL, EON_FC_INITRD
//! Run with: cargo test -p eon-capability --test microvm_e2e -- --ignored

use std::path::PathBuf;
use std::time::Duration;

use eon_capability::microvm::{run_script, MicroVm};

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("set {name} to run this test")))
}

#[test]
#[ignore = "needs /dev/kvm and a Firecracker kernel; see scripts/fetch_firecracker.sh"]
fn a_command_runs_inside_a_booted_microvm_and_the_guest_powers_off() {
    let vm = MicroVm {
        firecracker: env_path("EON_FC_BIN"),
        kernel: env_path("EON_FC_KERNEL"),
        initrd: env_path("EON_FC_INITRD"),
        vcpus: 1,
        mem_mib: 128,
    };
    let outcome = run_script(&vm, "echo EON_MARKER_$((6*7))\nreboot -f\n", Duration::from_secs(60))
        .expect("the guest must boot and power off within the deadline");
    assert!(outcome.serial.contains("EON_MARKER_42"), "serial output: {}", outcome.serial);
}
