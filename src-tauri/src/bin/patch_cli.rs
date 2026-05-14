//! Calibration CLI for the patch pass.
//!
//! Runs detect → verify → patch end-to-end on a single file and prints the
//! resulting proposals (diffs when located, raw blocks when not).
//!
//! Usage:
//!   cargo run --bin patch_cli -- <file>
//!   cargo run --bin patch_cli -- <file> --root <dir> --json
//!   cargo run --bin patch_cli -- <file> --skip-verify   (treat all detect vulns as kept)

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use open_sec_lib::config;
use open_sec_lib::providers::anthropic::AnthropicProvider;
use open_sec_lib::providers::Provider;
use open_sec_lib::scanner::detect::{scan_with_tools, DEFAULT_DETECT_MODEL};
use open_sec_lib::scanner::patch::{
    propose_many, Located, Patch, DEFAULT_PATCH_CONCURRENCY, DEFAULT_PATCH_MODEL,
};
use open_sec_lib::scanner::verify::{
    verify_many, Exploit, ExploitKind, Verdict, VerifiedFinding, DEFAULT_VERIFY_MODEL,
};
use open_sec_lib::scanner::FindingKind;

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
            "usage: patch_cli <file> [--root <dir>] [--skip-verify] \
             [--detect-model ID] [--verify-model ID] [--patch-model ID] \
             [--concurrency N] [--json]"
        );
        return ExitCode::from(2);
    }

    let mut path: Option<PathBuf> = None;
    let mut root: Option<PathBuf> = None;
    let mut skip_verify = false;
    let mut detect_model = DEFAULT_DETECT_MODEL.to_string();
    let mut verify_model = DEFAULT_VERIFY_MODEL.to_string();
    let mut patch_model = DEFAULT_PATCH_MODEL.to_string();
    let mut concurrency = DEFAULT_PATCH_CONCURRENCY;
    let mut json_out = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                root = args.get(i).map(PathBuf::from);
            }
            "--skip-verify" => skip_verify = true,
            "--detect-model" => {
                i += 1;
                detect_model = args.get(i).cloned().unwrap_or(detect_model);
            }
            "--verify-model" => {
                i += 1;
                verify_model = args.get(i).cloned().unwrap_or(verify_model);
            }
            "--patch-model" => {
                i += 1;
                patch_model = args.get(i).cloned().unwrap_or(patch_model);
            }
            "--concurrency" => {
                i += 1;
                concurrency = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(DEFAULT_PATCH_CONCURRENCY);
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
        skip_verify,
        &detect_model,
        &verify_model,
        &patch_model,
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

#[allow(clippy::too_many_arguments)]
async fn run(
    path: PathBuf,
    root: Option<PathBuf>,
    skip_verify: bool,
    detect_model: &str,
    verify_model: &str,
    patch_model: &str,
    concurrency: usize,
    json_out: bool,
) -> anyhow::Result<()> {
    let scan_root = root
        .or_else(|| path.parent().map(|p| p.to_path_buf()))
        .ok_or_else(|| anyhow::anyhow!("cannot derive scan root"))?;

    let api_key = config::load_anthropic_key()?;
    let provider: Arc<dyn Provider> = Arc::new(AnthropicProvider::new(api_key)?);

    let source = tokio::fs::read_to_string(&path).await?;
    eprintln!("detect: scanning {}", path.display());
    let findings = scan_with_tools(&path, &scan_root, &source, provider.as_ref(), detect_model)
        .await?;
    eprintln!("detect produced {} finding(s)", findings.len());

    let verified: Vec<VerifiedFinding> = if skip_verify {
        findings
            .into_iter()
            .map(|f| VerifiedFinding {
                verdict: match f.kind {
                    FindingKind::Vuln => Some(Verdict {
                        is_reachable: true,
                        source_is_untrusted: true,
                        concrete_exploit: Some(Exploit {
                            kind: ExploitKind::Other,
                            request: None,
                            payload: "(skipped)".into(),
                            expected_effect: "(skipped)".into(),
                        }),
                        reasoning: "verifier skipped via --skip-verify".into(),
                    }),
                    FindingKind::Hardening => None,
                },
                finding: f,
            })
            .collect()
    } else {
        eprintln!("verify: {} finding(s)", findings.len());
        verify_many(findings, scan_root.clone(), provider.clone(), verify_model, 2).await
    };

    let patchable = verified
        .iter()
        .filter(|v| {
            matches!(v.finding.kind, FindingKind::Hardening)
                || v.verdict.as_ref().map(|v| v.keep()).unwrap_or(false)
        })
        .count();
    eprintln!(
        "patch: {patchable} finding(s) using {patch_model} (concurrency={concurrency})"
    );
    if patchable == 0 {
        return Ok(());
    }

    let patches = propose_many(verified, scan_root, provider, patch_model, concurrency).await;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&patches)?);
        return Ok(());
    }
    render(&patches);
    Ok(())
}

fn render(patches: &[Patch]) {
    println!("patch funnel: {} proposal(s)", patches.len());
    println!();
    for p in patches {
        let lines = match &p.located {
            Located::Exact {
                line_start,
                line_end,
                ..
            }
            | Located::Fuzzy {
                line_start,
                line_end,
                ..
            } => {
                if line_start == line_end {
                    format!("L{line_start}")
                } else {
                    format!("L{line_start}-{line_end}")
                }
            }
            Located::NotFound => format!("L{} (anchor only — old_block not located)", p.proposal.anchor_line),
        };
        let mode = match &p.located {
            Located::Exact { .. } => "exact",
            Located::Fuzzy { .. } => "fuzzy",
            Located::NotFound => "not-found",
        };
        println!(
            "[{mode}] {file} {lines}",
            file = p.proposal.file,
            lines = lines
        );
        println!("        {}", p.proposal.explanation);
        println!();
        if let Some(diff) = &p.diff {
            for line in diff.lines() {
                println!("    {line}");
            }
        } else {
            println!("    (no diff — proposed replacement below)");
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
