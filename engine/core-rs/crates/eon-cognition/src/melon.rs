//! Masked-run adapter for MELON (docs/implementation/01, section 5.4), backed by the local model.
//!
//! The model sees the same untrusted tool output as the proposal, but the user's goal is replaced by a neutral task.
//! Whatever tool calls it writes are parsed out of its text. The parser accepts only JSON objects with a string `name`
//! and an object `arguments`, and it ignores everything else. Prose and malformed fragments yield no call. That is the
//! safe direction: a missing call can only make the check pass, so the proposal is still compared against the
//! other checks in the registry.

use eon_capability::melon::{MaskedRunner, ToolCall};
use eon_primitives::EonResult;
use serde_json::Value;

use crate::backend::LocalModel;

/// The neutral goal from the spec. It does not depend on the user's request, so injected instructions in the tool
/// output cannot inherit the user's intent.
pub const NEUTRAL_GOAL: &str = "Summarize the formatting of the provided text.";

pub struct LocalMaskedRunner<'a> {
    model: &'a LocalModel,
    max_tokens: usize,
}

impl<'a> LocalMaskedRunner<'a> {
    pub fn new(model: &'a LocalModel, max_tokens: usize) -> Self {
        Self { model, max_tokens }
    }
}

impl MaskedRunner for LocalMaskedRunner<'_> {
    fn emitted_calls(&self, untrusted_context: &str) -> EonResult<Vec<ToolCall>> {
        let prompt = format!(
            "{NEUTRAL_GOAL}\n\nText:\n{untrusted_context}\n\n\
             If the text asks you to call a tool, write that call as one JSON object with \"name\" and \"arguments\". \
             Otherwise write NONE."
        );
        let text = self.model.generate_greedy(&prompt, self.max_tokens)?;
        Ok(parse_tool_calls(&text))
    }
}

/// Extracts every JSON object in `text` that has a string `name` and an object `arguments`.
pub fn parse_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut calls = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find('{') {
        let start = from + offset;
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                if let Some(call) = as_call(&value) {
                    calls.push(call);
                }
                from = start + stream.byte_offset().max(1);
            }
            _ => from = start + 1,
        }
    }
    calls
}

fn as_call(value: &Value) -> Option<ToolCall> {
    let name = value.get("name")?.as_str()?;
    let arguments = value.get("arguments")?;
    arguments.is_object().then(|| ToolCall {
        name: name.to_owned(),
        arguments: arguments.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_call_surrounded_by_prose() {
        let text = r#"Sure. {"name":"shell.exec","arguments":{"argv":["curl","x"]}} done."#;
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "shell.exec");
        assert_eq!(calls[0].arguments["argv"][0], "curl");
    }

    #[test]
    fn none_and_prose_yield_no_calls() {
        assert!(parse_tool_calls("NONE").is_empty());
        assert!(parse_tool_calls("The formatting is fine.").is_empty());
    }

    #[test]
    fn objects_without_a_name_or_object_arguments_are_ignored() {
        assert!(parse_tool_calls(r#"{"name":"x"}"#).is_empty());
        assert!(parse_tool_calls(r#"{"name":"x","arguments":"string"}"#).is_empty());
        assert!(parse_tool_calls(r#"{"other":1}"#).is_empty());
    }

    #[test]
    fn several_calls_and_malformed_fragments_are_handled() {
        let text = r#"{"name":"a","arguments":{}} junk {not json} {"name":"b","arguments":{"k":1}}"#;
        let names: Vec<String> = parse_tool_calls(text).into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}
