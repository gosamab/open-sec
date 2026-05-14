//! Calibration CLI for the triage pass.
//!
//! Usage:
//!   cargo run --bin triage_cli -- <dir>
//!   cargo run --bin triage_cli -- <dir> --concurrency 8 --model claude-haiku-4-5
//!   cargo run --bin triage_cli -- <dir> --json
//!
//! Walks the directory with `ingest`, runs `triage_many` against the real
//! Anthropic API, and prints the bucket decisions + skip funnel. Intended for
//! iterating on the triage prompt against `fixtures/triage/`.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use open_sec_lib::config;
use open_sec_lib::providers::anthropic::AnthropicProvider;
use open_sec_lib::providers::Provider;
use open_sec_lib::scanner::ingest::{self, SkipReason, WalkResult};
use open_sec_lib::scanner::triage::{
    self, Priority, TriagedFile, DEFAULT_TRIAGE_CONCURRENCY, DEFAULT_TRIAGE_MODEL,
};

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
        eprintln!("usage: triage_cli <dir> [--concurrency N] [--model ID] [--json]");
        return ExitCode::from(2);
    }

    let mut root: Option<PathBuf> = None;
    let mut concurrency = DEFAULT_TRIAGE_CONCURRENCY;
    let mut model = DEFAULT_TRIAGE_MODEL.to_string();
    let mut json_out = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--concurrency" => {
                i += 1;
                concurrency = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(DEFAULT_TRIAGE_CONCURRENCY);
            }
            "--model" => {
                i += 1;
                model = args.get(i).cloned().unwrap_or(model);
            }
            "--json" => json_out = true,
            other if root.is_none() => root = Some(PathBuf::from(other)),
            other => {
                eprintln!("unexpected arg: {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }

    let Some(root) = root else {
        eprintln!("missing directory");
        return ExitCode::from(2);
    };

    if let Err(e) = run(root, concurrency, &model, json_out).await {
        eprintln!("error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(root: PathBuf, concurrency: usize, model: &str, json_out: bool) -> anyhow::Result<()> {
    let walk = ingest::walk(&root)?;
    eprintln!(
        "ingest: {} candidate(s), {} skipped",
        walk.candidates.len(),
        walk.skipped.len()
    );

    if walk.candidates.is_empty() {
        print_skipped(&walk);
        return Ok(());
    }

    let api_key = config::load_anthropic_key()?;
    let provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(api_key)?);

    let triaged =
        triage::triage_many(walk.candidates.clone(), provider, model, concurrency).await;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&triaged)?);
        return Ok(());
    }

    render_table(&triaged);
    render_summary(&triaged, &walk);
    Ok(())
}

fn render_table(triaged: &[TriagedFile]) {
    if triaged.is_empty() {
        println!("(no triage results)");
        return;
    }
    println!("{:<8} {:<40} reason", "prio", "file");
    println!("{}", "-".repeat(96));
    for t in triaged {
        let prio = format!("{:?}", t.result.priority).to_lowercase();
        let file = truncate(&t.candidate.rel_path, 40);
        println!("{:<8} {:<40} {}", prio, file, t.result.reason);
    }
    println!();
}

fn render_summary(triaged: &[TriagedFile], walk: &WalkResult) {
    let (mut h, mut n, mut l, mut s) = (0usize, 0usize, 0usize, 0usize);
    for t in triaged {
        match t.result.priority {
            Priority::High => h += 1,
            Priority::Normal => n += 1,
            Priority::Low => l += 1,
            Priority::Skip => s += 1,
        }
    }
    println!(
        "triage funnel: high={h} normal={n} low={l} skip={s} (errored={})",
        walk.candidates.len() - triaged.len()
    );
    print_skipped(walk);
}

fn print_skipped(walk: &WalkResult) {
    if walk.skipped.is_empty() {
        return;
    }
    let (mut excluded, mut too_large, mut binary, mut minified, mut io) = (0, 0, 0, 0, 0);
    for s in &walk.skipped {
        match s.reason {
            SkipReason::ExcludedDir => excluded += 1,
            SkipReason::UnsupportedExt => {}
            SkipReason::TooLarge => too_large += 1,
            SkipReason::Binary => binary += 1,
            SkipReason::Minified => minified += 1,
            SkipReason::IoError => io += 1,
        }
    }
    println!(
        "pre-triage skips: excluded_dir={excluded} too_large={too_large} binary={binary} minified={minified} io={io}",
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let cut = max.saturating_sub(1);
        format!("…{}", &s[s.len().saturating_sub(cut)..])
    }
}
