# CLAUDE.md

Guidance for Claude Code when working in this repo. Everything that *can* be
read off the code lives in the code. This file captures the load-bearing
decisions and invariants you can't recover by reading a single file.

## What this is

Tauri v2 desktop app (macOS for v0.1). A 5-stage LLM pipeline scans a folder
for security issues: **ingest → triage → detect → verify → patch**. Driver is
[`run_scan`](src-tauri/src/scanner/orchestrate.rs); each stage's module
docstring covers its scope.

## Supported languages

[`scanner/languages.rs`](src-tauri/src/scanner/languages.rs) is the single
source of truth. Each entry pairs file extensions with a Shiki id and (when
we have a grammar) a tree-sitter `Lang`. Ingest, excerpts, and the
`list_imports` tool all read from it — to add a language, edit only that file.

## Provider trait + decorator order

The `Provider` trait (one impl: `AnthropicProvider`) is wrapped by three
decorators in [`providers/counting.rs`](src-tauri/src/providers/counting.rs).
The order is load-bearing:

```
CancellingProvider → CountingProvider → RetryingProvider → AnthropicProvider
```

- **Retry innermost** so its sleeps aren't token-counted.
- **Counting** sees post-retry responses only.
- **Cancellation outermost** short-circuits before any retry decision via a
  shared `AtomicBool`. Already-running HTTP requests aren't aborted; cancel
  takes effect at the *next* round-trip. Budget cap (`budget_total_tokens`)
  uses the same flag — orchestrator flips it at a stage boundary.

This decorator stack is why no `*_many` signature threads a cancel token.

## Anthropic API specifics (locked, hard to re-derive)

- **1-hour prompt cache** via beta header
  `anthropic-beta: extended-cache-ttl-2025-04-11`. `CacheControl::ephemeral_1h`
  must be set on the system block **and on the LAST tool only**. Setting it
  elsewhere invalidates the cache. The test
  `cache_control_is_only_on_last_tool` enforces this.
- **Content-block messages** (`text` / `tool_use` / `tool_result`) — not the
  OpenAI flat format.
- **Omit `temperature` on Opus** (verify, patch). Opus 4.7 rejects it as
  deprecated. Triage and detect set `temperature = 0.0`.

## Stable finding ID — drives carry-over

[`scanner::stable_id`](src-tauri/src/scanner/mod.rs) hashes
`(file, line_start, line_end, cwe, normalized_title)`. The title is
whitespace-collapsed + lowercased so trivial rewording doesn't churn IDs.
The `triage` and `applied_patches` tables key on `(finding_id, root)` —
that's how accept/dismiss/snooze and applied-patch badges survive a re-scan.
If you change what goes into the hash, you break that carryover.

## Concurrency

One scan at a time. `commands::CancelHandle` is a single
`Mutex<Option<Arc<AtomicBool>>>` in app state; starting a scan replaces
whatever was there. `cancel_scan` flips the flag and the pipeline returns
the partial result with `status = "cancelled"`.

Inside a scan, work runs in parallel under per-stage `tokio::Semaphore`s.
Defaults live in `ScanConfig::default`.

## Sandbox invariant

Every detection-agent tool routes through
[`tools::sandbox::resolve_inside`](src-tauri/src/tools/sandbox.rs), which
canonicalizes the requested path and rejects anything that doesn't
`starts_with(canonical_scan_root)`. Catches `..` traversal and
out-of-root symlinks. `canonical_root` is computed **once per scan** and
threaded in — the resolver does not re-canonicalize it per call.

## Storage

SQLite via `rusqlite` (bundled, no `tauri-plugin-sql`) at
`<app_data_dir>/open-sec.db`. Schema migrated forward via PRAGMA
`user_version`; current version is 5. Complex per-finding payloads
(`Verdict`, `Patch`) live in JSON columns because they're always read with
their `Finding` and never queried internally.

API keys live in the OS keychain (`keyring` crate, service `open-sec`,
account `anthropic`). `ANTHROPIC_API_KEY` env var is a dev fallback loaded
by `dotenvy`.

## Frontend

SvelteKit SPA, single route ([`src/routes/+page.svelte`](src/routes/+page.svelte))
that holds the launcher *and* the workspace; small components live in
[`src/lib/components/`](src/lib/components/). Settings are in `localStorage`
([`src/lib/settings.svelte.ts`](src/lib/settings.svelte.ts)) and convert to
a `ScanConfigOverride` passed to `runPipeline`.

**IPC discipline**: every call goes through the typed wrappers in
[`src/lib/ipc.ts`](src/lib/ipc.ts). Don't `invoke('foo')` from components.
The Rust handler signature in [`commands.rs`](src-tauri/src/commands.rs) and
its TypeScript wrapper are kept aligned by hand — renaming or reshaping a
command means editing both files.

## Coding conventions

- `anyhow::Result` in app code, `thiserror::ProviderError` on the `Provider`
  trait.
- No `unwrap()` outside tests.
- Long work goes on `tokio::spawn` / `JoinSet`. Never block the Tauri main
  thread.
- `tracing` everywhere; provider-touching functions are `#[instrument]`.
- Frontend: strict TS, no `any`.

## Commands

```sh
bun run tauri dev                              # full app + hot reload
bun run tauri build                            # .app + .dmg → src-tauri/target/release/bundle/
bun run check                                  # frontend typecheck
cd src-tauri && cargo test --lib               # Rust unit tests (wiremock + tempfile)
cd src-tauri && cargo test --lib scanner::languages::tests   # single test target
```

## Fixtures workflow

[`fixtures/`](fixtures/) holds calibration corpora (`clean/`, `vulnerable/`,
`adversarial/`, `cross-file/`, `multi-lang/`, `rust-vulnerable/`, `triage/`).
**When you edit a stage prompt** (detect / triage / verify / patch), re-scan
the relevant fixtures and confirm the decisions still match before shipping
the change. Prompts are tuned against this corpus.
