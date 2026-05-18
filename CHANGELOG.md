# Changelog

All notable changes to Open Security are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
