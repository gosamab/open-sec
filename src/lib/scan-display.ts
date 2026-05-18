/**
 * Pure rendering helpers for scan output — severity / priority / status
 * styling, error humanization, formatters. Lives outside the route so the
 * extracted workspace components (FileTree / FindingsList / FindingDetail /
 * ScanSummary) can share them without re-importing the whole page.
 */

import type { Finding, Priority, Severity, SkipReason, TriageRecord, Verdict } from './ipc';

export const SEVERITY_ORDER: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

export function severityRank(s: Severity): number {
	return SEVERITY_ORDER.indexOf(s);
}

export function severityClass(s: Severity): string {
	switch (s) {
		case 'critical':
			return 'bg-red-600 text-white';
		case 'high':
			return 'bg-orange-500 text-white';
		case 'medium':
			return 'bg-amber-400 text-amber-950';
		case 'low':
			return 'bg-blue-500 text-white';
		case 'info':
			return 'bg-zinc-400 text-zinc-50';
	}
}

export function severityDot(s: Severity): string {
	switch (s) {
		case 'critical':
			return 'bg-red-600';
		case 'high':
			return 'bg-orange-500';
		case 'medium':
			return 'bg-amber-400';
		case 'low':
			return 'bg-blue-500';
		case 'info':
			return 'bg-zinc-400';
	}
}

export function priorityChipClass(p: Priority | null): string {
	switch (p) {
		case 'high':
			return 'bg-orange-500/15 text-orange-700 dark:text-orange-300';
		case 'normal':
			return 'bg-zinc-500/15 text-zinc-600 dark:text-zinc-300';
		case 'low':
			return 'bg-blue-500/15 text-blue-600 dark:text-blue-300';
		default:
			return 'bg-zinc-300/30 text-zinc-400';
	}
}

export function priorityChipLabel(p: Priority | null): string {
	switch (p) {
		case 'high':
			return 'H';
		case 'normal':
			return 'N';
		case 'low':
			return 'L';
		default:
			return '·';
	}
}

export function skipReasonLabel(r: SkipReason): string {
	switch (r) {
		case 'excluded_dir':
			return 'vendor/build dir';
		case 'too_large':
			return 'too large';
		case 'binary':
			return 'binary';
		case 'minified':
			return 'minified';
		case 'io_error':
			return 'io error';
	}
}

/** Single canonical status for a finding. See the state machine below.
 *
 *  Detect produces a Finding with kind = 'vuln' | 'hardening'.
 *    - vuln       → verifying → { open | dropped }   (pending if scan halts)
 *    - hardening  → open      (skips verify; detect already confirmed it)
 *
 *  User actions transition from any of the above to a terminal-ish state:
 *    dismissed | snoozed | accepted | patched.
 *
 *  Precedence when multiple apply (top wins):
 *    patched → dismissed → snoozed → accepted → (verifier outcome). */
export type FindingStatus =
	| 'verifying' // mid-flight verify (vuln only, scan running)
	| 'pending' // no verdict yet, scan not running (vuln only)
	| 'open' // confirmed real & unaddressed (vuln post-verify OR hardening)
	| 'dropped' // verifier said not exploitable (vuln only)
	| 'snoozed' // user deferred
	| 'dismissed' // user dismissed
	| 'accepted' // user accepted as-is
	| 'patched'; // patch applied to disk

export interface FindingStatusInputs {
	triageById: Map<string, TriageRecord>;
	appliedPatchIds: Set<string>;
	verdictById: Map<string, Verdict | null>;
	/** When true, vuln findings without a verdict show as `verifying` instead
	 *  of `pending` — the scan is mid-flight so a verdict is still expected. */
	scanning: boolean;
}

export function findingStatus(f: Finding, s: FindingStatusInputs): FindingStatus {
	if (s.appliedPatchIds.has(f.id)) return 'patched';
	const t = s.triageById.get(f.id);
	if (t?.status === 'dismissed') return 'dismissed';
	if (t?.status === 'snoozed') return 'snoozed';
	if (t?.status === 'accepted') return 'accepted';
	if (f.kind === 'hardening') return 'open';
	if (!s.verdictById.has(f.id)) return s.scanning ? 'verifying' : 'pending';
	const v = s.verdictById.get(f.id);
	if (v === null || v === undefined) return 'pending';
	return v.is_reachable && v.concrete_exploit ? 'open' : 'dropped';
}

export function statusClass(s: FindingStatus): string {
	switch (s) {
		case 'open':
			return 'bg-rose-500/15 text-rose-700 dark:text-rose-300';
		case 'patched':
			return 'bg-emerald-500/20 text-emerald-700 dark:text-emerald-300';
		case 'accepted':
			return 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-300';
		case 'snoozed':
			return 'bg-violet-500/15 text-violet-700 dark:text-violet-300';
		case 'dismissed':
			return 'bg-zinc-400/15 text-zinc-500';
		case 'dropped':
			return 'bg-zinc-400/15 text-zinc-500 line-through';
		case 'pending':
			return 'bg-amber-500/15 text-amber-700 dark:text-amber-300';
		case 'verifying':
			return 'bg-amber-500/15 text-amber-700 dark:text-amber-300 animate-pulse';
	}
}

export function statusDotClass(s: FindingStatus): string {
	switch (s) {
		case 'open':
			return 'bg-rose-500';
		case 'patched':
			return 'bg-emerald-500';
		case 'accepted':
			return 'bg-emerald-400';
		case 'snoozed':
			return 'bg-violet-500';
		case 'dismissed':
			return 'bg-zinc-400';
		case 'dropped':
			return 'bg-zinc-400';
		case 'pending':
		case 'verifying':
			return 'bg-amber-500';
	}
}

/** Label-with-detail for cases that need extra info (e.g. snooze remaining). */
export function statusLabelFor(f: Finding, s: FindingStatusInputs): string {
	const st = findingStatus(f, s);
	if (st === 'snoozed') {
		const t = s.triageById.get(f.id);
		if (t?.snooze_until) {
			const days = Math.max(0, Math.ceil((t.snooze_until - Date.now()) / 86_400_000));
			return `snoozed · ${days}d`;
		}
	}
	if (st === 'verifying') return 'verifying…';
	return st;
}

/** Compact "12.4k" / "3.2M" style for token counts. Returns the raw number
 *  when below 1k since precision matters at small scales. */
export function compactTokens(n: number): string {
	if (n < 1000) return n.toString();
	if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
	return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 1 : 0)}M`;
}

/** Format a millisecond duration as `12ms` / `3.4s` / `1m 23s` / `1h 5m`. */
export function formatDuration(ms: number): string {
	if (ms < 1000) return `${Math.round(ms)}ms`;
	const totalSec = ms / 1000;
	if (totalSec < 60) return `${totalSec.toFixed(totalSec < 10 ? 1 : 0)}s`;
	const totalMin = Math.floor(totalSec / 60);
	const secs = Math.round(totalSec - totalMin * 60);
	if (totalMin < 60) return `${totalMin}m ${secs}s`;
	const hr = Math.floor(totalMin / 60);
	const min = totalMin - hr * 60;
	return `${hr}h ${min}m`;
}

export function basename(p: string): string {
	const idx = p.lastIndexOf('/');
	return idx >= 0 ? p.slice(idx + 1) : p;
}

/** Split a data-flow narrative on arrows ("→" or "->") into discrete steps. */
export function parseDataFlow(text: string): string[] {
	const parts = text
		.split(/\s*(?:→|->)\s*/g)
		.map((s) => s.trim())
		.filter(Boolean);
	return parts.length > 0 ? parts : [text];
}

// ---------- findings filter + sort ----------

export type SortKey = 'severity' | 'file' | 'line' | 'status';
export type SortDir = 'asc' | 'desc';
export type KindFilter = 'all' | 'vuln' | 'hardening';

export interface FindingsFilter {
	sortKey: SortKey;
	sortDir: SortDir;
	/** Empty set = include every severity. */
	severities: Set<Severity>;
	/** Empty set = include every status. */
	statuses: Set<FindingStatus>;
	kind: KindFilter;
}

export const DEFAULT_FINDINGS_FILTER: FindingsFilter = {
	sortKey: 'severity',
	sortDir: 'asc',
	severities: new Set(),
	statuses: new Set(),
	kind: 'all'
};

/** Count of "narrowing" axes a user has applied — to badge the filter button.
 *  Sort changes don't count as filtering. */
export function activeFilterCount(f: FindingsFilter): number {
	let n = 0;
	if (f.severities.size > 0) n++;
	if (f.statuses.size > 0) n++;
	if (f.kind !== 'all') n++;
	return n;
}

export interface FindingForFilter {
	rel: string;
	f: import('./ipc').Finding;
}

/** Apply `filter` to `xs`, returning a new array. Caller still owns
 *  text-search and per-file selection — those are upstream. */
export function applyFindingsFilter(
	xs: FindingForFilter[],
	filter: FindingsFilter,
	statusInputs: FindingStatusInputs
): FindingForFilter[] {
	let out = xs;
	if (filter.severities.size > 0) {
		out = out.filter((x) => filter.severities.has(x.f.severity));
	}
	if (filter.statuses.size > 0) {
		out = out.filter((x) => filter.statuses.has(findingStatus(x.f, statusInputs)));
	}
	if (filter.kind !== 'all') {
		out = out.filter((x) => x.f.kind === filter.kind);
	}
	out = [...out].sort((a, b) => {
		let cmp = 0;
		switch (filter.sortKey) {
			case 'severity':
				cmp = severityRank(a.f.severity) - severityRank(b.f.severity);
				if (cmp === 0) cmp = a.rel.localeCompare(b.rel);
				break;
			case 'file':
				cmp = a.rel.localeCompare(b.rel);
				if (cmp === 0) cmp = a.f.line_start - b.f.line_start;
				break;
			case 'line':
				cmp = a.rel.localeCompare(b.rel);
				if (cmp === 0) cmp = a.f.line_start - b.f.line_start;
				break;
			case 'status': {
				const order: FindingStatus[] = [
					'open',
					'verifying',
					'pending',
					'patched',
					'accepted',
					'snoozed',
					'dismissed',
					'dropped'
				];
				cmp =
					order.indexOf(findingStatus(a.f, statusInputs)) -
					order.indexOf(findingStatus(b.f, statusInputs));
				if (cmp === 0) cmp = severityRank(a.f.severity) - severityRank(b.f.severity);
				break;
			}
		}
		return filter.sortDir === 'asc' ? cmp : -cmp;
	});
	return out;
}

export function diffLineClass(line: string): string {
	if (line.startsWith('+++') || line.startsWith('---')) return 'text-zinc-500';
	if (line.startsWith('+')) return 'bg-green-500/10 text-green-700 dark:text-green-300';
	if (line.startsWith('-')) return 'bg-red-500/10 text-red-700 dark:text-red-300';
	if (line.startsWith('@@')) return 'text-blue-600 dark:text-blue-300';
	return 'text-zinc-600 dark:text-zinc-300';
}

/** Translate raw backend errors (anyhow chains, ProviderError display) into
 *  short, plain-English messages. Returns the original string as a fallback
 *  for anything we don't recognize. */
export function humanizeError(raw: string): { title: string; detail?: string } {
	const lower = raw.toLowerCase();

	const rateMatch = raw.match(/retry after Some\((\d+)(?:\.\d+)?s\)/i);
	if (rateMatch) {
		return {
			title: `Rate limited by Anthropic — retry in ${rateMatch[1]}s`,
			detail: 'The API throttled us. Re-scan after the cooldown.'
		};
	}
	if (lower.includes('rate limited')) {
		return {
			title: 'Rate limited by Anthropic',
			detail: 'Wait a minute or two, then re-scan.'
		};
	}
	if (lower.includes('authentication failed') || lower.includes('invalid api key')) {
		return {
			title: 'Invalid Anthropic API key',
			detail: 'Update your key in Settings.'
		};
	}
	if (lower.includes('overloaded')) {
		return {
			title: 'Anthropic is overloaded',
			detail: 'The model is temporarily unavailable. Try again shortly.'
		};
	}
	if (
		lower.includes('context') &&
		(lower.includes('window') ||
			lower.includes('length') ||
			lower.includes('too long') ||
			lower.includes('exceeds'))
	) {
		return {
			title: 'File is too large for the model',
			detail: 'Detect skipped this file because its context exceeded the model limit.'
		};
	}
	if (
		lower.includes('network error') ||
		lower.includes('connection') ||
		lower.includes('timed out') ||
		lower.includes('timeout')
	) {
		return {
			title: 'Network error reaching Anthropic',
			detail: 'Check your internet connection and try again.'
		};
	}
	if (lower.includes('stream error')) {
		return {
			title: 'Connection dropped mid-response',
			detail: 'The model started replying but the stream broke. Re-scan to retry.'
		};
	}
	const serverMatch = raw.match(/server error \((\d+)\)/i);
	if (serverMatch) {
		return {
			title: `Anthropic server error (${serverMatch[1]})`,
			detail: 'Temporary upstream issue. Try again in a moment.'
		};
	}
	if (lower.includes('decode error') || lower.includes('json') || lower.includes('parse')) {
		return {
			title: "Couldn't parse the model's response",
			detail: "Detect got a reply but it wasn't valid JSON. Re-scanning usually fixes this."
		};
	}
	if (
		lower.includes('iteration') ||
		lower.includes('tool-use cap') ||
		lower.includes('25 iterations')
	) {
		return {
			title: 'Detect agent gave up',
			detail: 'The agent hit the 25-tool-call limit on this file without producing a verdict.'
		};
	}
	if (lower.includes('cancelled')) {
		return { title: 'Cancelled' };
	}
	if (lower.includes('bad request')) {
		return {
			title: 'Anthropic rejected the request',
			detail: raw.replace(/^.*?bad request:\s*/i, '')
		};
	}
	const stripped = raw
		.replace(/^detect failed:\s*/i, '')
		.replace(/^anthropic generate call failed:\s*/i, '');
	return { title: 'Detect failed', detail: stripped };
}
