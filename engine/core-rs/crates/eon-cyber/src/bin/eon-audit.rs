//! Read-only, local evidence reports. Never print submitted source or execute it.

use eon_cyber::cpg::Lang;
use eon_cyber::defense::{Assessment, MAX_SOURCE_BYTES, taint_auditor, telemetry};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::process::ExitCode;

const USAGE: &str = "Usage: eon-audit rust|python [--etw] < source-file\n       eon-audit self-io";

fn source_report(language: Lang, input: impl Read) -> Result<Value, &'static str> {
    let mut bytes = Vec::new();
    input
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read stdin")?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err("source exceeds the 16 KiB review limit");
    }
    let src = std::str::from_utf8(&bytes).map_err(|_| "expected UTF-8 source")?;
    let report = taint_auditor::review_source(language, src);
    let checks = report.checks.map(|check| {
        let cwe = check.rule.cwe();
        match check.evidence {
            Assessment::Observed(range) => json!({
                "cwe": cwe, "status": "observed_syntax", "start_byte": range.start, "end_byte": range.end,
                "recommendation": check.rule.recommendation(),
            }),
            Assessment::Unverified(gap) => json!({ "cwe": cwe, "status": "unverified", "reason": format!("{gap:?}") }),
        }
    });
    Ok(json!({ "source": "stdin", "language": format!("{language:?}"), "checks": checks }))
}

fn self_io() -> Value {
    match telemetry::sample_process_io() {
        Assessment::Observed(sample) => {
            let bytes = sample.bytes();
            json!({ "source": "proc_self_io", "status": "observed", "read_bytes": bytes.read, "write_bytes": bytes.written })
        }
        Assessment::Unverified(gap) => {
            json!({ "source": "proc_self_io", "status": "unverified", "reason": format!("{gap:?}") })
        }
    }
}

fn run() -> Result<(), &'static str> {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next();
    let option = args.next();
    if args.next().is_some() || option.as_ref().is_some_and(|o| o != "--etw") {
        return Err(USAGE);
    }
    let mut report = match mode.as_deref().and_then(|s| s.to_str()) {
        Some("rust") => source_report(Lang::Rust, io::stdin().lock())?,
        Some("python") => source_report(Lang::Python, io::stdin().lock())?,
        Some("self-io") if option.is_none() => self_io(),
        _ => return Err(USAGE),
    };
    if option.is_some() {
        report["etw"] = match telemetry::submit_review_completion() {
            Assessment::Observed(()) => json!({ "status": "submitted" }),
            Assessment::Unverified(gap) => {
                json!({ "status": "unverified", "reason": format!("{gap:?}") })
            }
        };
    }
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &report).map_err(|_| "cannot write report")?;
    writeln!(stdout).map_err(|_| "cannot write report")
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}
