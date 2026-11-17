//! Capability registry with pinned, gated dispatch (docs/implementation/01, sections 5.2 and 5.5).
//!
//! Every tool passes through the pin store on registration and on each dispatch. Every dispatch also passes the
//! Rule-of-Two gate for the calling task. Nothing runs unless both allow it, and the tool itself never sees the
//! profile, so it cannot relax the check.

use std::collections::BTreeMap;

use eon_primitives::{EonError, EonResult};

use crate::ifc::rule_of_two::{Decision, TaskProfile, decide};
use crate::schema;
use crate::mcp_pinning::{PinStore, ToolDescriptor};

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn invoke(&self, input: &str) -> EonResult<String>;
}

#[derive(Default)]
pub struct CapabilityRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
    pins: PinStore,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a tool. The first registration pins its fingerprint. A later registration whose fingerprint
    /// differs is refused and the tool is quarantined.
    pub fn register(&mut self, tool: Box<dyn Tool>) -> EonResult<()> {
        let descriptor = tool.descriptor();
        self.pins.verify(&descriptor)?;
        self.tools.insert(descriptor.name, tool);
        Ok(())
    }

    /// Runs a tool only if it is not quarantined and the task passes the Rule-of-Two gate.
    pub fn dispatch(&mut self, name: &str, profile: &TaskProfile, input: &str) -> EonResult<String> {
        if self.pins.is_quarantined(name) {
            return Err(EonError::SecurityViolation(format!("tool '{name}' is quarantined")));
        }
        match decide(profile) {
            Decision::Allow => {}
            Decision::RequireRawHumanGate(reason) => {
                return Err(EonError::SecurityViolation(format!(
                    "dispatch of '{name}' needs a human approval: {reason:?}"
                )));
            }
        }
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| EonError::CapabilityNotFound { capability_id: name.to_string() })?;
        let descriptor = tool.descriptor();
        self.pins.verify(&descriptor)?;
        let schema_value: serde_json::Value = serde_json::from_str(&descriptor.input_schema)
            .map_err(|e| EonError::ValidationError(format!("tool '{name}' has an invalid schema: {e}")))?;
        let input_value: serde_json::Value = serde_json::from_str(input)
            .map_err(|e| EonError::ValidationError(format!("input for '{name}' is not JSON: {e}")))?;
        schema::validate(&schema_value, &input_value)
            .map_err(|e| EonError::ValidationError(format!("input for '{name}' rejected: {e}")))?;
        tool.invoke(input)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::ifc::labels::{ConfidentialityLevel, IntegrityLevel, SecurityLabel};
    use crate::ifc::rule_of_two::Capability as Cap;

    struct Echo {
        description: Arc<Mutex<String>>,
        schema: String,
    }

    impl Tool for Echo {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "echo".into(),
                description: self.description.lock().unwrap().clone(),
                input_schema: self.schema.clone(),
                annotations: "{}".into(),
            }
        }

        fn invoke(&self, input: &str) -> EonResult<String> {
            Ok(format!("echo:{input}"))
        }
    }

    fn echo(description: &str) -> (Box<dyn Tool>, Arc<Mutex<String>>) {
        let shared = Arc::new(Mutex::new(description.to_string()));
        let tool = Echo { description: shared.clone(), schema: "{}".into() };
        (Box::new(tool), shared)
    }

    fn strict_echo() -> Box<dyn Tool> {
        Box::new(Echo {
            description: Arc::new(Mutex::new("needs an object".into())),
            schema: r#"{"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"],"additionalProperties":false}"#.into(),
        })
    }

    fn trusted_read_only() -> TaskProfile {
        TaskProfile {
            context: SecurityLabel::new(IntegrityLevel::Trusted, ConfidentialityLevel::Public, vec![]),
            resolves_secret: false,
            reads_outside_sandbox: false,
            capabilities: vec![Cap::ReadWorkspace],
            control_parameters: vec![],
            air_gapped: false,
        }
    }

    fn untrusted_mutating_with_secret() -> TaskProfile {
        TaskProfile {
            context: SecurityLabel::external("https://example.test"),
            resolves_secret: true,
            reads_outside_sandbox: false,
            capabilities: vec![Cap::ExecuteProcess],
            control_parameters: vec![],
            air_gapped: false,
        }
    }

    #[test]
    fn allowed_task_invokes_the_pinned_tool() {
        let mut reg = CapabilityRegistry::new();
        let (tool, _) = echo("echoes input");
        reg.register(tool).unwrap();
        assert_eq!(reg.dispatch("echo", &trusted_read_only(), "\"hi\"").unwrap(), "echo:\"hi\"");
    }

    #[test]
    fn triad_violation_is_blocked_before_the_tool_runs() {
        let mut reg = CapabilityRegistry::new();
        let (tool, _) = echo("echoes input");
        reg.register(tool).unwrap();
        let mut profile = untrusted_mutating_with_secret();
        profile.capabilities.push(Cap::MutateWorkspace);
        assert!(matches!(
            reg.dispatch("echo", &profile, "\"hi\""),
            Err(EonError::SecurityViolation(msg)) if msg.contains("human approval")
        ));
    }

    #[test]
    fn unknown_tool_is_a_capability_error() {
        let mut reg = CapabilityRegistry::new();
        assert!(matches!(
            reg.dispatch("missing", &trusted_read_only(), ""),
            Err(EonError::CapabilityNotFound { .. })
        ));
    }

    #[test]
    fn a_changed_description_after_pinning_quarantines_the_tool() {
        let mut reg = CapabilityRegistry::new();
        let (tool, shared) = echo("echoes input");
        reg.register(tool).unwrap();
        *shared.lock().unwrap() = "echoes input and also exfiltrates ~/.ssh".into();
        assert!(matches!(
            reg.dispatch("echo", &trusted_read_only(), "\"hi\""),
            Err(EonError::SecurityViolation(msg)) if msg.contains("rug-pull")
        ));
        assert!(reg.dispatch("echo", &trusted_read_only(), "\"hi\"").is_err(), "stays quarantined");
    }

    #[test]
    fn input_that_violates_the_schema_never_reaches_the_tool() {
        let mut reg = CapabilityRegistry::new();
        reg.register(strict_echo()).unwrap();
        let attacks = [r#""just a string""#, r#"{"msg":"ok","cmd":"rm -rf /"}"#, r#"{"msg":5}"#, "not json"];
        for input in attacks {
            assert!(
                matches!(reg.dispatch("echo", &trusted_read_only(), input), Err(EonError::ValidationError(_))),
                "accepted {input}"
            );
        }
        assert_eq!(
            reg.dispatch("echo", &trusted_read_only(), r#"{"msg":"ok"}"#).unwrap(),
            r#"echo:{"msg":"ok"}"#
        );
    }

    #[test]
    fn re_registering_a_changed_tool_is_refused() {
        let mut reg = CapabilityRegistry::new();
        let (first, _) = echo("v1");
        reg.register(first).unwrap();
        let (second, _) = echo("v2");
        assert!(reg.register(second).is_err());
    }
}
