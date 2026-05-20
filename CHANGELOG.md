# Changelog

All notable changes to Open Security are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0] — 2026-05-20

### Added

- **OpenAI provider (gpt-5 family).** Full Chat Completions integration
  behind the same `Provider` trait as Anthropic, with content-block ↔ flat
  Chat Completions message translation, strict-mode JSON schema on
  `submit_*` tools, automatic cache-token accounting (read from
  `prompt_tokens_details.cached_tokens`), and OpenAI rate-limit header
  parsing. `temperature` is dropped on the wire for gpt-5; `reasoning_effort`
  is pinned to `"minimal"` so per-stage token budgets (sized for Anthropic
  output-only) aren't eaten by reasoning before the tool call lands.
- **Multiplex provider.** Each stage's model id is routed to the right
  provider via prefix (`gpt-*` → OpenAI, default Anthropic), so a single
  scan can mix providers per stage (e.g. Haiku triage + gpt-5 detect). A
  fail-fast gate reports a clear error if the stage's required key isn't
  configured.
- **Multi-provider rate-limit pacing.** The proactive pacing decorator now
  reads a `MultiObserver` keyed by provider, so an Anthropic exhaust
  doesn't pause OpenAI calls (or vice-versa). OpenAI's
  `x-ratelimit-{requests,tokens}-{remaining,reset}` headers are parsed
  with the same logic as Anthropic's `anthropic-ratelimit-*`.
- **OpenAI key management.** Stored at `<app_data_dir>/openai-api-key`
  with 0600 perms (mirrors the Anthropic key). New `has_openai_key` /
  `set_openai_key` commands; the API-key prompt and Settings panel handle
  both keys.
- **gpt-5 / gpt-5-mini / gpt-5-nano in the model preset selector**, plus
  pricing tables in the pre-scan cost estimator (USD per MTok, with
  cache-read accounting; OpenAI never bills cache writes so the
  cache-write field is unused on those rows).

### Fixed

- **Triage failures no longer silently drop files.** A triage call that
  errored (model truncation, read error, task panic) used to be
  `warn!`-logged and the file would just be absent from the result — a
  broken scan looked "clean" because every candidate fell out of the
  pipeline before detect ever ran. Now captured as `triage_errors` on
  `ScanResult`, emitted as `TriageFileErrored` events, persisted via
  schema V6, and surfaced in the UI as red "Scan errors" badges
  alongside detect errors. Each message carries a `"triage failed:"`
  prefix so the stage that broke is visible.
- **gpt-5 length-truncation now surfaces as a real error.** A response
  with `finish_reason: "length"` and empty content used to propagate
  upstream as the misleading "model didn't call the tool" — now mapped
  to a `BadRequest` whose message names the reasoning-budget culprit.
- Shiki integration uses a dynamic import for the highlighter, fixing a
  cold-start path that broke after a dependency bump.

### Changed

- DB schema bumped to user_version 6 (adds `triage_errors_json` to the
  `scans` table; default `'[]'` so existing rows load cleanly).
- CI: GitHub Actions bumped off Node-20-deprecated versions onto
  Node-24-capable ones.

[1.2.0]: https://github.com/gosamab/open-sec/releases/tag/v1.2.0

## [1.1.0] — 2026-05-19

### Added

- **Pre-scan cost estimation.** Walks the scan root, estimates per-stage
  token usage, and surfaces USD + SAR cost projections in the onboarding
  panel and a new rescan confirmation dialog. SAR-per-USD rate is
  configurable in Settings (default 3.75, the SAMA peg).
- **Proactive Anthropic rate-limit pacing.** A new provider decorator
  reads `anthropic-ratelimit-{requests,input-tokens,output-tokens}-{limit,remaining,reset}`
  from every response and sleeps until reset before the next call when
  any counter hits zero — or drops below 5% of its limit. Replaces the
  previous purely-reactive 429-retry behavior, so cascading 429 storms
  from concurrent stage calls are gone.
- **Model preset selector in Settings.** Each stage's model field now
  offers Opus 4.7 / Sonnet 4.6 / Haiku 4.5 plus a Custom… fallback for
  arbitrary IDs.
- **`pipeline_smoke` example.** Runs the full pipeline against any folder
  for end-to-end smoke testing without launching the UI.

### Changed

- **Verify defaults to Sonnet 4.6** (was Opus 4.7). Verify is per-finding
  so the model swap is the largest single lever on scan wall time; Sonnet
  is roughly 3–5× faster. Opus 4.7 remains selectable in Settings.
- **Structured output via tool-use.** All four LLM stages now return their
  final answer through a stage-specific submission tool
  (`submit_triage`, `submit_findings`, `submit_verdict`, `submit_patch`).
  Anthropic validates the tool input against the schema we send, so
  malformed JSON is structurally impossible — the previous free-text JSON
  parsing and its failure modes are gone. The agent loop terminates when
  the model calls the submission tool.
- **API key location documented correctly.** README, PRIVACY, and CLAUDE
  docs now reflect that the key lives at
  `<app_data_dir>/anthropic-api-key` (0600 perms), not the macOS Keychain.
- Release notes no longer claim builds are unsigned — v1.0.0 onward is
  Developer ID signed and Apple-notarized.

### Removed

- Dead JSON-extraction utilities (`extract_json_object`, `collect_text`,
  per-stage `parse_*` parsers) — replaced by the submission-tool path.

[1.1.0]: https://github.com/gosamab/open-sec/releases/tag/v1.1.0

## [1.0.0] — 2026-05-18

First public release.

### Pipeline

- Five-stage scan: **ingest → triage → detect → verify → patch**.
- Triage: `haiku-4-5` classifies files as `high` / `normal` / `low` / `skip`.
- Detect: `sonnet-4-6` agent loop with seven sandboxed read-only tools (read,
  grep, find references, imports, blame, list-dir, view-source-fragment).
- Verify: `opus-4-7` adversarially re-examines each finding and emits a
  concrete exploit. Unreachable findings are dropped.
- Patch: `sonnet-4-6` proposes a minimal `old_block` → `new_block` fix and
  renders a unified diff.
- One-hour Anthropic prompt cache via the `extended-cache-ttl-2025-04-11`
  beta header.

### App

- Tauri v2 desktop app for macOS 11+ (Apple Silicon and Intel).
- SvelteKit + Tailwind v4 + shadcn-svelte frontend with a single workspace
  view: file tree, findings list, detail pane with diff and exploit.
- **Resume** for interrupted scans — finished stages (detect / verify /
  patch) are reused from the last attempt; only the missing work is re-run.
- **Cancel** at any time; partial results are kept.
- **Open in editor** jumps to a finding's file + line in your default editor.
- Per-finding **accept / dismiss / snooze** that survives a re-scan via a
  stable finding ID hashed from `(file, line range, CWE, normalized title)`.
- Export to Markdown, PDF, or SARIF v2.1.0.
- Theme toggle (light / dark / midnight).

### Storage

- SQLite at `<app_data_dir>/open-sec.db` (schema version 5). Migrated forward
  via PRAGMA `user_version`. Findings, verdicts, patches, triage decisions,
  and applied-patch records all persist.
- Anthropic API key stored in the macOS Keychain (service `open-sec`,
  account `anthropic`).

### Languages

- Tree-sitter-backed import / reference analysis for Rust, JavaScript,
  TypeScript, Python, Dart, Java, C#, and HTML. Detection itself is
  language-agnostic and runs against any text file ≤ 500 KB.

### Privacy

- Zero telemetry, zero analytics, zero background activity.
- Only outbound traffic is to `api.anthropic.com`. See [PRIVACY.md](PRIVACY.md).

[1.0.0]: https://github.com/gosamab/open-sec/releases/tag/v1.0.0
