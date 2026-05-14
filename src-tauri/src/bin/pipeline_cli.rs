//! Full-pipeline CLI: walks a directory, runs triage → detect → verify →
//! patch end-to-end, and prints a per-stage summary plus the kept findings
//! and proposed diffs.
//!
//! Usage:
//!   cargo run --bin pipeline_cli -- <dir>
//!   cargo run --bin pipeline_cli -- <dir> --json
//!
//! `--json` emits the full `ScanResult` to stdout (UI consumption); the
//! per-stage funnel summaries always go to stderr so they don't pollute
//! JSON output.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use open_sec_lib::config;
use open_sec_lib::providers::anthropic::AnthropicProvider;
use open_sec_lib::providers::Provider;
use open_sec_lib::scanner::ingest::SkipReason;
use open_sec_lib::scanner::orchestrate::{run_scan, ScanConfig, ScanResult};
use open_sec_lib::scanner::patch::{Located, Patch};
use open_sec_lib::scanner::triage::Priority;
use open_sec_lib::scanner::verify::VerifiedFinding;
use open_sec_lib::scanner::FindingKind;

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,open_sec_lib=info")),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: pipeline_cli <dir> [--json]");
        return ExitCode::from(2);
    }

    let mut root: Option<PathBuf> = None;
    let mut json_out = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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

    if let Err(e) = run(root, json_out).await {
        eprintln!("error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(root: PathBuf, json_out: bool) -> anyhow::Result<()> {
    let api_key = config::load_anthropic_key()?;
    let provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(api_key)?);
    let config = ScanConfig::default();

    eprintln!(">>> scanning {}", root.display());
    let result = run_scan(root, provider, &config, None, None).await?;

    print_summary(&result);

    if json_out {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    print_kept(&result);
    print_patches(&result.patches);
    Ok(())
}

fn print_summary(r: &ScanResult) {
    // Ingest funnel.
    let (mut excluded, mut too_large, mut binary, mut minified, mut io) = (0, 0, 0, 0, 0);
    for s in &r.ingest.skipped {
        match s.reason {
            SkipReason::ExcludedDir => excluded += 1,
            SkipReason::UnsupportedExt => {}
            SkipReason::TooLarge => too_large += 1,
            SkipReason::Binary => binary += 1,
            SkipReason::Minified => minified += 1,
            SkipReason::IoError => io += 1,
        }
    }
    eprintln!(
        "  ingest:  {} candidate(s); skipped excluded={excluded} too_large={too_large} binary={binary} minified={minified} io={io}",
        r.ingest.candidates.len()
    );

    // Triage funnel.
    let (mut h, mut n, mut l, mut s) = (0usize, 0usize, 0usize, 0usize);
    for t in &r.triaged {
        match t.result.priority {
            Priority::High => h += 1,
            Priority::Normal => n += 1,
            Priority::Low => l += 1,
            Priority::Skip => s += 1,
        }
    }
    eprintln!("  triage:  high={h} normal={n} low={l} skip={s}");

    // Detect.
    let total_findings: usize = r.findings_by_file.iter().map(|ff| ff.findings.len()).sum();
    let files_with_findings = r
        .findings_by_file
        .iter()
        .filter(|ff| !ff.findings.is_empty())
        .count();
    eprintln!(
        "  detect:  {total_findings} finding(s) across {} file(s) ({} scanned, {} had findings)",
        files_with_findings,
        r.findings_by_file.len(),
        files_with_findings
    );

    // Verify funnel.
    let (mut kept, mut dropped, mut hardening, mut errored) = (0, 0, 0, 0);
    for v in &r.verified {
        match v.verdict.as_ref() {
            None if matches!(v.finding.kind, FindingKind::Hardening) => hardening += 1,
            None => errored += 1,
            Some(verdict) if verdict.keep() => kept += 1,
            Some(_) => dropped += 1,
        }
    }
    eprintln!("  verify:  kept={kept} dropped={dropped} hardening_passthrough={hardening} errored={errored}");

    // Patch funnel.
    let (mut exact, mut fuzzy, mut not_found) = (0, 0, 0);
    for p in &r.patches {
        match p.located {
            Located::Exact { .. } => exact += 1,
            Located::Fuzzy { .. } => fuzzy += 1,
            Located::NotFound => not_found += 1,
        }
    }
    eprintln!(
        "  patch:   {} proposal(s) [exact={exact} fuzzy={fuzzy} not_found={not_found}]",
        r.patches.len()
    );

    let u = &r.usage;
    let t = &u.total;
    eprintln!(
        "  tokens:  in={} out={} cache_read={} cache_create={}  (triage in/out={}/{} · detect={}/{} · verify={}/{} · patch={}/{})",
        t.input_tokens,
        t.output_tokens,
        t.cache_read_input_tokens,
        t.cache_creation_input_tokens,
        u.triage.input_tokens,
        u.triage.output_tokens,
        u.detect.input_tokens,
        u.detect.output_tokens,
        u.verify.input_tokens,
        u.verify.output_tokens,
        u.patch.input_tokens,
        u.patch.output_tokens,
    );
    eprintln!();
}

fn print_kept(r: &ScanResult) {
    let displayed: Vec<&VerifiedFinding> = r
        .verified
        .iter()
        .filter(|v| {
            matches!(v.finding.kind, FindingKind::Hardening)
                || v.verdict.as_ref().map(|x| x.keep()).unwrap_or(false)
        })
        .collect();
    if displayed.is_empty() {
        println!("(no findings retained)");
        return;
    }
    println!("=== {} finding(s) ===", displayed.len());
    println!();
    for v in &displayed {
        let f = &v.finding;
        let tag = match (&f.kind, v.verdict.as_ref()) {
            (FindingKind::Hardening, _) => "HARDENING",
            (_, Some(_)) => "VULN",
            _ => "?",
        };
        let lines = if f.line_start == f.line_end {
            f.line_start.to_string()
        } else {
            format!("{}-{}", f.line_start, f.line_end)
        };
        println!(
            "[{tag}] {cwe} {severity:?} {file}:{lines}  {title}",
            cwe = f.cwe,
            severity = f.severity,
            file = f.file,
            title = f.title,
        );
        println!("  {}", f.description);
        if let Some(verdict) = &v.verdict {
            if let Some(ex) = &verdict.concrete_exploit {
                let kind = format!("{:?}", ex.kind).to_lowercase();
                println!("  exploit ({}): {}", kind, ex.expected_effect);
                if let Some(req) = &ex.request {
                    println!("    {} {}", req.method, req.path);
                }
                println!("    payload: {}", ex.payload);
            }
        }
        println!();
    }
}

fn print_patches(patches: &[Patch]) {
    if patches.is_empty() {
        return;
    }
    println!("=== {} patch proposal(s) ===", patches.len());
    println!();
    for p in patches {
        let mode = match p.located {
            Located::Exact { .. } => "exact",
            Located::Fuzzy { .. } => "fuzzy",
            Located::NotFound => "not-found",
        };
        println!(
            "[{mode}] {file} (anchor L{line})",
            file = p.proposal.file,
            line = p.proposal.anchor_line
        );
        println!("  {}", p.proposal.explanation);
        println!();
        if let Some(diff) = &p.diff {
            for line in diff.lines() {
                println!("    {line}");
            }
        } else {
            println!("    (no diff — old_block not located; raw blocks below)");
            println!("    --- old_block ---");
            for line in p.proposal.old_block.lines() {
                println!("    | {line}");
            }
            println!("    --- new_block ---");
            for line in p.proposal.new_block.lines() {
                println!("    | {line}");
            }
        }
        println!();
    }
}
