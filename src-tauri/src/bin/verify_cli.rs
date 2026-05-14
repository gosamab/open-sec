//! Calibration CLI for the verification pass.
//!
//! Two modes:
//!   1. End-to-end (default): detect on a file, then verify each finding.
//!        cargo run --bin verify_cli -- <file>
//!        cargo run --bin verify_cli -- <file> --root <dir>
//!   2. Hand-fed finding: skip detection, feed the JSON finding directly.
//!        cargo run --bin verify_cli -- <file> --finding <finding.json>
//!
//! Useful for stress-testing the verifier on adversarial "looks vulnerable
//! but isn't" cases that real detection wouldn't naturally produce.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use open_sec_lib::config;
use open_sec_lib::providers::anthropic::AnthropicProvider;
use open_sec_lib::providers::Provider;
use open_sec_lib::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use open_sec_lib::scanner::verify::{
    verify_many, DEFAULT_VERIFY_CONCURRENCY, DEFAULT_VERIFY_MODEL,
};
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
        eprintln!(
            "usage: verify_cli <file> [--root <dir>] [--finding <json>] \
             [--detect-model ID] [--verify-model ID] [--concurrency N] [--json]"
        );
        return ExitCode::from(2);
    }

    let mut path: Option<PathBuf> = None;
    let mut root: Option<PathBuf> = None;
    let mut finding_path: Option<PathBuf> = None;
    let mut detect_model = DEFAULT_DETECT_MODEL.to_string();
    let mut verify_model = DEFAULT_VERIFY_MODEL.to_string();
    let mut concurrency = DEFAULT_VERIFY_CONCURRENCY;
    let mut json_out = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                root = args.get(i).map(PathBuf::from);
            }
            "--finding" => {
                i += 1;
                finding_path = args.get(i).map(PathBuf::from);
            }
            "--detect-model" => {
                i += 1;
                detect_model = args.get(i).cloned().unwrap_or(detect_model);
            }
            "--verify-model" => {
                i += 1;
                verify_model = args.get(i).cloned().unwrap_or(verify_model);
            }
            "--concurrency" => {
                i += 1;
                concurrency = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(DEFAULT_VERIFY_CONCURRENCY);
            }
            "--json" => json_out = true,
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

    if let Err(e) = run(
        path,
        root,
        finding_path,
        &detect_model,
        &verify_model,
        concurrency,
        json_out,
    )
    .await
    {
        eprintln!("error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(
    path: PathBuf,
    root: Option<PathBuf>,
    finding_path: Option<PathBuf>,
    detect_model: &str,
    verify_model: &str,
    concurrency: usize,
    json_out: bool,
) -> anyhow::Result<()> {
    let scan_root = root
        .or_else(|| path.parent().map(|p| p.to_path_buf()))
        .ok_or_else(|| anyhow::anyhow!("cannot derive scan root"))?;

    let api_key = config::load_anthropic_key()?;
    let provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(api_key)?);

    let findings: Vec<Finding> = if let Some(fp) = finding_path {
        let raw = tokio::fs::read_to_string(&fp).await?;
        // Accept either a single finding object or {"findings": [...]}.
        if let Ok(env) = serde_json::from_str::<FindingsEnvelope>(&raw) {
            env.findings
        } else {
            let one: Finding = serde_json::from_str(&raw)?;
            vec![one]
        }
    } else {
        let source = tokio::fs::read_to_string(&path).await?;
        eprintln!("detect: scanning {}", path.display());
        scan_with_tools(&path, &scan_root, &source, provider.as_ref(), detect_model).await?
    };

    eprintln!("detect produced {} finding(s)", findings.len());
    if findings.is_empty() {
        return Ok(());
    }

    eprintln!(
        "verify: {} finding(s) using {} (concurrency={})",
        findings.len(),
        verify_model,
        concurrency
    );
    let verified =
        verify_many(findings, scan_root, provider, verify_model, concurrency).await;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&verified)?);
        return Ok(());
    }
    render(&verified);
    Ok(())
}

#[derive(serde::Deserialize)]
struct FindingsEnvelope {
    findings: Vec<Finding>,
}

fn render(verified: &[open_sec_lib::scanner::verify::VerifiedFinding]) {
    let (mut kept, mut dropped, mut hardening, mut errored) = (0usize, 0usize, 0usize, 0usize);
    for v in verified {
        match v.verdict.as_ref() {
            None if matches!(v.finding.kind, open_sec_lib::scanner::FindingKind::Hardening) => {
                hardening += 1
            }
            None => errored += 1,
            Some(verdict) if verdict.keep() => kept += 1,
            Some(_) => dropped += 1,
        }
    }

    println!(
        "verify funnel: kept={kept} dropped={dropped} hardening_passthrough={hardening} errored={errored}",
    );
    println!();
    for v in verified {
        let cwe = &v.finding.cwe;
        let title = &v.finding.title;
        let file = &v.finding.file;
        let lines = if v.finding.line_start == v.finding.line_end {
            v.finding.line_start.to_string()
        } else {
            format!("{}-{}", v.finding.line_start, v.finding.line_end)
        };
        match v.verdict.as_ref() {
            None if matches!(v.finding.kind, open_sec_lib::scanner::FindingKind::Hardening) => {
                println!("[hardening] {cwe} {file}:{lines}  {title}");
            }
            None => println!("[errored]   {cwe} {file}:{lines}  {title}"),
            Some(verdict) => {
                let tag = if verdict.keep() { "KEEP" } else { "DROP" };
                println!("[{tag}]      {cwe} {file}:{lines}  {title}");
                println!(
                    "             reachable={} untrusted={}",
                    verdict.is_reachable, verdict.source_is_untrusted
                );
                if let Some(ex) = &verdict.concrete_exploit {
                    let kind = format!("{:?}", ex.kind).to_lowercase();
                    println!("             exploit ({}): {}", kind, ex.expected_effect);
                    if let Some(req) = &ex.request {
                        println!("                {} {}", req.method, req.path);
                    }
                    println!("                payload: {}", ex.payload);
                }
                println!("             reasoning: {}", verdict.reasoning);
            }
        }
        println!();
    }
}
