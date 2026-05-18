# open-sec

AI-powered security code scanner. Desktop app (macOS for v0.1, Windows planned) that scans
codebases for vulnerabilities using LLMs. Architecture: triage (Haiku) →
detect (Sonnet, agentic) → adversarially verify (Opus) → propose patches
(Sonnet).

## Tech stack (locked — do not propose alternatives)

- **Shell:** Tauri v2
- **Frontend:** SvelteKit (SPA via `adapter-static`), TypeScript, Tailwind v4, shadcn-svelte (nova / zinc)
- **Backend:** Rust, tokio
- **LLM:** Anthropic API via `reqwest` (no third-party SDK crate). Models: `claude-haiku-4-5`, `claude-sonnet-4-6`, `claude-opus-4-7`
- **Storage:** SQLite via `rusqlite` (bundled feature, Rust-only — no `tauri-plugin-sql`)
- **Repo walking:** `ignore` crate
- **Config:** `config.toml`
- **Secrets:** OS keychain via `keyring` crate, with `ANTHROPIC_API_KEY` env var fallback
- **Package manager (frontend):** `bun`

The `Provider` trait lives in `src-tauri/src/providers/mod.rs`. One impl today
(Anthropic). Trait is shaped so other backends slot in without changing call
sites.

## Locked decisions (from spec interview)

### Scanner pipeline
- Pre-triage skips: vendor/build dirs (`node_modules`, `vendor`, `dist`, `build`, `.next`, `target`, `__pycache__`, `.venv`, `coverage`), binary content (null bytes in first 8KB), avg line length > 200 (minified heuristic), file size > 500KB
- Concurrency: triage=8, detect=4, verify=2 (configurable in `config.toml`)
- Hard cap: 25 tool iterations per file during detection
- Soft budget cap: warn at 80%, stop at 100%, mark scan `budget-capped`
- Confirm-before-start dialog when scannable files > 1000
- File extensions scanned: `.rs .ts .tsx .js .jsx .mjs .cjs .py .go .rb .php .java .kt .swift .cs .c .cc .cpp .h .hpp .m .mm .svelte .vue .yml .yaml .tf .hcl .sh` + exact names `Dockerfile`, `docker-compose.yml`, `.env.example`
- Test files: included, but triage usually de-prioritizes them
- Models per stage configurable in `config.toml`; defaults baked in

### Anthropic API
- Prompt caching: 1h beta TTL (`anthropic-beta: extended-cache-ttl-2025-04-11`), `cache_control` on the last block of system+tools
- Content-block message format (text / tool_use / tool_result), not OpenAI flat format
- Opus 4.7 rejects `temperature` ("deprecated for this model"); omit it on verify/patch

### Detection output schema
- `kind: "vuln" | "hardening"` — hardening skips verifier
- `cwe` required, `owasp` optional
- `severity: critical | high | medium | low | info` (prompt defines each sharply to avoid info-bucket bloat)
- Cross-file findings attached to the **sink** file; source described in `data_flow`
- Hardening items must not restate or mitigate a vuln in the same response

### Detection agent tools (all sandboxed to scan-root — non-negotiable)
- `read_file`, `read_file_range`, `grep`, `find_references`, `list_directory`, `list_imports`, `git_blame`
- `list_imports` uses tree-sitter
- `git_blame` gracefully degrades when target isn't a git repo

### Triage
- `Priority::{High, Normal, Low, Skip}`. `low` is reserved for test/fixture/example code only — pure-logic first-party files are `normal` regardless of trust boundary

### Verifier
- Output: `{ is_reachable, source_is_untrusted, concrete_exploit, reasoning }`
- `concrete_exploit: { kind: http|args|file|other, request?, payload, expected_effect }` — `request` only when `kind=http`
- Keep finding iff `is_reachable && concrete_exploit != null`
- Hardening findings bypass the verifier entirely (no API call)

### Patch generation
- Model returns `{ file, anchor_line, old_block, new_block, explanation }`
- Rust locates `old_block` (exact, fuzzy fallback) and synthesizes the unified diff
- Runs on KEPT vulns + all hardening; dropped vulns are skipped

### UI workflow
- Entry: hybrid launcher with sidebar of past scans
- Three-pane live workspace from t=0 of a scan (left: file tree with status badges, middle: findings stream, right: detail or summary dashboard)
- Raw findings appear immediately with "verifying…" badge, update in place
- Empty detail pane = scan summary dashboard (counts by severity/CWE/file, cost, triage funnel)
- Triage workflow: accept / dismiss(with reason) / snooze, persisted
- Stable finding ID = `sha256(file + line_range + cwe + normalized_description)[..16]`
- Re-scans diff against prior scan; triage status carries over via stable ID
- Cancel keeps partial findings, scan marked `cancelled`

### Rendering
- Shiki for syntax highlighting (excerpts and the custom side-by-side diff viewer)
- tree-sitter extracts the enclosing function/class for excerpts

## Coding standards

- `anyhow::Result` in app code; `thiserror::ProviderError` on the Provider trait
- All `#[tauri::command]` return `Result<T, String>` (stringify errors at the boundary)
- No `unwrap()` outside tests
- All long work uses `tokio::spawn`, never blocks the Tauri main thread
- `tracing` everywhere; instrument every provider call
- All public Rust types: `#[derive(Debug, Clone, Serialize, Deserialize)]`
- Frontend: strict TypeScript, no `any`, typed IPC wrappers around `invoke`
- After any detection/triage/verify/patch prompt edit, re-run the `fixtures/` sweep (~$0.03, ~2 min) before declaring it shipped

## Layout

Rust backend in `src-tauri/src/` (entry: `lib.rs`; pipeline: `scanner/`; LLM: `providers/`; agent tools: `tools/`). Frontend in `src/` (routes in `src/routes/`, components/stores/IPC in `src/lib/`). Fixtures for calibration in `fixtures/`.

## Commands

- `bun run tauri dev` — run the app in dev mode
- `bun run tauri build` — production build
- `bun run dev` — frontend only (no Tauri shell)
- `bun run check` — typecheck
- `cd src-tauri && cargo check` — Rust typecheck
- `cd src-tauri && cargo test` — Rust tests
