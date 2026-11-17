//! Polyglot code graph from tree-sitter syntax trees (docs/implementation/03, code property graph).
//!
//! Functions are the nodes. An edge goes from a function to every function it calls, when the callee name matches a
//! function defined in the same file. Calls to names defined elsewhere are kept as unresolved, so the caller sees
//! them instead of losing them. Only Rust and Python are supported for now, and each language's call and definition
//! node kinds are listed explicitly, so an unsupported construct is never silently treated as a call.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use eon_primitives::{EonError, EonResult};
use tree_sitter::{Language, Node, Parser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Python,
}

impl Lang {
    fn language(self) -> Language {
        match self {
            Lang::Rust => Language::new(tree_sitter_rust::LANGUAGE),
            Lang::Python => Language::new(tree_sitter_python::LANGUAGE),
        }
    }

    /// Node kind for a function definition. Both grammars name its identifier field `name`.
    fn function_kind(self) -> &'static str {
        match self {
            Lang::Rust => "function_item",
            Lang::Python => "function_definition",
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CodeGraph {
    /// Function names defined in the file, in source order.
    pub functions: Vec<String>,
    /// Resolved edges: caller name to callee name, both defined in this file.
    pub edges: BTreeSet<(String, String)>,
    /// Calls whose callee is not defined in this file, keyed by caller name.
    pub unresolved: BTreeMap<String, BTreeSet<String>>,
}

impl CodeGraph {
    /// True when `to` can be reached from `from` through resolved edges.
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([from.to_owned()]);
        while let Some(current) = queue.pop_front() {
            if current == to {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            for (caller, callee) in &self.edges {
                if *caller == current {
                    queue.push_back(callee.clone());
                }
            }
        }
        false
    }
}

fn parse_error(reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: "cpg.parse".into(),
        reason: reason.to_string(),
    }
}

/// Last identifier of a callee expression: `helper` for `helper()`, `new` for `Thing::new()`, `run` for `self.run()`.
fn callee_name(node: Node, source: &[u8]) -> Option<String> {
    let text = |n: Node| n.utf8_text(source).ok().map(str::to_owned);
    match node.kind() {
        "identifier" => text(node),
        "scoped_identifier" => node.child_by_field_name("name").and_then(text),
        "field_expression" => node.child_by_field_name("field").and_then(text),
        "attribute" => node.child_by_field_name("attribute").and_then(text),
        _ => None,
    }
}

fn call_callee(lang: Lang, call: Node) -> Option<Node> {
    match lang {
        Lang::Rust if call.kind() == "call_expression" => call.child_by_field_name("function"),
        Lang::Python if call.kind() == "call" => call.child_by_field_name("function"),
        _ => None,
    }
}

struct Walk<'a> {
    lang: Lang,
    source: &'a [u8],
    calls: Vec<(String, String)>,
    defined: Vec<String>,
}

impl Walk<'_> {
    fn visit(&mut self, node: Node, current: Option<&str>) {
        let definition = (node.kind() == self.lang.function_kind())
            .then(|| node.child_by_field_name("name").and_then(|n| n.utf8_text(self.source).ok()).map(str::to_owned))
            .flatten();
        if let Some(name) = definition {
            self.defined.push(name.clone());
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.visit(child, Some(&name));
            }
            return;
        }
        if let (Some(caller), Some(callee)) = (current, call_callee(self.lang, node).and_then(|c| callee_name(c, self.source))) {
            self.calls.push((caller.to_owned(), callee));
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, current);
        }
    }
}

/// Builds the code graph for one source file.
pub fn build(lang: Lang, source: &str) -> EonResult<CodeGraph> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).map_err(parse_error)?;
    let tree = parser.parse(source, None).ok_or_else(|| parse_error("parser produced no tree"))?;
    let mut walk = Walk {
        lang,
        source: source.as_bytes(),
        calls: Vec::new(),
        defined: Vec::new(),
    };
    walk.visit(tree.root_node(), None);

    let defined: BTreeSet<&str> = walk.defined.iter().map(String::as_str).collect();
    let mut graph = CodeGraph {
        functions: walk.defined.clone(),
        ..CodeGraph::default()
    };
    for (caller, callee) in &walk.calls {
        if defined.contains(callee.as_str()) {
            graph.edges.insert((caller.clone(), callee.clone()));
        } else {
            graph.unresolved.entry(caller.clone()).or_default().insert(callee.clone());
        }
    }
    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = r#"
fn handle_request(input: &str) -> String {
    let cleaned = sanitize(input);
    store(&cleaned);
    format!("{cleaned}")
}
fn sanitize(s: &str) -> String { s.trim().to_string() }
fn store(s: &str) { write_to_disk(s); }
fn unrelated() { println_helper(); }
"#;

    const PYTHON: &str = r#"
def handler(payload):
    cleaned = sanitize(payload)
    return run_query(cleaned)

def sanitize(text):
    return text.strip()

def run_query(q):
    return db.execute(q)
"#;

    #[test]
    fn rust_calls_resolve_within_the_file_and_external_calls_are_kept() {
        let graph = build(Lang::Rust, RUST).unwrap();
        assert!(graph.functions.contains(&"handle_request".to_owned()));
        assert!(graph.edges.contains(&("handle_request".into(), "sanitize".into())));
        assert!(graph.edges.contains(&("handle_request".into(), "store".into())));
        assert!(graph.unresolved["store"].contains("write_to_disk"));
        assert!(graph.unresolved["unrelated"].contains("println_helper"));
    }

    #[test]
    fn reachability_follows_resolved_edges_only() {
        let graph = build(Lang::Rust, RUST).unwrap();
        assert!(graph.reaches("handle_request", "store"));
        assert!(!graph.reaches("store", "handle_request"));
        assert!(!graph.reaches("unrelated", "store"));
    }

    #[test]
    fn python_calls_and_attribute_calls_are_extracted() {
        let graph = build(Lang::Python, PYTHON).unwrap();
        assert!(graph.reaches("handler", "run_query"));
        assert!(graph.reaches("handler", "sanitize"));
        assert!(graph.unresolved["run_query"].contains("execute"), "{:?}", graph.unresolved);
    }
}
