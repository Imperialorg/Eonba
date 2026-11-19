//! Opens the structural diff of two Rust sources. Run with `cargo run -p eon-canvas --example diff`.

const BEFORE: &str = "fn load(path: &str) -> String { std::fs::read_to_string(path).unwrap() }
fn save(path: &str, body: &str) { std::fs::write(path, body).unwrap() }
fn drop_cache() {}
";

const AFTER: &str = "fn load(path: &str) -> String { std::fs::read_to_string(path).unwrap() }
fn save(path: &str, body: &str) { std::fs::write(path, body.trim()).unwrap() }
fn audit(event: &str) { println!(\"{event}\"); }
";

fn main() -> eframe::Result {
    let old = eon_cyber::cpg::function_blocks(eon_cyber::cpg::Lang::Rust, BEFORE).expect("the sample parses");
    let new = eon_cyber::cpg::function_blocks(eon_cyber::cpg::Lang::Rust, AFTER).expect("the sample parses");
    eon_canvas::view::run_diff(old, new)
}
