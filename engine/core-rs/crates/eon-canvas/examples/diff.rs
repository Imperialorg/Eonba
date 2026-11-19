//! Opens a read-only function diff: `cargo run -p eon-canvas --example diff -- before.rs after.rs`.
//! Without paths it shows the bundled Rust example.

use std::{error::Error, fs, path::Path};

use eon_cyber::cpg::{FunctionBlock, Lang, function_blocks};

const BEFORE: &str = "fn load(path: &str) -> String { std::fs::read_to_string(path).unwrap() }
fn save(path: &str, body: &str) { std::fs::write(path, body).unwrap() }
fn drop_cache() {}
";

const AFTER: &str = "fn load(path: &str) -> String { std::fs::read_to_string(path).unwrap() }
fn save(path: &str, body: &str) { std::fs::write(path, body.trim()).unwrap() }
fn audit(event: &str) { println!(\"{event}\"); }
";

fn read_blocks(path: &Path) -> Result<Vec<FunctionBlock>, Box<dyn Error>> {
    let lang = match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => Lang::Rust,
        Some("py") => Lang::Python,
        _ => return Err(format!("{}: expected a .rs or .py source file", path.display()).into()),
    };
    let source = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(function_blocks(lang, &source).map_err(|e| format!("{}: {e}", path.display()))?)
}

fn main() -> Result<(), Box<dyn Error>> {
    let paths: Vec<_> = std::env::args_os().skip(1).collect();
    let (old, new) = match paths.as_slice() {
        [] => {
            eprintln!("Showing the bundled example. Pass two source paths to review files.");
            (
                function_blocks(Lang::Rust, BEFORE)?,
                function_blocks(Lang::Rust, AFTER)?,
            )
        }
        [old, new] => (read_blocks(Path::new(old))?, read_blocks(Path::new(new))?),
        _ => return Err("usage: diff [<before.rs|py> <after.rs|py>]".into()),
    };
    Ok(eon_canvas::view::run_diff(old, new)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_named_source_or_reports_the_path_before_opening_a_window() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let blocks = read_blocks(&root.join("examples/diff.rs")).unwrap();
        assert!(
            blocks
                .iter()
                .any(|b| b.name == "read_blocks" && b.text.contains("fs::read_to_string"))
        );
        assert!(
            read_blocks(&root.join("absent.py"))
                .unwrap_err()
                .to_string()
                .contains("absent.py")
        );
        assert!(
            read_blocks(&root.join("Cargo.toml"))
                .unwrap_err()
                .to_string()
                .contains("expected a .rs or .py")
        );
    }
}
