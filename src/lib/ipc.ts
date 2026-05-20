import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type Severity = 'critical' | 'high' | 'medium' | 'low' | 'info';
export type FindingKind = 'vuln' | 'hardening';
export type Priority = 'high' | 'normal' | 'low' | 'skip';
export type SkipReason = 'excluded_dir' | 'too_large' | 'binary' | 'minified' | 'io_error';
export type ExploitKind = 'http' | 'args' | 'file' | 'other';

export interface Finding {
	id: string;
	kind: FindingKind;
	severity: Severity;
	cwe: string;
	owasp: string | null;
	title: string;
	file: string;
	line_start: number;
	line_end: number;
	description: string;
	data_flow: string;
}

export interface Candidate {
	path: string;
	rel_path: string;
	size_bytes: number;
	line_count: number;
}

export interface Skipped {
	path: string;
	rel_path: string;
	reason: SkipReason;
}

export interface WalkResult {
	candidates: Candidate[];
	skipped: Skipped[];
}

export interface TriageResult {
	priority: Priority;
	reason: string;
}

export interface TriagedFile {
	candidate: Candidate;
	result: TriageResult;
}

export interface HttpRequest {
	method: string;
	path: string;
	headers?: unknown;
	body?: unknown;
}

export interface Exploit {
	kind: ExploitKind;
	request?: HttpRequest;
	payload: string;
	expected_effect: string;
}

export interface Verdict {
	is_reachable: boolean;
	source_is_untrusted: boolean;
	concrete_exploit: Exploit | null;
	reasoning: string;
}

export interface VerifiedFinding {
	finding: Finding;
	verdict: Verdict | null;
}

export interface PatchProposal {
	file: string;
	anchor_line: number;
	old_block: string;
	new_block: string;
	explanation: string;
}

export type Located =
	| { kind: 'exact'; byte_offset: number; line_start: number; line_end: number }
	| {
			kind: 'fuzzy';
			byte_offset: number;
			line_start: number;
			line_end: number;
			matched_text: string;
	  }
	| { kind: 'not_found' };

export interface Patch {
	finding_id: string;
	proposal: PatchProposal;
	located: Located;
	diff: string | null;
}

export interface FileFindings {
	path: string;
	rel_path: string;
	findings: Finding[];
}

export interface Usage {
	input_tokens: number;
	output_tokens: number;
	cache_creation_input_tokens: number;
	cache_read_input_tokens: number;
}

export interface StageUsage {
	triage: Usage;
	detect: Usage;
	verify: Usage;
	patch: Usage;
	total: Usage;
}

/** Wall-clock milliseconds spent in each pipeline stage. `total_ms` is the
 *  cumulative scan duration as of the most recent emission. */
export interface StageDurations {
	ingest_ms: number;
	triage_ms: number;
	detect_ms: number;
	verify_ms: number;
	patch_ms: number;
	total_ms: number;
}

export type ScanStatus = 'running' | 'completed' | 'cancelled';

export const EMPTY_USAGE: Usage = {
	input_tokens: 0,
	output_tokens: 0,
	cache_creation_input_tokens: 0,
	cache_read_input_tokens: 0
};

export const EMPTY_STAGE_USAGE: StageUsage = {
	triage: EMPTY_USAGE,
	detect: EMPTY_USAGE,
	verify: EMPTY_USAGE,
	patch: EMPTY_USAGE,
	total: EMPTY_USAGE
};

export const EMPTY_STAGE_DURATIONS: StageDurations = {
	ingest_ms: 0,
	triage_ms: 0,
	detect_ms: 0,
	verify_ms: 0,
	patch_ms: 0,
	total_ms: 0
};

export interface DetectError {
	rel_path: string;
	error: string;
}

/** Per-file triage failure. Previously dropped silently — now surfaced so a
 *  scan whose triage stage broke doesn't masquerade as "clean". */
export interface TriageError {
	rel_path: string;
	error: string;
}

export interface ScanResult {
	root: string;
	ingest: WalkResult;
	triaged: TriagedFile[];
	triage_errors: TriageError[];
	findings_by_file: FileFindings[];
	detect_errors: DetectError[];
	verified: VerifiedFinding[];
	patches: Patch[];
	usage: StageUsage;
	durations: StageDurations;
	status: ScanStatus;
}

export type ScanEvent =
	| { kind: 'started'; root: string }
	| { kind: 'ingest_complete'; walk: WalkResult }
	| { kind: 'triage_complete'; triaged: TriagedFile[] }
	| { kind: 'triage_file_errored'; rel_path: string; error: string }
	| { kind: 'detect_file_complete'; rel_path: string; findings: Finding[] }
	| { kind: 'detect_file_errored'; rel_path: string; error: string }
	| { kind: 'detect_complete'; total: number }
	| { kind: 'verify_progress'; done: number; total: number }
	| { kind: 'verify_complete'; verified: VerifiedFinding[] }
	| { kind: 'patch_progress'; done: number; total: number }
	| { kind: 'patch_complete'; patches: Patch[] }
	| { kind: 'usage_update'; usage: StageUsage }
	| { kind: 'durations_update'; durations: StageDurations }
	| { kind: 'rate_limited'; attempt: number; retry_after_secs: number };

export async function hasAnthropicKey(): Promise<boolean> {
	return invoke<boolean>('has_anthropic_key');
}

export async function setAnthropicKey(key: string): Promise<void> {
	return invoke<void>('set_anthropic_key', { key });
}

export async function hasOpenAiKey(): Promise<boolean> {
	return invoke<boolean>('has_openai_key');
}

export async function setOpenAiKey(key: string): Promise<void> {
	return invoke<void>('set_openai_key', { key });
}

export async function scanFile(
	path: string,
	scanRoot?: string,
	model?: string
): Promise<Finding[]> {
	return invoke<Finding[]>('scan_file', {
		path,
		scanRoot: scanRoot ?? null,
		model: model ?? null
	});
}

/** Subscribe to scan events. Caller must invoke the returned function to unlisten. */
export async function listenScanEvents(handler: (event: ScanEvent) => void): Promise<UnlistenFn> {
	return listen<ScanEvent>('scan:event', (e) => handler(e.payload));
}

/** Optional config override matching the Rust `ScanConfig` shape. */
export interface ScanConfigOverride {
	triage_concurrency: number;
	detect_concurrency: number;
	verify_concurrency: number;
	patch_concurrency: number;
	triage_model: string;
	detect_model: string;
	verify_model: string;
	patch_model: string;
	budget_total_tokens: number | null;
}

export async function runPipeline(root: string, config?: ScanConfigOverride): Promise<ScanResult> {
	return invoke<ScanResult>('run_pipeline', { root, config: config ?? null });
}

/** Resume an interrupted scan. Re-uses the previous `scan_id` so incremental
 *  saves continue to overwrite the same row, and per-stage work that's
 *  already in the DB (detected files, verified findings, drafted patches)
 *  is skipped. */
export async function resumePipeline(
	scanId: string,
	config?: ScanConfigOverride
): Promise<ScanResult> {
	return invoke<ScanResult>('resume_pipeline', { scanId, config: config ?? null });
}

/** Flag the currently-running scan to cancel. Returns true if a scan was
 *  actively running. */
export async function cancelScan(): Promise<boolean> {
	return invoke<boolean>('cancel_scan');
}

/** Run only the filesystem walk + pre-triage skip heuristics — no LLM. The
 *  onboarding panel calls this on project open so the user sees LoC totals
 *  and an estimated USD/SAR cost before they commit to a scan. */
export async function estimateScan(root: string): Promise<WalkResult> {
	return invoke<WalkResult>('estimate_scan', { root });
}

// ---------- excerpts -----------------------------------------------------

export type ExcerptSource = 'enclosing_function' | 'line_range';

export interface Excerpt {
	language: string | null;
	start_line: number;
	end_line: number;
	text: string;
	source: ExcerptSource;
}

export async function getExcerpt(
	file: string,
	lineStart: number,
	lineEnd: number
): Promise<Excerpt> {
	return invoke<Excerpt>('get_excerpt', { file, lineStart, lineEnd });
}

// ---------- apply patch --------------------------------------------------

export interface ApplyPatchResult {
	located: Located;
	bytes_written: number;
}

export async function applyPatch(
	findingId: string,
	root: string,
	file: string,
	oldBlock: string,
	newBlock: string
): Promise<ApplyPatchResult> {
	return invoke<ApplyPatchResult>('apply_patch', {
		findingId,
		root,
		file,
		oldBlock,
		newBlock
	});
}

export interface AppliedPatchRecord {
	finding_id: string;
	file: string;
	applied_at: number;
}

export async function getAppliedForRoot(root: string): Promise<AppliedPatchRecord[]> {
	return invoke<AppliedPatchRecord[]>('get_applied_for_root', { root });
}

// ---------- export -------------------------------------------------------

export async function exportMarkdown(root: string): Promise<string> {
	return invoke<string>('export_markdown', { root });
}

export async function exportSarif(root: string): Promise<string> {
	return invoke<string>('export_sarif', { root });
}

/** Backend-side file write — avoids fs-plugin scope restrictions on the path
 *  returned by the native save dialog. */
export async function saveTextFile(path: string, content: string): Promise<void> {
	return invoke<void>('save_text_file', { path, content });
}

/** Ask the patcher for an alternative fix that's structurally different
 *  from the supplied prior attempts. `model` should match the user's
 *  configured `patch_model` so the regenerate routes to the same provider
 *  that produced the original patch. */
export async function regeneratePatch(
	root: string,
	verified: VerifiedFinding,
	priorAttempts: PatchProposal[],
	model?: string
): Promise<Patch> {
	return invoke<Patch>('regenerate_patch', {
		root,
		verified,
		priorAttempts,
		model: model ?? null
	});
}

// ---------- persisted scans -----------------------------------------------

export interface ScanGroup {
	root: string;
	latest_scan_id: string;
	latest_started_at: number; // ms since epoch
	latest_kept: number;
}

export async function listScanGroups(limit?: number): Promise<ScanGroup[]> {
	return invoke<ScanGroup[]>('list_scan_groups', { limit: limit ?? null });
}

export async function loadScan(scanId: string): Promise<ScanResult> {
	return invoke<ScanResult>('load_scan', { scanId });
}

export async function deleteScansForRoot(root: string): Promise<void> {
	return invoke<void>('delete_scans_for_root', { root });
}

// ---------- triage -------------------------------------------------------

export type TriageStatus = 'accepted' | 'dismissed' | 'snoozed';

export interface TriageRecord {
	finding_id: string;
	status: TriageStatus;
	reason: string | null;
	snooze_until: number | null;
	updated_at: number;
}

export async function setTriage(
	findingId: string,
	root: string,
	status: TriageStatus,
	reason?: string,
	snoozeUntil?: number
): Promise<void> {
	return invoke<void>('set_triage', {
		findingId,
		root,
		status,
		reason: reason ?? null,
		snoozeUntil: snoozeUntil ?? null
	});
}

export async function clearTriage(findingId: string, root: string): Promise<void> {
	return invoke<void>('clear_triage', { findingId, root });
}

export async function getTriageForRoot(root: string): Promise<TriageRecord[]> {
	return invoke<TriageRecord[]>('get_triage_for_root', { root });
}

/** Open a URL in the OS default browser. Backend whitelists http/https/mailto. */
export async function openUrl(url: string): Promise<void> {
	return invoke<void>('open_url', { url });
}

/** Open a file in VS Code (or any editor registering the `vscode://` URL
 *  handler — Cursor, VSCodium, etc.) at the given 1-indexed line. */
export async function openInEditor(path: string, line?: number): Promise<void> {
	return invoke<void>('open_in_editor', { path, line });
}
