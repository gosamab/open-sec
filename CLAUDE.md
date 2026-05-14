# open-sec

Local-first security code scanner. Desktop app (macOS + Windows) that scans
codebases for vulnerabilities using LLMs. Architecture: triage (Haiku) →
detect (Sonnet, agentic) → adversarially verify (Opus) → propose patches
(Sonnet, displayed only, never applied).

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

## Phase 1 scope

Build a scanner that walks a folder, triages files, runs an agentic
detection pass with tool use, adversarially verifies, generates patches,
and shows results in a live three-pane UI. Persists to SQLite.

**Out of scope (Phase 2+):** local model support, SCA (dependency CVE
scanning), scheduled scans, multi-repo, exports, patch application.

The `Provider` trait lives in `src-tauri/src/providers/mod.rs`. Phase 1 has
one impl (Anthropic). Trait is shaped to support llama.cpp / OpenAI-compat
later without changing call sites.

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

### Detection output schema
- `kind: "vuln" | "hardening"` — hardening skips verifier
- `cwe` required, `owasp` optional
- `severity: critical | high | medium | low | info` (prompt defines each sharply to avoid info-bucket bloat)
- Cross-file findings attached to the **sink** file; source described in `data_flow`

### Detection agent tools (all sandboxed to scan-root — non-negotiable)
- `read_file`, `read_file_range`, `grep`, `find_references`, `list_directory`, `list_imports`, `git_blame`
- `list_imports` uses tree-sitter
- `git_blame` gracefully degrades when target isn't a git repo

### Verifier
- Output: `{ is_reachable, source_is_untrusted, concrete_exploit, reasoning }`
- Keep finding iff `is_reachable && concrete_exploit != null`

### Patch generation
- Model returns `{ file, anchor_line, old_block, new_block, explanation }`
- Rust locates `old_block` (exact, fuzzy fallback) and synthesizes the unified diff
- Display only, never auto-apply

### UI workflow
- Entry: hybrid launcher with sidebar of past scans
- Three-pane live workspace from t=0 of a scan (left: file tree with status badges, middle: findings stream, right: detail or summary dashboard)
- Raw findings appear immediately with "verifying…" badge, update in place
- Empty detail pane = scan summary dashboard (counts by severity/CWE/file, cost, triage funnel)
- Triage workflow: accept / dismiss(with reason) / snooze, persisted
- Stable finding ID = `sha256(file + line_range + cwe + normalized_description)[..16]`
- Re-scans diff against prior scan; triage status carries over via stable ID
- No dismiss-feedback loop into prompts in Phase 1
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

## Project layout

```
open-sec/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs                # tauri::Builder, plugin/handler wiring
│   │   ├── commands.rs           # #[tauri::command] handlers
│   │   ├── providers/
│   │   │   ├── mod.rs            # Provider trait + shared types
│   │   │   └── anthropic.rs
│   │   ├── scanner/
│   │   │   ├── mod.rs            # orchestrator
│   │   │   ├── ingest.rs         # repo walk
│   │   │   ├── triage.rs
│   │   │   ├── detect.rs         # agent loop
│   │   │   ├── verify.rs
│   │   │   └── patch.rs
│   │   ├── tools/                # tool-use handlers
│   │   ├── store.rs              # SQLite
│   │   ├── config.rs
│   │   └── error.rs
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/                          # SvelteKit
│   ├── routes/
│   ├── lib/
│   │   ├── components/ui/        # shadcn-svelte components
│   │   ├── stores/
│   │   └── ipc.ts                # typed wrappers around invoke()
│   └── app.html
├── components.json
├── config.toml
├── .env.example
└── README.md
```

## Commands

- `bun run tauri dev` — run the app in dev mode
- `bun run tauri build` — production build
- `bun run dev` — frontend only (no Tauri shell)
- `bun run check` — typecheck
- `cd src-tauri && cargo check` — Rust typecheck
- `cd src-tauri && cargo test` — Rust tests

## Build order (sequenced — confirm with user before advancing)

1. ✅ Scaffold (SvelteKit + Tauri, Tailwind, shadcn, hello-world IPC)
2. ✅ Provider trait + Anthropic implementation (1h cache TTL, custom SSE parser, wiremock tests)
3. ✅ Single-file scan, no tools (calibrated against textbook fixtures)
4. ✅ Tool-use agent loop (7 tools sandboxed to scan-root, 25-iter cap)
5. ✅ Triage pass (Haiku gate, 4 priority buckets, 10/10 fixture calibration)
6. ✅ Verification pass (Opus + tools, structured exploit schema, 5/5 textbook vulns KEPT, adversarial finding DROPPED)
7. ✅ Patch generation (Sonnet + tools, exact + fuzzy locate, unified diffs via `diffy`, hardening also patched)
8. ✅ Full pipeline + UI (three-pane workspace)
   - ✅ 8a. Orchestrator + `pipeline_cli` (end-to-end: ingest → triage → detect → verify → patch)
   - ✅ 8b. SvelteKit three-pane UI with live event streaming (folder picker, files/findings/detail panes, progressive updates as triage→detect→verify→patch land)
9. ✅ Persistence (SQLite scans/findings)
   - ✅ 9a. Store schema, scan history, launcher reads from SQLite, past-scan hydration
   - ✅ 9b. Triage workflow (accept/dismiss/snooze) keyed by stable finding ID; carries over on re-scan
   - ✅ 9c. Cancel with partial findings — `CancellingProvider` wraps every API call, stage-boundary checks return partial state, status `cancelled` persisted

## Calibration log

### Step 3: detection prompt baseline (no tools)
- Built a 6-file fixture suite at `fixtures/` covering SQLi, command injection,
  path traversal, SSRF, XSS, plus a clean parameterized-query handler.
- First sweep: 5/5 textbook vulns correctly flagged as `vuln` with correct CWEs
  (89, 78, 22, 918, 79). Severities calibrated as expected (critical for RCE,
  high for the rest). 0 false-positive vulns on the safe file.
- Observed failure mode: model emitted ~2–3 `hardening` items per file, often
  *duplicating* the vuln it had just reported (e.g. SQLi vuln + "missing
  input validation" hardening = same finding twice). Net noise inflation ~3×.

### Step 3.5: dedupe rule
- Patched the detection prompt with an explicit rule: hardening items must not
  be the mitigation, restatement, or sub-aspect of a vuln in the same response.
  Concrete examples in the rule body (SQLi → no "parameterize" hardening;
  command injection → no "use argv form" hardening).
- Re-ran the sweep: ~70% reduction in hardening noise, all 5 vulns still
  detected, genuinely-distinct hardening preserved (e.g. SSRF + Content-Type
  reflection on `ssrf_fetch.ts` both survived; Flask 0.0.0.0 exposure on
  `command_injection.py` survived). Acceptable trade: some defensible
  defense-in-depth observations on `safe_handler.ts` got suppressed.

### Step 4: agent loop verification
- Added cross-file fixture at `fixtures/cross-file/` — `handler.ts` looks safe
  because it calls `sanitize()` before the SQL query, but `sanitize.ts` is a
  no-op stub. Catchable only with tools.
- Real-API check: agent called `read_file('sanitize.ts')` (1 iteration), then
  emitted CWE-89 high with a `data_flow` narrative explicitly noting
  "sanitize.ts reveals the function is a stub that returns its argument
  unchanged." Tool-use behavior verified end-to-end.
- Single-file SQLi comparison (`sqli_express.ts`) between `--no-tools` and
  tools modes: both found the same vuln. Tools mode produced a tighter result
  (line range narrowed 11-22 → 15-22, borderline hardening dropped). Net:
  tools improve or match no-tools on every fixture tested.

### Step 4.5: prompt dedupe
- Consolidated the two near-identical system prompts (no-tools / with-tools)
  into one `BASE_DETECTION_PROMPT` const plus a `TOOLS_PREAMBLE` that's
  prepended when tools are active. Single source of truth for the schema,
  severity guide, and rules. 39 unit tests still green.

**Re-run protocol after any prompt edit:** the sweep at the top of this log
costs ~$0.03 and ~2 minutes. Always run it; compare to baseline before
declaring a prompt change shipped.

### Step 5: triage pass
- Built `scanner/ingest.rs` (walk + pre-triage skips: extension allowlist,
  exact-name allowlist for `Dockerfile`/`docker-compose.yml`/`.env.example`,
  vendor-dir exclusion, 500KB cap, null-byte = binary, avg line len > 200 =
  minified). Returns `Candidate` + `Skipped` so the funnel is visible.
- Built `scanner/triage.rs` (`Priority::{High,Normal,Low,Skip}`,
  `triage_one`, `triage_many` with `tokio::sync::Semaphore` at concurrency
  8). Full file content sent to Haiku; system prompt cached at 1h TTL so
  parallel workers share it. Output is strict JSON `{priority, reason}`.
- Built `bin/triage_cli` for calibration: walks a dir, prints
  `prio | file | reason` table plus a bucket funnel + pre-triage skip
  breakdown.
- Calibration fixture set at `fixtures/triage/` — 10 files spanning all four
  buckets:
    high   — `api/webhook.ts`, `auth/login.ts`, `db/users.py`
    normal — `domain/pricing.ts`, `lib/format.py`
    low    — `auth/login.test.ts`, `domain/pricing.spec.ts`
    skip   — `generated/schema.ts`, `index.ts`, `types/api.ts`
- First sweep: 8/10. Both pure-logic files (`pricing.ts`, `format.py`) got
  demoted from `normal` to `low` — the model was reading "no trust boundary"
  as "low priority" instead of "normal priority".

### Step 5.5: lock `low` to test/example files only
- Tightened the prompt: `low` is RESERVED for test/fixture/example code, and
  a pure-logic first-party file is `normal` regardless of trust-boundary
  status. Added an explicit rule explaining *why* (pure-logic bugs are still
  in scope for detection; `low` exists only to drain the queue last).
- Re-ran: 10/10. Reasons stayed concrete and grounded. Wider sweep over the
  full `fixtures/` tree (18 files) bucketed every textbook vulnerable file
  to `high` including `clean/safe_handler.ts` and `cross-file/sanitize.ts`
  — correct: detect should still look at both. 49 unit tests green.

### Step 6: verification pass
- Extracted `scanner/util.rs` for the JSON helpers (`extract_json_object`,
  `collect_text`) — third call site (detect + triage + verify) made the
  duplication real.
- Built `scanner/verify.rs`: `Verdict { is_reachable, source_is_untrusted,
  concrete_exploit, reasoning }`. Exploit is structured —
  `{ kind: http|args|file|other, request?, payload, expected_effect }` —
  with `request` filled only for `kind=http`. Keep iff `is_reachable &&
  concrete_exploit.is_some()`.
- Agent loop mirrors detect: same tool set, 25-iter cap, 1h cached system
  prompt. Default model `claude-opus-4-7`; concurrency 2 via
  `tokio::Semaphore`. Hardening findings bypass the verifier entirely
  (return `verdict: None`) and never consume an API call.
- Built `bin/verify_cli`: by default runs detect→verify on a file; `--finding
  <json>` accepts a hand-crafted finding so we can stress-test the drop path
  on adversarial cases detection wouldn't naturally produce.
- Calibration sweep:
  - All 5 textbook vulns (sqli/cmdi/path-traversal/ssrf/xss) → **KEPT** with
    structured exploits. Exploit `kind` distribution: 4× http, 1× other
    (DOM XSS — correctly excludes the `request` field).
  - The ssrf fixture's hardening companion finding → passed through (no
    verifier call), kept=1 / hardening_passthrough=1.
  - Adversarial fixture (`fixtures/adversarial/`): a SQLi-looking handler
    gated by a strict-hex regex in middleware.ts. Detect read the middleware
    via the agent loop and downgraded the finding to `hardening` on its
    own — so the verifier wasn't exercised on this path. To still exercise
    verify's drop logic, fed a synthetic `vuln` finding via `--finding`:
    verifier read middleware.ts adversarially, correctly emitted
    `is_reachable: false`, `concrete_exploit: null`, with reasoning
    "the detection pass missed the upstream allowlist sanitizer." ✅
- Bugs found and fixed along the way:
  - `temperature` is rejected by Opus 4.7 ("deprecated for this model");
    verify omits it. Sonnet/Haiku still accept its absence.
  - `extract_json_object` was matching `{16}` from prose (e.g.
    `/^[a-f0-9]{16}$/`). Tightened the extractor to require the first
    non-whitespace inner byte to be `"`, i.e. start of a quoted key. Added
    regression test.
- 56 unit tests green (50 → 56: +4 verify, +2 util).

### Step 7: patch generation
- Built `scanner/patch.rs`. Schema per CLAUDE.md:
  `PatchProposal { file, anchor_line, old_block, new_block, explanation }`.
  Rust wraps with `Patch { finding_id, proposal, located, diff }` where
  `Located::{Exact, Fuzzy, NotFound}` records how `old_block` was matched.
- `locate()`: exact `find()` first; on miss, line-by-line trimmed match that
  returns the file's true substring (not the model's trimmed copy) so the
  diff edits the real bytes. Refuses pure-whitespace needles.
- Diff via `diffy::create_patch`. Diffy's default `--- original / +++
  modified` header is stripped and replaced with a bare `--- <file>` /
  `+++ <file>` pair tagged with the focus file path (no `a/`/`b/` prefix —
  avoids double-slash when the path is absolute).
- Agent loop reuses the detect/verify pattern: Sonnet default, full tool
  access (read_file/grep/etc), 25-iter cap, 1h cached system prompt. No
  `temperature` (consistent with verify).
- `propose_many` filter: KEPT vulns (`verdict.keep()`) AND all hardening
  items. Dropped vulns are skipped without an API call.
- Built `bin/patch_cli`: runs detect → verify → patch on a single file,
  prints per-finding `[exact|fuzzy|not-found]` tag + diff or raw blocks.
  `--skip-verify` short-circuits the verifier (useful for stressing the
  patcher on raw detection output).
- Calibration sweep:
  - **sqli_express.ts** (CWE-89): exact-match patch. Template literal →
    parameterized `$1`/`$2` with values array. Explanation names the
    exploit closure (`1 OR 1=1--`). ✅
  - **ssrf_fetch.ts** (CWE-918 vuln + CWE-116 hardening): TWO patches
    emitted in one run. Vuln patch adds an `ALLOWED_SCHEMES` /
    `ALLOWED_HOSTS` allowlist plus an RFC-1918 / metadata-IP block regex
    before `fetch`. Hardening patch adds a Content-Type allowlist with
    utf-8 charset pinning. Both exact-match. ✅
  - **cross-file/handler.ts** (CWE-89 via no-op `sanitize` stub): exact-
    match. Patcher used `read_file` on `sanitize.ts` (confirmed via INFO
    log), then removed the dead `sanitize()` call AND switched to a
    parameterized query in the same edit. Explanation calls out the
    removal explicitly. ✅
- 66 unit tests green (56 → 66: +10 patch).

### Step 8a: pipeline orchestrator
- Built `scanner/orchestrate.rs::run_scan(root, provider, config) ->
  ScanResult`. Chains ingest → triage → detect → verify → patch
  sequentially (each stage gets the full output of the previous), with
  per-stage parallelism via `tokio::Semaphore` at the concurrencies locked
  in CLAUDE.md (triage=8, detect=4, verify=2, patch=4).
- `ScanResult` carries every intermediate stage's output —
  `ingest`/`triaged`/`findings_by_file`/`verified`/`patches` — so the UI
  (Step 8b) can render funnels and per-file detail without re-running
  anything. `--json` flag on `pipeline_cli` emits the whole result.
- Triage filter: `priority != Skip` files flow into detect; everything else
  is dropped. Test/example/low files still get detected (they're queued
  last only for ordering; we don't actually queue by priority here yet —
  that's a future UI concern).
- Detect errors are logged + skipped, not fatal. Verify/patch errors per
  finding likewise pass through with `verdict: None` / no patch.
- Built `bin/pipeline_cli`: stderr gets the per-stage funnel summary +
  `tracing` logs; stdout gets the kept-findings table and per-finding diff
  blocks (or the raw `ScanResult` JSON in `--json` mode). Clean separation
  for piping.
- End-to-end smoke on `fixtures/vulnerable/` (5 files): ingest=5,
  triage=5 high, detect=6 findings across 5 files, verify=5 KEPT + 1
  hardening passthrough, patch=6 proposals all exact-match. Numbers match
  per-stage CLI calibration runs exactly — orchestrator is wired right.
- Out of scope here (deferred to UI step): budget caps, >1000-file
  confirmation, cancellation that "keeps partial findings". The
  >1000 case currently just emits a `tracing` warn.

### Step 8b: three-pane SvelteKit workspace
- Added `ScanEvent` enum in `orchestrate.rs` and refactored `run_scan` to
  take an optional `mpsc::UnboundedSender<ScanEvent>`. Events emitted:
  `Started`, `IngestComplete(walk)`, `TriageComplete(triaged)`,
  `DetectFileComplete(rel_path, findings)` per file, `DetectComplete`,
  `VerifyComplete(verified)`, `PatchComplete(patches)`. `pipeline_cli`
  passes `None` (unchanged behaviour); the Tauri command threads an mpsc
  sender into the scan and forwards events via `AppHandle::emit("scan:event")`.
- New Tauri command `run_pipeline(root)` returns the full `ScanResult` and
  streams events while it works.
- `src/lib/ipc.ts` mirrors every Rust type — `Finding`, `Candidate`,
  `WalkResult`, `TriagedFile`, `VerifiedFinding`, `Patch`, `Located`,
  `Exploit`, `ScanResult`, `ScanEvent`. `listenScanEvents(cb)` returns an
  `UnlistenFn` for cleanup. Strict types, no `any`.
- `+page.svelte` replaces the Step 4 single-file UI with a three-pane
  workspace:
  - Topbar with folder picker + Scan button + live stage status string
  - **Left pane**: file list with severity dot per file (top severity) +
    finding count. "All files" entry at top selects the un-filtered view.
    Triage funnel badges at the bottom once triage lands.
  - **Middle pane**: findings list. Each entry shows severity badge, CWE,
    title, file:lines. Verification badge transitions from `verifying…`
    (animated pulse) → `kept` / `dropped` / `hardening` as
    `VerifyComplete` lands. Click to focus.
  - **Right pane**: detail view of the selected finding — description,
    data flow, verifier verdict + structured exploit (request/payload/
    expected_effect), and the patch with a coloured unified-diff block
    (+/- line highlighting via Tailwind classes; Shiki is deferred).
    Empty selection → summary dashboard with ingest/triage/findings/
    patches counts plus an expandable list of pre-triage skips.
- API key gate stays at the top when no key is configured.
- Live updates use Svelte 5 runes (`$state`, `$derived.by`) plus reactive
  `Map`s for `findingsByFile` / `verdictById` / `patchById`. Reassigning
  the Map (rather than mutating) triggers reactivity correctly.
- `bun run check` green, `bun run build` green, 66 Rust unit tests still
  green.
- Try it: `bun run tauri dev`, pick `fixtures/vulnerable`, hit Scan.
  Files populate as detect lands each file (~3s each, parallel 4); badges
  flip from `verifying…` to `kept`/`hardening` as verify lands; patches
  appear in the right pane on selection.
