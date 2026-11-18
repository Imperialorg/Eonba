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

#[test]
fn a_module_qualifier_picks_the_named_module_when_two_files_define_the_name() {
    let caller = "fn run() { store::save(1) }\n";
    let files = [
        SourceFile { path: "run.rs", lang: Lang::Rust, source: caller },
        SourceFile { path: "store.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
        SourceFile { path: "audit.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.contains(&("run.rs::run".into(), "store.rs::save".into())));
    assert!(!graph.edges.contains(&("run.rs::run".into(), "audit.rs::save".into())));
    assert!(graph.ambiguous.is_empty());
}

#[test]
fn a_type_qualified_call_still_resolves_inside_its_own_file() {
    let files = [SourceFile {
        path: "thing.rs",
        lang: Lang::Rust,
        source: "struct Thing;\nimpl Thing { fn new() -> Thing { Thing } }\nfn build() { Thing::new(); }\n",
    }];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.contains(&("thing.rs::build".into(), "thing.rs::new".into())));
}

#[test]
fn a_python_module_attribute_resolves_to_the_named_module() {
    let files = [
        SourceFile { path: "app.py", lang: Lang::Python, source: "import store\ndef run():\n    store.save(1)\n" },
        SourceFile { path: "store.py", lang: Lang::Python, source: "def save(x):\n    pass\n" },
        SourceFile { path: "audit.py", lang: Lang::Python, source: "def save(x):\n    pass\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.contains(&("app.py::run".into(), "store.py::save".into())));
}

#[test]
fn a_qualifier_naming_a_module_outside_the_project_stays_unresolved() {
    let files = [
        SourceFile { path: "run.rs", lang: Lang::Rust, source: "fn run() { serde_json::save(1) }\n" },
        SourceFile { path: "store.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.is_empty());
    assert!(graph.unresolved["run.rs::run"].contains("save"));
}

#[test]
fn a_call_through_a_use_import_resolves_to_the_imported_module() {
    let files = [
        SourceFile { path: "run.rs", lang: Lang::Rust, source: "use store::save;\nfn run() { save(1) }\n" },
        SourceFile { path: "store.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
        SourceFile { path: "audit.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.contains(&("run.rs::run".into(), "store.rs::save".into())));
    assert!(graph.ambiguous.is_empty());
}

#[test]
fn an_aliased_use_import_resolves_the_original_name() {
    let files = [
        SourceFile { path: "run.rs", lang: Lang::Rust, source: "use store::save as persist;\nfn run() { persist(1) }\n" },
        SourceFile { path: "store.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
        SourceFile { path: "audit.rs", lang: Lang::Rust, source: "pub fn save(x: i32) {}\n" },
    ];
    let graph = build_project(&files).unwrap();
    assert!(graph.edges.contains(&("run.rs::run".into(), "store.rs::save".into())));
}
