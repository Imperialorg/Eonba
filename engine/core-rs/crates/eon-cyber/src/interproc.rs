//! Interprocedural taint over the code graph (docs/implementation/03, sections 2 and 3).
//!
//! The slicer in `taint` works on a data-flow graph the caller builds. This module takes the call graph from `cpg`
//! and finds source functions that reach sink functions through resolved calls. A sanitizer function is removed from
//! the search, so a flow that only exists through a sanitizer is not reported. The shortest witness path is returned
//! for each (source, sink) pair, and the result uses the same `Finding` type as the intraprocedural slicer.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::cpg::CodeGraph;
use crate::taint::Finding;

/// Source, sink, and sanitizer function names for one analysis.
pub struct TaintSpec<'a> {
    pub sources: &'a [&'a str],
    pub sinks: &'a [&'a str],
    pub sanitizers: &'a [&'a str],
}

/// Shortest path from `start` to any sink, avoiding sanitizers. Returns the path, source to sink, inclusive.
fn shortest_path_to_sink(
    adjacency: &BTreeMap<&str, BTreeSet<&str>>,
    start: &str,
    sinks: &BTreeSet<&str>,
    sanitizers: &BTreeSet<&str>,
) -> Option<Vec<String>> {
    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some(current) = queue.pop_front() {
        if current != start && sinks.contains(current) {
            let mut path = vec![current.to_owned()];
            let mut at = current;
            while let Some(&prev) = parent.get(at) {
                path.push(prev.to_owned());
                at = prev;
            }
            path.reverse();
            return Some(path);
        }
        for &next in adjacency.get(current).into_iter().flatten() {
            if sanitizers.contains(next) || !seen.insert(next) {
                continue;
            }
            parent.insert(next, current);
            queue.push_back(next);
        }
    }
    None
}

/// Returns one finding per (source, sink) pair that is reachable through resolved calls without a sanitizer.
pub fn call_flow_findings(graph: &CodeGraph, spec: &TaintSpec<'_>) -> Vec<Finding> {
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (caller, callee) in &graph.edges {
        adjacency.entry(caller.as_str()).or_default().insert(callee.as_str());
    }
    let defined: BTreeSet<&str> = graph.functions.iter().map(String::as_str).collect();
    let sinks: BTreeSet<&str> = spec.sinks.iter().copied().filter(|s| defined.contains(s)).collect();
    let sanitizers: BTreeSet<&str> = spec.sanitizers.iter().copied().collect();

    let mut findings = Vec::new();
    for &source in spec.sources {
        if !defined.contains(source) || sanitizers.contains(source) {
            continue;
        }
        let mut reachable_sinks: BTreeSet<&str> = sinks.clone();
        reachable_sinks.remove(source);
        if let Some(path) = shortest_path_to_sink(&adjacency, source, &reachable_sinks, &sanitizers) {
            let sink = path.last().cloned().unwrap_or_default();
            findings.push(Finding {
                source: source.to_owned(),
                sink,
                witness: path,
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpg::{build, Lang};

    const SANITIZED: &str = r#"
fn read_request() -> String { String::from("payload") }
fn clean(input: &str) -> String { input.trim().to_string() }
fn normalize(input: &str) -> String {
    let cleaned = clean(input);
    dispatch(&cleaned);
    cleaned
}
fn run_shell(cmd: &str) { std::process::Command::new(cmd); }
fn dispatch(cmd: &str) { run_shell(cmd); }
fn handle() {
    let raw = read_request();
    normalize(&raw);
}
"#;

    const BYPASS: &str = r#"
fn read_request() -> String { String::from("payload") }
fn clean(input: &str) -> String { input.trim().to_string() }
fn normalize(input: &str) -> String { clean(input) }
fn run_shell(cmd: &str) { std::process::Command::new(cmd); }
fn handle() {
    let raw = read_request();
    let safe = normalize(&raw);
    run_shell(&raw);
    run_shell(&safe);
}
"#;

    fn spec<'a>(source: &'a [&'a str], sanitizers: &'a [&'a str]) -> TaintSpec<'a> {
        TaintSpec {
            sources: source,
            sinks: &["run_shell"],
            sanitizers,
        }
    }

    #[test]
    fn a_flow_through_helpers_is_reported_with_its_shortest_witness() {
        let graph = build(Lang::Rust, SANITIZED).unwrap();
        let findings = call_flow_findings(&graph, &spec(&["handle"], &[]));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].sink, "run_shell");
        assert_eq!(findings[0].witness, vec!["handle", "normalize", "dispatch", "run_shell"]);
    }

    #[test]
    fn a_sanitizer_on_every_path_clears_the_finding() {
        let graph = build(Lang::Rust, SANITIZED).unwrap();
        assert!(call_flow_findings(&graph, &spec(&["handle"], &["normalize"])).is_empty());
    }

    #[test]
    fn a_bypass_that_calls_the_sink_directly_is_still_reported() {
        // The call graph does not track values, so a direct call from the source to the sink is a flow that no
        // sanitizer on the other branch can clear. This is the conservative direction.
        let graph = build(Lang::Rust, BYPASS).unwrap();
        let findings = call_flow_findings(&graph, &spec(&["handle"], &["normalize"]));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].witness, vec!["handle", "run_shell"]);
    }

    #[test]
    fn unknown_names_produce_nothing_rather_than_a_finding() {
        let graph = build(Lang::Rust, SANITIZED).unwrap();
        assert!(call_flow_findings(&graph, &spec(&["missing_source"], &[])).is_empty());
        assert_eq!(call_flow_findings(&graph, &spec(&["handle"], &["missing_sanitizer"])).len(), 1);
    }
}
