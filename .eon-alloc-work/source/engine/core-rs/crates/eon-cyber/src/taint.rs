//! Source-to-sink taint slicing over a data-flow graph (docs/implementation/03, section 2).
//!
//! A finding is a path from a `Source` node to a `Sink` node that never passes through a `Sanitizer`.
//! Sanitizers are removed from traversal, so a flow that is sanitized on one branch but not another
//! is still reported. Every finding carries its witness path so a reviewer can follow it.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Plain,
    Source,
    Sink,
    Sanitizer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub source: String,
    pub sink: String,
    /// Statement labels from source to sink, inclusive.
    pub witness: Vec<String>,
}

#[derive(Debug, Default)]
pub struct DataFlowGraph {
    graph: DiGraph<(String, NodeKind), ()>,
    index: HashMap<String, NodeIndex>,
}

impl DataFlowGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a statement. Adding the same label twice returns the existing node and keeps its first kind.
    pub fn add_node(&mut self, label: &str, kind: NodeKind) -> NodeIndex {
        if let Some(&existing) = self.index.get(label) {
            return existing;
        }
        let idx = self.graph.add_node((label.to_string(), kind));
        self.index.insert(label.to_string(), idx);
        idx
    }

    /// Adds a data-flow edge between two existing statements. Unknown labels are an error.
    pub fn add_flow(&mut self, from: &str, to: &str) -> Result<(), String> {
        let a = *self.index.get(from).ok_or_else(|| format!("unknown statement '{from}'"))?;
        let b = *self.index.get(to).ok_or_else(|| format!("unknown statement '{to}'"))?;
        self.graph.add_edge(a, b, ());
        Ok(())
    }

    /// Every source-to-sink flow that avoids sanitizers, one finding per (source, sink) pair.
    pub fn findings(&self) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut sources: Vec<NodeIndex> = self.nodes_of(NodeKind::Source);
        sources.sort_by_key(|n| self.graph[*n].0.clone());
        let mut sinks: Vec<NodeIndex> = self.nodes_of(NodeKind::Sink);
        sinks.sort_by_key(|n| self.graph[*n].0.clone());
        for &source in &sources {
            let reachable = self.bfs_from(source);
            for &sink in &sinks {
                if let Some(path) = reachable.get(&sink) {
                    out.push(Finding {
                        source: self.graph[source].0.clone(),
                        sink: self.graph[sink].0.clone(),
                        witness: path.iter().map(|n| self.graph[*n].0.clone()).collect(),
                    });
                }
            }
        }
        out
    }

    fn nodes_of(&self, kind: NodeKind) -> Vec<NodeIndex> {
        self.graph
            .node_indices()
            .filter(|n| self.graph[*n].1 == kind)
            .collect()
    }

    /// Breadth-first search from `start` that never enters a sanitizer. Returns the shortest witness to each node.
    fn bfs_from(&self, start: NodeIndex) -> HashMap<NodeIndex, Vec<NodeIndex>> {
        let mut parent: HashMap<NodeIndex, NodeIndex> = HashMap::new();
        let mut seen: HashSet<NodeIndex> = HashSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            for edge in self.graph.edges(node) {
                let next = edge.target();
                if seen.contains(&next) || self.graph[next].1 == NodeKind::Sanitizer {
                    continue;
                }
                seen.insert(next);
                parent.insert(next, node);
                queue.push_back(next);
            }
        }
        seen.into_iter()
            .filter(|n| *n != start)
            .map(|n| {
                let mut path = vec![n];
                let mut cur = n;
                while let Some(&p) = parent.get(&cur) {
                    path.push(p);
                    cur = p;
                }
                path.reverse();
                (n, path)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(nodes: &[(&str, NodeKind)], flows: &[(&str, &str)]) -> DataFlowGraph {
        let mut g = DataFlowGraph::new();
        for (label, kind) in nodes {
            g.add_node(label, *kind);
        }
        for (a, b) in flows {
            g.add_flow(a, b).unwrap();
        }
        g
    }

    #[test]
    fn unsanitized_flow_is_reported_with_its_witness() {
        let g = graph(
            &[("read_input", NodeKind::Source), ("concat", NodeKind::Plain), ("exec", NodeKind::Sink)],
            &[("read_input", "concat"), ("concat", "exec")],
        );
        let findings = g.findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].witness, vec!["read_input", "concat", "exec"]);
    }

    #[test]
    fn sanitizer_on_every_path_clears_the_finding() {
        let g = graph(
            &[("src", NodeKind::Source), ("escape", NodeKind::Sanitizer), ("sink", NodeKind::Sink)],
            &[("src", "escape"), ("escape", "sink")],
        );
        assert!(g.findings().is_empty());
    }

    #[test]
    fn a_bypass_branch_around_the_sanitizer_is_still_reported() {
        let g = graph(
            &[
                ("src", NodeKind::Source),
                ("escape", NodeKind::Sanitizer),
                ("bypass", NodeKind::Plain),
                ("sink", NodeKind::Sink),
            ],
            &[("src", "escape"), ("escape", "sink"), ("src", "bypass"), ("bypass", "sink")],
        );
        let findings = g.findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].witness, vec!["src", "bypass", "sink"]);
    }

    #[test]
    fn disconnected_source_and_sink_produce_nothing() {
        let g = graph(&[("src", NodeKind::Source), ("sink", NodeKind::Sink)], &[]);
        assert!(g.findings().is_empty());
    }

    #[test]
    fn cycles_do_not_loop_forever() {
        let g = graph(
            &[("src", NodeKind::Source), ("a", NodeKind::Plain), ("b", NodeKind::Plain), ("sink", NodeKind::Sink)],
            &[("src", "a"), ("a", "b"), ("b", "a"), ("b", "sink")],
        );
        assert_eq!(g.findings().len(), 1);
    }

    #[test]
    fn unknown_statement_in_a_flow_is_rejected() {
        let mut g = DataFlowGraph::new();
        g.add_node("a", NodeKind::Plain);
        assert!(g.add_flow("a", "missing").is_err());
    }
}
