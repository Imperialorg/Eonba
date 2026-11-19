//! Confinement for tool processes: Landlock filesystem rules and a seccomp filter that denies socket creation
//! (docs/implementation/01, section 5, sandbox layer).
//!
//! Everything is prepared in the parent, where allocation is safe. The child only calls `restrict_self` and
//! `apply_filter` between `fork` and `exec`. If the kernel does not enforce Landlock, the child fails before `exec`,
//! so a tool never runs unconfined by accident.

use std::collections::BTreeMap;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::Mutex;

use eon_primitives::{EonError, EonResult};
use landlock::{
    Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus,
    ABI,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch};

/// What a confined process may do. Filesystem access outside `read_only` is denied, and with `deny_network` the
/// process cannot create sockets at all (AF_UNIX included, which is the conservative choice).
#[derive(Debug, Clone, Default)]
pub struct ConfinementPolicy {
    pub read_only: Vec<PathBuf>,
    pub deny_network: bool,
}

fn sandbox_err(context: &str, e: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: format!("sandbox.{context}"),
        reason: e.to_string(),
    }
}

fn build_ruleset(policy: &ConfinementPolicy) -> EonResult<RulesetCreated> {
    let abi = ABI::V3;
    let mut created = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .and_then(|r| r.create())
        .map_err(|e| sandbox_err("ruleset", e))?;
    for path in &policy.read_only {
        let fd = PathFd::new(path).map_err(|e| sandbox_err("path", format!("{}: {e}", path.display())))?;
        created = created
            .add_rule(PathBeneath::new(fd, AccessFs::from_read(abi) | AccessFs::Execute))
            .map_err(|e| sandbox_err("rule", e))?;
    }
    Ok(created)
}

fn socket_deny_filter() -> EonResult<BpfProgram> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    rules.insert(libc::SYS_socket, Vec::new());
    rules.insert(libc::SYS_socketpair, Vec::new());
    let arch = if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        TargetArch::x86_64
    };
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|e| sandbox_err("seccomp", e))?;
    filter.try_into().map_err(|e: seccompiler::BackendError| sandbox_err("seccomp", e))
}

/// Runs `program` with `args` under `policy` and returns its output. Fails if confinement cannot be enforced.
pub fn run_confined(program: &str, args: &[&str], policy: &ConfinementPolicy) -> EonResult<Output> {
    let ruleset = Mutex::new(Some(build_ruleset(policy)?));
    let network_filter = if policy.deny_network { Some(socket_deny_filter()?) } else { None };
    let mut command = Command::new(program);
    command.args(args);
    // SAFETY: the closure runs in the forked child before exec. It takes prebuilt objects, calls `restrict_self`
    // (landlock, sets no_new_privs) and then `apply_filter` (seccomp). Nothing is allocated for the filter up front.
    unsafe {
        command.pre_exec(move || {
            let created = ruleset
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .ok_or_else(|| io::Error::other("sandbox ruleset already consumed"))?;
            match created.restrict_self() {
                Ok(status) if status.ruleset != RulesetStatus::NotEnforced => {}
                Ok(_) => return Err(io::Error::other("landlock not enforced by this kernel")),
                Err(e) => return Err(io::Error::other(e.to_string())),
            }
            if let Some(filter) = &network_filter {
                seccompiler::apply_filter(filter).map_err(|e| io::Error::other(e.to_string()))?;
            }
            Ok(())
        });
    }
    command.output().map_err(|e| sandbox_err("spawn", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eon-sandbox-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn confined_process_reads_allowed_file_and_is_denied_outside() {
        let allowed = fixture_dir("allowed");
        let secret = fixture_dir("secret");
        std::fs::write(allowed.join("ok.txt"), "allowed-content").unwrap();
        std::fs::write(secret.join("key.txt"), "TOP-SECRET").unwrap();

        let policy = ConfinementPolicy {
            read_only: vec![
                allowed.clone(),
                PathBuf::from("/usr"),
                PathBuf::from("/lib"),
                PathBuf::from("/lib64"),
                PathBuf::from("/bin"),
            ],
            deny_network: false,
        };
        let ok = run_confined("/bin/cat", &[allowed.join("ok.txt").to_str().unwrap()], &policy).unwrap();
        assert!(ok.status.success(), "allowed read failed: {}", String::from_utf8_lossy(&ok.stderr));
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "allowed-content");

        let denied = run_confined("/bin/cat", &[secret.join("key.txt").to_str().unwrap()], &policy).unwrap();
        assert!(!denied.status.success(), "read outside the policy succeeded");
        assert!(!String::from_utf8_lossy(&denied.stdout).contains("TOP-SECRET"));

        std::fs::remove_dir_all(allowed).ok();
        std::fs::remove_dir_all(secret).ok();
    }

    fn socket_probe() -> (&'static str, Vec<&'static str>) {
        ("/usr/bin/python3", vec!["-c", "import socket; socket.socket(); print('socket-ok')"])
    }

    #[test]
    fn socket_creation_succeeds_without_network_denial() {
        let (program, args) = socket_probe();
        let policy = ConfinementPolicy {
            read_only: vec![PathBuf::from("/usr"), PathBuf::from("/lib"), PathBuf::from("/lib64"), PathBuf::from("/etc")],
            deny_network: false,
        };
        let out = run_confined(program, &args, &policy).unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("socket-ok"));
    }

    #[test]
    fn socket_creation_is_refused_when_network_is_denied() {
        let (program, args) = socket_probe();
        let policy = ConfinementPolicy {
            read_only: vec![PathBuf::from("/usr"), PathBuf::from("/lib"), PathBuf::from("/lib64"), PathBuf::from("/etc")],
            deny_network: true,
        };
        let out = run_confined(program, &args, &policy).unwrap();
        assert!(!out.status.success(), "socket creation succeeded under deny_network");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("socket-ok"));
    }
}
