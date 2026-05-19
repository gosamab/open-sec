// End-to-end smoke test: runs the full orchestrator (ingest → triage → detect
// → verify → patch) against fixtures/vulnerable to confirm all four submit_*
// tools fire correctly with the real Anthropic API.
//
// Run: cd src-tauri && cargo run --example pipeline_smoke --release
// Optional arg: a different scan root (defaults to ../fixtures/vulnerable).
// API key: ANTHROPIC_API_KEY env var, or the on-disk file the app writes
// under ~/Library/Application Support/<bundle>/anthropic-api-key.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use open_sec_lib::providers::anthropic::AnthropicProvider;
use open_sec_lib::providers::rate_limit::RateLimitObserver;
use open_sec_lib::providers::Provider;
use open_sec_lib::scanner::orchestrate::{run_scan, ScanConfig, ScanEvent};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

fn load_api_key() -> Result<String> {
    let _ = dotenvy::dotenv();
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let home = std::env::var("HOME").context("HOME unset")?;
    for bundle in ["com.opensec.app", "com.oazab.open-sec"] {
        let p = PathBuf::from(&home)
            .join("Library/Application Support")
            .join(bundle)
            .join("anthropic-api-key");
        if let Ok(s) = std::fs::read_to_string(&p) {
            let t = s.trim();
            if !t.is_empty() {
                return Ok(t.to_string());
            }
        }
    }
    Err(anyhow!("no API key in env or on disk"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,open_sec_lib=info")),
        )
        .try_init();

    let root: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../fixtures/vulnerable"));
    let root = root.canonicalize().with_context(|| format!("canonicalize {}", root.display()))?;
    eprintln!("== root: {}", root.display());

    let api_key = load_api_key()?;
    let observer = RateLimitObserver::new();
    let provider: Arc<dyn Provider> = Arc::new(
        AnthropicProvider::new(api_key)?.with_rate_limit_observer(observer.clone()),
    );

    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<ScanEvent>();
    let consumer = tokio::spawn(async move {
        while let Some(ev) = events_rx.recv().await {
            match &ev {
                ScanEvent::Started { root } => eprintln!("== started: {}", root.display()),
                ScanEvent::IngestComplete { walk } => {
                    eprintln!("== ingest: {} candidates, {} skipped", walk.candidates.len(), walk.skipped.len())
                }
                ScanEvent::TriageComplete { triaged } => {
                    let kept = triaged.iter().filter(|t| format!("{:?}", t.result.priority) != "Skip").count();
                    eprintln!("== triage: {}/{} kept", kept, triaged.len());
                }
                ScanEvent::DetectFileComplete { rel_path, findings } => {
                    eprintln!("   detect[{rel_path}]: {} findings", findings.len());
                }
                ScanEvent::DetectFileErrored { rel_path, error } => {
                    eprintln!("   detect[{rel_path}]: ERROR {error}");
                }
                ScanEvent::DetectComplete { total } => eprintln!("== detect: {total} findings total"),
                ScanEvent::VerifyProgress { done, total } => eprintln!("   verify: {done}/{total}"),
                ScanEvent::VerifyComplete { verified } => eprintln!("== verify: {} verified", verified.len()),
                ScanEvent::PatchProgress { done, total } => eprintln!("   patch: {done}/{total}"),
                ScanEvent::PatchComplete { patches } => eprintln!("== patch: {} patches", patches.len()),
                ScanEvent::UsageUpdate { usage } => {
                    eprintln!(
                        "   tokens: in={} out={} cache_read={}",
                        usage.total.input_tokens, usage.total.output_tokens, usage.total.cache_read_input_tokens
                    );
                }
                ScanEvent::DurationsUpdate { durations } => {
                    eprintln!(
                        "   timing: triage={}ms detect={}ms verify={}ms patch={}ms total={}ms",
                        durations.triage_ms,
                        durations.detect_ms,
                        durations.verify_ms,
                        durations.patch_ms,
                        durations.total_ms
                    );
                }
                ScanEvent::RateLimited { attempt, retry_after_secs } => {
                    eprintln!("   !! rate limited, attempt {attempt}, retry in {retry_after_secs}s");
                }
            }
        }
    });

    let config = ScanConfig::default();
    let result = run_scan(root, provider, &config, events_tx, None, None, Some(observer)).await?;
    consumer.await.ok();

    println!("\n=== Final result ===");
    println!("ingest candidates: {}", result.ingest.candidates.len());
    println!("triaged keepers:   {}", result.triaged.iter().filter(|t| format!("{:?}", t.result.priority) != "Skip").count());
    println!("findings by file:  {}", result.findings_by_file.len());
    let total_findings: usize = result.findings_by_file.iter().map(|f| f.findings.len()).sum();
    println!("total findings:    {total_findings}");
    println!("detect errors:     {}", result.detect_errors.len());
    println!("verified:          {}", result.verified.len());
    let kept = result
        .verified
        .iter()
        .filter(|v| v.verdict.as_ref().map(|x| x.keep()).unwrap_or(false))
        .count();
    println!("verified kept:     {kept}");
    println!("patches:           {}", result.patches.len());
    println!(
        "tokens:            in={} out={} cache_read={}",
        result.usage.total.input_tokens, result.usage.total.output_tokens, result.usage.total.cache_read_input_tokens
    );
    println!(
        "wall:              triage={}ms detect={}ms verify={}ms patch={}ms total={}ms",
        result.durations.triage_ms,
        result.durations.detect_ms,
        result.durations.verify_ms,
        result.durations.patch_ms,
        result.durations.total_ms
    );

    Ok(())
}
