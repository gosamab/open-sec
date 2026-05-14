import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type Severity = 'critical' | 'high' | 'medium' | 'low' | 'info';
export type FindingKind = 'vuln' | 'hardening';
export type Priority = 'high' | 'normal' | 'low' | 'skip';
export type SkipReason =
	| 'excluded_dir'
	| 'unsupported_ext'
	| 'too_large'
	| 'binary'
	| 'minified'
	| 'io_error';
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

export interface ScanResult {
	root: string;
	ingest: WalkResult;
	triaged: TriagedFile[];
	findings_by_file: FileFindings[];
	verified: VerifiedFinding[];
	patches: Patch[];
	usage: StageUsage;
}

export type ScanEvent =
	| { kind: 'started'; root: string }
	| { kind: 'ingest_complete'; walk: WalkResult }
	| { kind: 'triage_complete'; triaged: TriagedFile[] }
	| { kind: 'detect_file_complete'; rel_path: string; findings: Finding[] }
	| { kind: 'detect_file_errored'; rel_path: string; error: string }
	| { kind: 'detect_complete'; total: number }
	| { kind: 'verify_complete'; verified: VerifiedFinding[] }
	| { kind: 'patch_complete'; patches: Patch[] }
	| { kind: 'usage_update'; usage: StageUsage };

export async function greet(name: string): Promise<string> {
	return invoke<string>('greet', { name });
}

export async function hasAnthropicKey(): Promise<boolean> {
	return invoke<boolean>('has_anthropic_key');
}

export async function setAnthropicKey(key: string): Promise<void> {
	return invoke<void>('set_anthropic_key', { key });
}

export async function scanFile(path: string, scanRoot?: string): Promise<Finding[]> {
	return invoke<Finding[]>('scan_file', { path, scanRoot: scanRoot ?? null });
}

/** Subscribe to scan events. Caller must invoke the returned function to unlisten. */
export async function listenScanEvents(
	handler: (event: ScanEvent) => void
): Promise<UnlistenFn> {
	return listen<ScanEvent>('scan:event', (e) => handler(e.payload));
}

export async function runPipeline(root: string): Promise<ScanResult> {
	return invoke<ScanResult>('run_pipeline', { root });
}

// ---------- persisted scans -----------------------------------------------

export interface ScanGroup {
	root: string;
	latest_scan_id: string;
	latest_started_at: number; // ms since epoch
	latest_finished_at: number | null;
	latest_status: string;
	latest_kept: number;
	latest_total: number;
	scan_count: number;
}

export async function listScanGroups(limit?: number): Promise<ScanGroup[]> {
	return invoke<ScanGroup[]>('list_scan_groups', { limit: limit ?? null });
}

export async function loadScan(scanId: string): Promise<ScanResult> {
	return invoke<ScanResult>('load_scan', { scanId });
}

export async function deleteScan(scanId: string): Promise<void> {
	return invoke<void>('delete_scan', { scanId });
}

export async function deleteScansForRoot(root: string): Promise<void> {
	return invoke<void>('delete_scans_for_root', { root });
}
