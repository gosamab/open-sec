# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A Tauri v2 desktop app (macOS for v0.1) that walks a folder and runs a 5-stage
LLM pipeline against the source: ingest → triage → detect → verify → patch.
SvelteKit frontend (SPA via `adapter-static`), Rust + tokio backend, SQLite for
scan history, Anthropic API for the models.

## Pipeline architecture

The whole pipeline is driven by [`run_scan` in scanner/orchestrate.rs](src-tauri/src/scanner/orchestrate.rs) — stages
execute sequentially (each needs the previous stage's full output), but work
inside each stage runs in parallel under a per-stage `Semaphore`. Defaults:
triage=8, detect=4, verify=2, patch=4.

1. **Ingest** ([scanner/ingest.rs](src-tauri/src/scanner/ingest.rs)) — pure sync I/O. Walks `root` via the
   `ignore` crate (respects `.gitignore`), drops vendor/build dirs
   (`node_modules`, `vendor`, `dist`, `build`, `.next`, `target`, `__pycache__`,
   `.venv`, `coverage`, `.git`), files > 500 KB, binaries (null byte in first 8
   KB), and minified files (avg line length > 200). Allowed extensions and the
   short list of bare filenames (`Dockerfile`, `docker-compose.yml`,
   `.env.example`) are in `ALLOWED_EXTS` / `ALLOWED_NAMES`.
2. **Triage** ([scanner/triage.rs](src-tauri/src/scanner/triage.rs)) — Haiku (`claude-haiku-4-5`) buckets each
   candidate into `Priority::{High, Normal, Low, Skip}`. `Low` is reserved for
   test/example/fixture code only; pure-logic files default to `Normal`.
3. **Detect** ([scanner/detect.rs](src-tauri/src/scanner/detect.rs)) — Sonnet (`claude-sonnet-4-6`) agent loop
   with 7 read-only tools (see below), capped at 25 iterations per file. Emits
   `Finding { kind: vuln | hardening, severity, cwe, title, line_start,
   line_end, description, data_flow }`. Hardening items must describe a
   DISTINCT issue from any sibling vuln in the same response.
4. **Verify** ([scanner/verify.rs](src-tauri/src/scanner/verify.rs)) — Opus (`claude-opus-4-7`) adversarially
   re-examines `vuln` findings. Returns `Verdict { is_reachable,
   source_is_untrusted, concrete_exploit, reasoning }`. Decision rule in
   `Verdict::keep`: keep iff `is_reachable && concrete_exploit.is_some()`.
   Hardening findings bypass the verifier (no API call) — they're
   defense-in-depth notes, not reachability claims.
5. **Patch** ([scanner/patch.rs](src-tauri/src/scanner/patch.rs)) — Sonnet drafts a `PatchProposal { file,
   anchor_line, old_block, new_block, explanation }`. Rust locates `old_block`
   in the source (exact, then line-trimmed fuzzy), and synthesizes a unified
   diff via `diffy`. Patches are display-only during scan; the `apply_patch`
   command writes to disk later and records the apply in SQLite so the "applied"
   badge survives reloads.

Each stage's `ScanEvent` is forwarded via Tauri `emit("scan:event", …)` so the
UI populates progressively — `detect_file_complete` is per-file. The
`StageUsage` (per-stage token totals + rolling total) is updated and emitted at
each stage boundary.

## Provider trait + decorators

The `Provider` trait is in [providers/mod.rs](src-tauri/src/providers/mod.rs) — `generate()` returns a
`Response { content: Vec<ContentBlock>, stop_reason, usage }` and `stream()`
yields a `BoxStream<StreamEvent>`. Only one impl today: `AnthropicProvider`
(direct `reqwest`, no SDK crate). Two decorators in [providers/counting.rs](src-tauri/src/providers/counting.rs)
wrap whatever inner provider is in play during a scan:

- `CountingProvider` adds every response's `Usage` to a shared `UsageCounter`.
  The orchestrator snapshots that counter at each stage boundary and subtracts
  to get per-stage spend (`diff` in the same file).
- `CancellingProvider` short-circuits every `generate` / `stream` call with
  `ProviderError::Cancelled` once the shared `AtomicBool` flips. Already-running
  HTTP requests aren't aborted — cancel takes effect at the **next** round-trip.
  This is how the budget cap is enforced too: when `budget_total_tokens` is
  exceeded, the orchestrator flips the flag at the next stage boundary.

This is why you never want to thread cancellation tokens manually through every
`*_many` signature — wrapping the provider is the chosen mechanism.

## Anthropic API specifics (locked)

- Cache the system prompt + tool block with the 1-hour beta:
  `anthropic-beta: extended-cache-ttl-2025-04-11`. `CacheControl::ephemeral_1h`
  is set on the system block and on the **last** tool only — the unit test
  `cache_control_is_only_on_last_tool` enforces this. Putting it elsewhere
  invalidates the cached block.
- Content-block message shape (`text` / `tool_use` / `tool_result`) — not the
  OpenAI flat format.
- **Omit `temperature`** on verify and patch — Opus 4.7 rejects it
  ("deprecated for this model"). Triage and detect set `temperature = 0.0`;
  see comment in [scanner/verify.rs:207](src-tauri/src/scanner/verify.rs#L207).

## Detection-agent tools (sandboxed)

Defined in [tools/mod.rs](src-tauri/src/tools/mod.rs): `read_file`, `read_file_range`, `grep`,
`find_references`, `list_directory`, `list_imports` (tree-sitter for `.rs / .ts
/ .tsx / .js / .jsx / .mjs / .cjs / .py`), `git_blame` (gracefully degrades
when target isn't a git repo).

All tools route through `tools::sandbox::resolve_inside` ([tools/sandbox.rs](src-tauri/src/tools/sandbox.rs))
which canonicalizes the requested path and rejects anything that doesn't
`starts_with(canonical_scan_root)` — this catches `..` traversal and symlinks
that point out of the root. `canonical_root` must be computed once per scan
and passed in; the resolver does not re-canonicalize the root on each call.

## Stable finding ID

`stable_id(file, line_start, line_end, cwe, title)` in [scanner/mod.rs:65](src-tauri/src/scanner/mod.rs#L65)
returns the first 16 hex chars of a SHA-256. The title is whitespace-collapsed
and lowercased (`normalize`) so trivial rewording doesn't churn IDs. This is
what lets triage decisions and applied-patch records carry across re-scans:
the `triage` and `applied_patches` SQLite tables key on `(finding_id, root)`.

## Storage

SQLite via `rusqlite` (bundled feature — no `tauri-plugin-sql`) at
`<app_data_dir>/open-sec.db`. Schema migrated forward via `PRAGMA user_version`
in [store.rs](src-tauri/src/store.rs) — currently v2. Tables: `scans`, `findings`, `triage`,
`applied_patches`. Complex per-finding payloads (`Verdict`, `Patch`) are stored
as JSON columns because they're always read together with their `Finding` and
never queried internally.

API keys live in the OS keychain via the `keyring` crate
(`open-sec` / `anthropic`); `ANTHROPIC_API_KEY` is loaded from `.env` as a dev
fallback by `dotenvy` ([config.rs](src-tauri/src/config.rs)).

## Concurrency model

Only **one** scan runs at a time. `commands::CancelHandle` is a single
`Mutex<Option<Arc<AtomicBool>>>` in app state — starting a new scan installs a
fresh flag, replacing whatever was there. The `cancel_scan` IPC command flips
the flag; the pipeline finishes the in-flight API call, skips the rest, and
returns the partial result with `status = "cancelled"`.

## Tauri command boundary

All `#[tauri::command]` handlers live in [commands.rs](src-tauri/src/commands.rs) and are registered in
[lib.rs](src-tauri/src/lib.rs). They return `Result<T, String>` (stringify errors at the
boundary with `format!("{e:#}")`) and most are `async` so they run on the tokio
runtime, not the Tauri main thread. The matching typed IPC wrappers (one
`invoke<T>(...)` per command) live in [src/lib/ipc.ts](src/lib/ipc.ts) — the Rust and
TypeScript types are kept structurally aligned by hand. Renaming or
reshaping a command means editing both files.

## Frontend

SvelteKit SPA. Two routes only:

- [src/routes/+page.svelte](src/routes/+page.svelte) — the launcher AND workspace UI in one big
  component. Owns scan state, the `listenScanEvents` subscription, the
  three-pane workspace (file tree / findings stream / detail), triage actions,
  patch apply, and export. Recent-scans/settings come from
  [src/lib/components/Launcher.svelte](src/lib/components/Launcher.svelte) and [Settings.svelte](src/lib/components/Settings.svelte).
- [src/routes/report/+page.svelte](src/routes/report/+page.svelte) — read-only report view.

Settings (per-stage model overrides, concurrency, budget cap) live in
`localStorage` via [settings.svelte.ts](src/lib/settings.svelte.ts) and are converted to a
`ScanConfigOverride` passed to `runPipeline`. There is **no `config.toml`** —
ignore older references; defaults are baked into `ScanConfig::default` in
[orchestrate.rs](src-tauri/src/scanner/orchestrate.rs).

Code excerpts (enclosing function/class) are extracted server-side with
tree-sitter in [scanner/excerpts.rs](src-tauri/src/scanner/excerpts.rs); the frontend renders them and the
side-by-side diff viewer with Shiki ([src/lib/shiki.svelte.ts](src/lib/shiki.svelte.ts)).

## Coding conventions

- `anyhow::Result` in app code; `thiserror::ProviderError` on the `Provider`
  trait (see [error.rs](src-tauri/src/error.rs)).
- No `unwrap()` outside tests.
- All long work uses `tokio::spawn` / `JoinSet`; never block the Tauri main
  thread.
- `tracing` everywhere; provider-touching functions are `#[instrument]`.
- All public Rust types derive `Debug, Clone, Serialize, Deserialize` (a few
  add `PartialEq, Eq, Copy` for enums).
- Frontend: strict TypeScript, no `any`. IPC always goes through the typed
  wrappers in [src/lib/ipc.ts](src/lib/ipc.ts) — don't call `invoke` from components.

## Commands

```sh
bun run tauri dev                              # full app (Tauri + hot-reload UI)
bun run tauri build                            # release .app + .dmg under src-tauri/target/release/bundle/
bun run dev                                    # frontend only, no Tauri shell
bun run check                                  # SvelteKit + svelte-check
bun run lint                                   # prettier --check + eslint
bun run format                                 # prettier --write
cd src-tauri && cargo check                    # Rust typecheck
cd src-tauri && cargo test --lib               # Rust unit tests (uses wiremock for the Anthropic client, tempfile for fs tools)
cd src-tauri && cargo test --lib scanner::tests::stable_id_is_deterministic   # single test
```

There are no separate CLI binaries — the `triage_cli` / `verify_cli` bins were
removed in a recent refactor. Everything runs through the Tauri app or the
unit tests.

## Fixtures

[fixtures/](fixtures/) — calibration corpora used to smoke-test prompt edits:
`clean/`, `vulnerable/`, `adversarial/`, `cross-file/`, `multi-lang/`,
`rust-vulnerable/`, `triage/`. Whenever you change a stage prompt
(detect/triage/verify/patch), re-scan the relevant fixtures and confirm the
detection/triage decisions still match before declaring the change shipped.
