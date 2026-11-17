use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, cmd, dir] if cmd == "status" => match eon_cli::read_status(&PathBuf::from(dir)) {
            Ok(status) => {
                print!("{}", eon_cli::render_text(&status));
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("eon: {err}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: eon status <journal-dir>");
            ExitCode::from(2)
        }
    }
}
