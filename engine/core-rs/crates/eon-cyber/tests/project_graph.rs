//! Cross-file call resolution over a project. A call resolves only when its name has exactly one definition in the project.

use eon_cyber::cpg::{build_project, Lang, SourceFile};

const HANDLER: &str = "use crate::store;\nfn handle(input: &str) -> String {\n    store::save(input)\n}\n";
const STORE: &str = "pub fn save(data: &str) -> String {\n    format_record(data)\n}\nfn format_record(d: &str) -> String { d.to_owned() }\n";

#[test]
fn a_call_into_another_file_resolves_to_that_files_function() {
    let files = [
        SourceFile { path: "handler.rs", lang: Lang::Rust, source: HANDLER },
        SourceFile { path: "store.rs", lang: Lang::Rust, source: STORE },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.contains(&("handler.rs::handle".into(), "store.rs::save".into())));
    assert!(graph.reaches("handler.rs::handle", "store.rs::format_record"));
}

#[test]
fn a_name_defined_in_two_files_is_reported_as_ambiguous_not_guessed() {
    let caller = "fn run() { helper() }\n";
    let files = [
        SourceFile { path: "a.rs", lang: Lang::Rust, source: caller },
        SourceFile { path: "b.rs", lang: Lang::Rust, source: "fn helper() {}\n" },
        SourceFile { path: "c.rs", lang: Lang::Rust, source: "fn helper() {}\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.iter().all(|(from, _)| from != "a.rs::run"));
    assert_eq!(graph.ambiguous["a.rs::run"].iter().collect::<Vec<_>>(), vec!["helper"]);
}

#[test]
fn a_call_to_a_function_nobody_defines_stays_unresolved() {
    let files = [SourceFile { path: "a.rs", lang: Lang::Rust, source: "fn run() { external_crate_call() }\n" }];
    let graph = build_project(&files).unwrap();
    assert!(graph.unresolved["a.rs::run"].contains("external_crate_call"));
}

#[test]
fn a_python_name_does_not_resolve_to_a_rust_definition() {
    let files = [
        SourceFile { path: "a.py", lang: Lang::Python, source: "def run():\n    helper()\n" },
        SourceFile { path: "b.rs", lang: Lang::Rust, source: "fn helper() {}\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.unresolved["a.py::run"].contains("helper"));
    assert!(graph.edges.is_empty());
}
