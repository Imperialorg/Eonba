//! Function blocks carry each definition's name, start line, and exact source text.

use eon_cyber::cpg::{function_blocks, Lang};

const BEFORE: &str = r#"
fn load(path: &str) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn save(path: &str, body: &str) {
    std::fs::write(path, body).unwrap();
}
"#;

#[test]
fn every_function_is_listed_with_its_name_line_and_exact_text() {
    let blocks = function_blocks(Lang::Rust, BEFORE).unwrap();
    let names: Vec<&str> = blocks.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["load", "save"]);
    assert_eq!(blocks[0].start_line, 2);
    assert_eq!(blocks[1].start_line, 6);
    assert!(blocks[1].text.starts_with("fn save("));
    assert!(blocks[1].text.trim_end().ends_with('}'));
}

#[test]
fn nested_functions_are_included() {
    let source = "fn outer() {\n    fn inner() {}\n}\n";
    let names: Vec<String> = function_blocks(Lang::Rust, source).unwrap().into_iter().map(|b| b.name).collect();
    assert_eq!(names, vec!["outer", "inner"]);
}
