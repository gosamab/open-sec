# open-sec

AI-powered security code scanner. Desktop app that walks a folder, triages
files with Haiku, runs an agentic detection pass with Sonnet, adversarially
verifies with Opus, and proposes patches — all without sending your code
anywhere except the Anthropic API.

## What it does

Scans run as a five-stage pipeline:

```
ingest  →  triage  →  detect  →  verify  →  patch
 walk     Haiku     Sonnet      Opus       Sonnet
 fs       buckets   agentic     adversarial single-file
                    tool use    reachability fix proposal
```

- **Ingest** filters vendor/build dirs (`node_modules`, `dist`, `.git`, …),
  binary content, minified output, and files > 500 KB.
- **Triage** classifies each candidate as `high` / `normal` / `low` / `skip`
  so detection focuses on real trust-boundary code.
- **Detect** runs an agent loop with 7 read-only tools sandboxed to the
  scan root (read_file, grep, find_references, list_imports via
  tree-sitter, list_directory, git_blame, read_file_range) — 25-iteration
  cap per file.
- **Verify** has Opus adversarially re-examine each finding, returning a
  structured exploit (HTTP request / payload / expected effect). Findings
  that aren't reachable or have no concrete exploit are dropped.
- **Patch** proposes a minimal `old_block` → `new_block` replacement;
  Rust re-locates `old_block` (exact, fuzzy fallback) and synthesises a
  unified diff via `diffy`.

Findings + verdicts + patches + triage decisions persist to SQLite at
`<app_data_dir>/open-sec.db`, so re-scanning the same folder carries over
your accept/dismiss/snooze decisions via a stable finding ID.

## Requirements

- macOS (Windows support is planned)
- Anthropic API key (stored in the OS keychain; falls back to
  `ANTHROPIC_API_KEY` from `.env` for development)

## Install

Build from source:

```sh
git clone <repo>
cd open-sec
bun install
bun run tauri build
```

The `.app` lands in `src-tauri/target/release/bundle/macos/` and the
`.dmg` in `src-tauri/target/release/bundle/dmg/`. Drag to Applications.

> **Unsigned local builds (macOS):** the first launch will warn that the
> developer can't be verified. Right-click → **Open** → confirm. Signed
> releases produced by the CI workflow don't trigger this prompt.

## Usage

1. Launch open-sec. Paste your Anthropic API key when prompted (saved to
   keychain).
2. From the launcher: pick **+ New project** to scan a folder, or click a
   **Recent project** to reopen a past result.
3. In the workspace:
   - **Left pane** — file tree with priority chips (`H`/`N`/`L`) and
     severity dots per file. Skipped files are visible but muted.
   - **Middle pane** — findings stream with severity / CWE / triage
     status. Filter and arrow-key navigation supported.
   - **Right pane** — finding detail: description, data flow, enclosing
     function excerpt (via tree-sitter), verifier verdict, structured
     exploit, suggested patch with syntax-highlighted diff. Apply / Try
     another fix / Triage actions live here.
4. Topbar **Export ▾** gives you Markdown, PDF, and SARIF v2.1.0 output.

## Development

```sh
bun install
bun run tauri dev      # launch app with hot reload
bun run check          # SvelteKit + Svelte typecheck
cd src-tauri
cargo test --lib       # unit tests across the pipeline
cargo check            # Rust typecheck
```

Frontend: SvelteKit (SPA via `adapter-static`), TypeScript strict mode,
Tailwind v4, shadcn-svelte.

Backend: Rust + tokio, `reqwest` for Anthropic, `rusqlite` (bundled) for
storage, `tree-sitter` for code analysis, `diffy` for unified diffs,
`keyring` for the API key.

## Configuration

In-app **Settings** (gear icon in workspace topbar):

- Per-stage model overrides
- Concurrency knobs (`triage=8 / detect=4 / verify=2 / patch=4` by default)
- Token budget cap — when exceeded mid-scan, partial findings are saved
  and the scan is marked `cancelled`

Settings are stored in `localStorage`.

## Tech stack

- **Shell** — Tauri v2
- **Models** — Claude `haiku-4-5` / `sonnet-4-6` / `opus-4-7`
- **Storage** — SQLite via `rusqlite` (no `tauri-plugin-sql`)
- **Repo walk** — `ignore` crate
- **Secrets** — OS keychain via `keyring`, `ANTHROPIC_API_KEY` env fallback

## License

MIT
