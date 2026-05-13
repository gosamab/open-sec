//! Calibration CLI for the detection prompt.
//!
//! Usage: `cargo run --bin scan_cli -- path/to/file.ts [--json]`
//!
//! Reads ANTHROPIC_API_KEY from .env / env. Prints findings as a compact table by
//! default, or raw JSON with --json. Intended for iterating on the detection
//! system prompt without the UI in the loop. Not shipped with the app.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use open_sec_lib::config;
use open_sec_lib::providers::anthropic::AnthropicProvider;
use open_sec_lib::scanner::detect::{scan_single_file, DEFAULT_DETECT_MODEL};
use open_sec_lib::scanner::Finding;

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,open_sec_lib=info")),
        )
        .try_init();

    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: scan_cli <file> [--json] [--model <id>]");
        return ExitCode::from(2);
    }

    let mut path: Option<PathBuf> = None;
    let mut json_out = false;
    let mut model = DEFAULT_DETECT_MODEL.to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json_out = true,
            "--model" => {
                i += 1;
                model = args
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| DEFAULT_DETECT_MODEL.to_string());
            }
            other if path.is_none() => path = Some(PathBuf::from(other)),
            other => {
                eprintln!("unexpected arg: {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let Some(path) = path else {
        eprintln!("missing file path");
        return ExitCode::from(2);
    };

    if let Err(e) = run(path, json_out, &model).await {
        eprintln!("error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(path: PathBuf, json_out: bool, model: &str) -> anyhow::Result<()> {
    let source = tokio::fs::read_to_string(&path).await?;
    let api_key = config::load_anthropic_key()?;
    let provider = AnthropicProvider::new(api_key)?;

    let findings = scan_single_file(&path, &source, &provider, model).await?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        render_table(&findings);
    }
    Ok(())
}

fn render_table(findings: &[Finding]) {
    if findings.is_empty() {
        println!("(no findings)");
        return;
    }
    println!("{:<8} {:<10} {:<10} {:<10} title", "kind", "severity", "cwe", "lines");
    println!("{}", "-".repeat(80));
    for f in findings {
        let lines = if f.line_start == f.line_end {
            f.line_start.to_string()
        } else {
            format!("{}-{}", f.line_start, f.line_end)
        };
        println!(
            "{:<8} {:<10} {:<10} {:<10} {}",
            format!("{:?}", f.kind).to_lowercase(),
            format!("{:?}", f.severity).to_lowercase(),
            f.cwe,
            lines,
            f.title,
        );
    }
    println!();
    for f in findings {
        let sev = format!("{:?}", f.severity).to_lowercase();
        println!("[{} / {}] {}", f.cwe, sev, f.title);
        println!("  {}", f.description);
        println!("  data flow: {}", f.data_flow);
        println!();
    }
}
