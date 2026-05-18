# Open Security

**Version 1.0.0** · package: `open-sec` · bundle: `com.oazab.open-sec`

AI-powered security code scanner. A desktop app that scans a folder, finds vulnerabilities, and proposes patches — using the Anthropic API.

## Install

Download the latest `.dmg` from [Releases](https://github.com/gosamab/open-sec/releases/latest) — Apple Silicon → `*_aarch64.dmg`, Intel → `*_x64.dmg`. Open the `.dmg` and drag **Open Security** to Applications.

First launch on an unsigned build: right-click the app → **Open** → **Open** to bypass Gatekeeper.

## Pipeline

```
ingest  →  triage  →  detect  →  verify  →  patch
 walk     Haiku     Sonnet      Opus       Sonnet
```

- **Ingest** — walks the folder, skipping vendor dirs, binaries, minified files, and files > 500 KB.
- **Triage** — classifies files as `high` / `normal` / `low` / `skip` so detection focuses on real risk.
- **Detect** — agent loop with 7 sandboxed read-only tools (read, grep, find references, imports, blame).
- **Verify** — adversarially re-examines each finding and returns a concrete exploit. Unreachable findings are dropped.
- **Patch** — proposes a minimal `old_block` → `new_block` fix and renders a unified diff.

Results persist to SQLite at `<app_data_dir>/open-sec.db`. Re-scanning the same folder reuses your accept/dismiss/snooze decisions. Interrupted scans (cancel, crash, quit) can be resumed — finished stages are reused, only the missing work is re-run.

## Requirements

- macOS 11 or later (Apple Silicon or Intel). Windows / Linux planned.
- Anthropic API key (stored in the macOS Keychain).

## Usage

1. Launch Open Security and paste your Anthropic API key.
2. Pick **+ New project** to scan a folder, or reopen a **Recent project**.
3. Browse findings in the workspace: file tree, findings list, and detail pane with diff and exploit.
4. **Open in editor** to jump to a finding in your default editor, or apply a patch in place.
5. **Export ▾** for Markdown, PDF, or SARIF v2.1.0.

## Build from source

```sh
git clone https://github.com/gosamab/open-sec.git
cd open-sec
bun install
bun run tauri build
```

The `.app` and `.dmg` land in `src-tauri/target/release/bundle/`.

## Develop

```sh
bun install
bun run tauri dev      # app with hot reload
bun run check          # frontend typecheck
cd src-tauri
cargo test --lib       # pipeline tests
```

## Stack

- **Shell** — Tauri v2
- **Frontend** — SvelteKit + TypeScript + Tailwind v4 + shadcn-svelte
- **Backend** — Rust + tokio, `reqwest`, `rusqlite`, `tree-sitter`, `diffy`, `keyring`
- **Models** — Claude `haiku-4-5` / `sonnet-4-6` / `opus-4-7`

## Privacy

Source code you scan is sent to the Anthropic API. Nothing else leaves your machine — no telemetry, no analytics. See [PRIVACY.md](PRIVACY.md) for details.

## License

MIT © 2026 Osama Azab and Open Security contributors.
