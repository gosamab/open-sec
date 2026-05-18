/**
 * Scan-pipeline state — everything that flows from running (or loading) a
 * scan. Owned by this store so the route, the workspace view, and the
 * ScanSummary panel can all see the same writer.
 */

import {
	EMPTY_STAGE_DURATIONS,
	EMPTY_STAGE_USAGE,
	type Finding,
	type Patch,
	type ScanResult,
	type StageDurations,
	type StageUsage,
	type TriagedFile,
	type Verdict
} from '$lib/ipc';

class ScanState {
	// Selected project / API key gate
	keyConfigured = $state(false);
	root = $state('');

	// Lifecycle
	scanning = $state(false);
	cancelling = $state(false);
	stage = $state<string>('idle');
	error = $state<string | null>(null);
	rateLimitNotice = $state<{ attempt: number; retry_after_secs: number } | null>(null);
	/** `Date.now()` snapshot taken when a scan starts. Drives the realtime
	 *  total-duration counter in the progress bar; cleared back to `null`
	 *  when the scan ends so we fall back to the orchestrator-reported value. */
	scanStartedAt = $state<number | null>(null);

	// Per-stage outputs (populated by scan:event stream)
	walk = $state<ScanResult['ingest'] | null>(null);
	triaged = $state<TriagedFile[]>([]);
	findingsByFile = $state<Map<string, Finding[]>>(new Map());
	detectErrors = $state<Map<string, string>>(new Map());
	verdictById = $state<Map<string, Verdict | null>>(new Map());
	patchById = $state<Map<string, Patch>>(new Map());
	usage = $state<StageUsage>(EMPTY_STAGE_USAGE);
	durations = $state<StageDurations>(EMPTY_STAGE_DURATIONS);

	// Persisted result + the root it belongs to (for "did the user switch
	// projects since the last result?" comparisons).
	scanResult = $state<ScanResult | null>(null);
	resultRoot = $state<string | null>(null);

	// Patch regeneration history (per finding_id, ordered oldest → newest)
	patchHistoryById = $state<Map<string, Patch[]>>(new Map());

	// Detect-error retry
	retryingFiles = $state<Set<string>>(new Set());
	retryingAll = $state(false);

	/** Wipe every piece of per-scan state. Used by every code path that
	 *  needs to forget the previous scan (starting a new scan, switching
	 *  projects, returning to the launcher). Selection / triage are owned
	 *  by other stores and reset separately. */
	resetResults() {
		this.walk = null;
		this.triaged = [];
		this.findingsByFile = new Map();
		this.detectErrors = new Map();
		this.verdictById = new Map();
		this.patchById = new Map();
		this.usage = EMPTY_STAGE_USAGE;
		this.durations = EMPTY_STAGE_DURATIONS;
		this.rateLimitNotice = null;
		this.scanResult = null;
	}
}

export const scan = new ScanState();
