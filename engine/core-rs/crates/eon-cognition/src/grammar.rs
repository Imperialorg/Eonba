//! GBNF grammar for tool calls (docs/implementation/02, structured output).
//!
//! The model may only emit `{"name": <allowed tool>, "arguments": <object>}`. The tool names come from a fixed allow-list,
//! and each name must match `[a-z0-9._-]+`, so a name cannot close a string or add a rule. Arguments are any JSON object.

use eon_primitives::{EonError, EonResult};

/// Builds the GBNF grammar for the given tool names, with rule `root` as the entry point.
pub fn tool_call_grammar(tool_names: &[&str]) -> EonResult<String> {
    if tool_names.is_empty() {
        return Err(EonError::ValidationError("at least one tool name is required".into()));
    }
    let mut alternatives = Vec::with_capacity(tool_names.len());
    for name in tool_names {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
        if !valid {
            return Err(EonError::ValidationError(format!("tool name {name:?} is not allowed in a grammar")));
        }
        alternatives.push(format!("\"\\\"{name}\\\"\""));
    }
    Ok(format!(
        r#"root ::= "{{" ws "\"name\"" ws ":" ws name ws "," ws "\"arguments\"" ws ":" ws object ws "}}"
name ::= {names}
object ::= "{{" ws ( string ws ":" ws value ( ws "," ws string ws ":" ws value )* )? ws "}}"
value ::= object | array | string | number | "true" | "false" | "null"
array ::= "[" ws ( value ( ws "," ws value )* )? ws "]"
string ::= "\"" ( [^"\\] | "\\" ["\\/bfnrt] )* "\""
number ::= "-"? ("0" | [1-9] [0-9]*) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
ws ::= [ \t\n]*
"#,
        names = alternatives.join(" | ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_lists_every_allowed_tool_name() {
        let grammar = tool_call_grammar(&["shell.exec", "fs.read"]).unwrap();
        assert!(grammar.contains(r#"name ::= "\"shell.exec\"" | "\"fs.read\"""#), "{grammar}");
        assert!(grammar.starts_with("root ::="));
    }

    #[test]
    fn names_that_could_escape_the_grammar_are_refused() {
        assert!(tool_call_grammar(&[]).is_err());
        assert!(tool_call_grammar(&["x\" | value"]).is_err());
        assert!(tool_call_grammar(&["Shell.Exec"]).is_err());
        assert!(tool_call_grammar(&[""]).is_err());
    }
}
