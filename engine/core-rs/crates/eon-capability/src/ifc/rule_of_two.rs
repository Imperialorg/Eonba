//! Meta's Rule of Two as a deterministic dispatch gate (docs/implementation/01, section 5.2).
//!
//! A task may hold at most two of three properties at once without a human in the loop:
//! A. it processes untrusted input, B. it touches sensitive data, C. it mutates state or sends data out.
//! The check runs in the kernel, outside any model, so no prompt or classifier can talk its way past it.

use crate::ifc::labels::{ConfidentialityLevel, IntegrityLevel, SecurityLabel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    ReadWorkspace,
    MutateWorkspace,
    ExecuteProcess,
    DeviceControl,
    /// Network access. Unpinned endpoints and endpoints taking query parameters count as egress.
    NetworkAccess { pinned: bool, query_parameterized: bool },
}

impl Capability {
    fn mutates_or_egresses(&self) -> bool {
        match self {
            Self::MutateWorkspace | Self::ExecuteProcess | Self::DeviceControl => true,
            Self::NetworkAccess { pinned, query_parameterized } => !pinned || *query_parameterized,
            Self::ReadWorkspace => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlParameter {
    pub name: String,
    pub label: SecurityLabel,
}

#[derive(Debug, Clone)]
pub struct TaskProfile {
    /// Join of the context ledger and every task input.
    pub context: SecurityLabel,
    /// The task resolves a `secret://` handle.
    pub resolves_secret: bool,
    /// The task reads outside its ephemeral sandbox.
    pub reads_outside_sandbox: bool,
    pub capabilities: Vec<Capability>,
    /// Values that choose what the task executes, such as `cmd`, `url`, `patch`, or `target_path`.
    pub control_parameters: Vec<ControlParameter>,
    /// Runs inside an air-gapped sandbox: no host mounts and no network egress.
    pub air_gapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Properties {
    pub processes_untrusted: bool,
    pub accesses_sensitive: bool,
    pub mutates_or_egresses: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    RuleOfTwoTriadViolation,
    UntrustedControlParameter { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Autonomous execution is refused. A human must approve the exact serialized action.
    RequireRawHumanGate(Reason),
}

pub fn properties(profile: &TaskProfile) -> Properties {
    Properties {
        processes_untrusted: profile.context.integrity == IntegrityLevel::Untrusted,
        accesses_sensitive: profile.context.confidentiality >= ConfidentialityLevel::Workspace
            || profile.resolves_secret
            || profile.reads_outside_sandbox,
        mutates_or_egresses: profile.capabilities.iter().any(Capability::mutates_or_egresses),
    }
}

pub fn decide(profile: &TaskProfile) -> Decision {
    let props = properties(profile);

    if props.processes_untrusted && props.accesses_sensitive && props.mutates_or_egresses {
        return Decision::RequireRawHumanGate(Reason::RuleOfTwoTriadViolation);
    }

    // An untrusted value choosing what runs is only tolerable inside a sandbox that cannot reach the host.
    let steered_by_untrusted = profile
        .control_parameters
        .iter()
        .find(|p| p.label.integrity == IntegrityLevel::Untrusted);
    if let Some(param) = steered_by_untrusted.filter(|_| props.processes_untrusted && props.mutates_or_egresses && !profile.air_gapped) {
        return Decision::RequireRawHumanGate(Reason::UntrustedControlParameter {
            name: param.name.clone(),
        });
    }

    Decision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted() -> SecurityLabel {
        SecurityLabel::new(IntegrityLevel::Trusted, ConfidentialityLevel::Public, vec![])
    }

    fn untrusted_web() -> SecurityLabel {
        SecurityLabel::external("https://example.test")
    }

    fn profile(context: SecurityLabel, sensitive: bool, caps: Vec<Capability>) -> TaskProfile {
        TaskProfile {
            context,
            resolves_secret: sensitive,
            reads_outside_sandbox: false,
            capabilities: caps,
            control_parameters: vec![],
            air_gapped: false,
        }
    }

    #[test]
    fn all_three_properties_always_require_a_human() {
        let p = profile(untrusted_web(), true, vec![Capability::MutateWorkspace]);
        assert_eq!(decide(&p), Decision::RequireRawHumanGate(Reason::RuleOfTwoTriadViolation));
    }

    #[test]
    fn air_gap_does_not_exempt_the_triad() {
        let mut p = profile(untrusted_web(), true, vec![Capability::ExecuteProcess]);
        p.air_gapped = true;
        assert_eq!(decide(&p), Decision::RequireRawHumanGate(Reason::RuleOfTwoTriadViolation));
    }

    #[test]
    fn two_of_three_is_allowed_without_a_control_parameter() {
        let p = profile(untrusted_web(), false, vec![Capability::ExecuteProcess]);
        assert_eq!(decide(&p), Decision::Allow);
    }

    #[test]
    fn untrusted_command_line_is_gated_outside_an_air_gap() {
        let mut p = profile(trusted(), false, vec![Capability::ExecuteProcess]);
        p.context = SecurityLabel::external("repo");
        p.control_parameters.push(ControlParameter {
            name: "cmd".into(),
            label: untrusted_web(),
        });
        assert_eq!(
            decide(&p),
            Decision::RequireRawHumanGate(Reason::UntrustedControlParameter { name: "cmd".into() })
        );
    }

    #[test]
    fn decompiling_an_untrusted_binary_is_allowed_inside_an_air_gap() {
        let mut p = profile(untrusted_web(), false, vec![Capability::ExecuteProcess]);
        p.control_parameters.push(ControlParameter {
            name: "target_path".into(),
            label: untrusted_web(),
        });
        p.air_gapped = true;
        assert_eq!(decide(&p), Decision::Allow);
    }

    #[test]
    fn pinned_read_only_network_is_not_egress() {
        let caps = vec![Capability::NetworkAccess { pinned: true, query_parameterized: false }];
        assert!(!properties(&profile(untrusted_web(), true, caps)).mutates_or_egresses);
    }

    #[test]
    fn query_parameterized_network_counts_as_egress_even_when_pinned() {
        let caps = vec![Capability::NetworkAccess { pinned: true, query_parameterized: true }];
        assert!(properties(&profile(untrusted_web(), false, caps)).mutates_or_egresses);
    }

    #[test]
    fn trusted_owner_with_no_untrusted_input_is_never_gated_by_the_untrusted_rule() {
        let mut p = profile(trusted(), true, vec![Capability::ExecuteProcess]);
        p.control_parameters.push(ControlParameter {
            name: "cmd".into(),
            label: trusted(),
        });
        assert_eq!(decide(&p), Decision::Allow);
    }
}
