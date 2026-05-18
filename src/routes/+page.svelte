<script lang="ts">
	import { onDestroy, onMount } from 'svelte';

	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Badge } from '$lib/components/ui/badge';
	import Launcher from '$lib/components/Launcher.svelte';
	import Settings from '$lib/components/Settings.svelte';
	import ThemeToggle from '$lib/components/ThemeToggle.svelte';
	import logo from '$lib/assets/logo.png';
	import { renderMd, renderInlineMd } from '$lib/markdown';
	import { asScanConfig, settings } from '$lib/settings.svelte';
	import { highlightCode, highlightDiff } from '$lib/shiki.svelte';
	import {
		EMPTY_STAGE_USAGE,
		EMPTY_STAGE_DURATIONS,
		applyPatch,
		cancelScan,
		clearTriage,
		exportMarkdown,
		exportSarif,
		saveTextFile,
		getAppliedForRoot,
		getExcerpt,
		getTriageForRoot,
		regeneratePatch,
		hasAnthropicKey,
		listenScanEvents,
		listScanGroups,
		loadScan,
		runPipeline,
		setAnthropicKey,
		setTriage,
		type Excerpt,
		type Finding,
		type FileFindings,
		type Patch,
		type PatchProposal,
		type Priority,
		type ScanGroup,
		type ScanResult,
		type Severity,
		type SkipReason,
		type StageDurations,
		type StageUsage,
		type TriagedFile,
		type TriageRecord,
		type TriageStatus,
		type Verdict,
		type VerifiedFinding,
		type WalkResult
	} from '$lib/ipc';
	import type { UnlistenFn } from '@tauri-apps/api/event';

	// ---------- state ----------------------------------------------------
	let keyConfigured = $state(false);
	let keyInput = $state('');
	let savingKey = $state(false);

	let root = $state('');
	let scanning = $state(false);
	let cancelling = $state(false);
	let stage = $state<string>('idle');
	let error = $state<string | null>(null);

	type View = 'launcher' | 'workspace';
	let view = $state<View>('launcher');

	let walk = $state<WalkResult | null>(null);
	let triaged = $state<TriagedFile[]>([]);
	let findingsByFile = $state<Map<string, Finding[]>>(new Map());
	let detectErrors = $state<Map<string, string>>(new Map());
	let verdictById = $state<Map<string, Verdict | null>>(new Map());
	let patchById = $state<Map<string, Patch>>(new Map());
	let usage = $state<StageUsage>(EMPTY_STAGE_USAGE);
	let durations = $state<StageDurations>(EMPTY_STAGE_DURATIONS);
	/** Latest "rate-limited, retrying in Xs" notice from the provider, or
	 *  null. Cleared on the next non-rate-limited event so a transient
	 *  retry doesn't stick in the status bar after the scan resumes. */
	let rateLimitNotice = $state<{ attempt: number; retry_after_secs: number } | null>(null);
	let scanResult = $state<ScanResult | null>(null);

	/** Triage decisions keyed by finding_id (scoped to the current root). */
	let triageById = $state<Map<string, TriageRecord>>(new Map());
	let triageBusy = $state(false);

	/** Findings whose patch has been applied to disk in this session. Not
	 *  persisted — re-scanning the same root will produce a clean slate (and
	 *  hopefully not re-emit the same finding). */
	let appliedPatchIds = $state<Set<string>>(new Set());
	let applyBusy = $state(false);
	let applyError = $state<string | null>(null);

	/** Per-finding patch history. patchById holds the *currently displayed*
	 *  patch; this map keeps every variant the user has seen so they can flip
	 *  between alternatives. */
	let patchHistoryById = $state<Map<string, Patch[]>>(new Map());
	let regenBusy = $state(false);
	let regenError = $state<string | null>(null);

	function patchHistoryFor(findingId: string): Patch[] {
		const history = patchHistoryById.get(findingId);
		if (history && history.length > 0) return history;
		const current = patchById.get(findingId);
		return current ? [current] : [];
	}

	let selectedPatchVariants = $derived.by(() =>
		selectedFinding ? patchHistoryFor(selectedFinding.id) : []
	);
	let selectedPatchVariantIdx = $derived.by(() => {
		if (!selectedFinding || !selectedPatch) return 0;
		const list = selectedPatchVariants;
		const idx = list.findIndex((p) => p === selectedPatch);
		return idx < 0 ? Math.max(0, list.length - 1) : idx;
	});

	function selectPatchVariant(idx: number) {
		if (!selectedFinding) return;
		const list = patchHistoryFor(selectedFinding.id);
		const v = list[idx];
		if (!v) return;
		const next = new Map(patchById);
		next.set(selectedFinding.id, v);
		patchById = next;
	}

	async function regenerateAlternative() {
		if (!selectedFinding || !root) return;
		const verified = verifiedFor(selectedFinding);
		if (!verified) return;
		regenBusy = true;
		regenError = null;
		try {
			const existing = patchHistoryFor(selectedFinding.id);
			const priors = existing.map((p) => p.proposal);
			const newPatch = await regeneratePatch(root, verified, priors);
			const history = [...existing, newPatch];
			const histMap = new Map(patchHistoryById);
			histMap.set(selectedFinding.id, history);
			patchHistoryById = histMap;
			const pbi = new Map(patchById);
			pbi.set(selectedFinding.id, newPatch);
			patchById = pbi;
		} catch (e) {
			regenError = e instanceof Error ? e.message : String(e);
		} finally {
			regenBusy = false;
		}
	}

	function verifiedFor(f: Finding): VerifiedFinding | null {
		// Find the VerifiedFinding wrapper for a given Finding, from the
		// session's verified array. Required by regeneratePatch.
		if (!scanResult) return { finding: f, verdict: verdictById.get(f.id) ?? null };
		const v = scanResult.verified.find((x) => x.finding.id === f.id);
		return v ?? { finding: f, verdict: verdictById.get(f.id) ?? null };
	}

	async function applySelectedPatch() {
		if (!selectedPatch || !selectedFinding) return;
		applyBusy = true;
		applyError = null;
		try {
			const result = await applyPatch(
				selectedFinding.id,
				root,
				selectedPatch.proposal.file,
				selectedPatch.proposal.old_block,
				selectedPatch.proposal.new_block
			);
			if (result.located.kind === 'not_found' || result.bytes_written === 0) {
				applyError = 'Patch could not be located in the file — nothing was written.';
				return;
			}
			const next = new Set(appliedPatchIds);
			next.add(selectedFinding.id);
			appliedPatchIds = next;
		} catch (e) {
			applyError = e instanceof Error ? e.message : String(e);
		} finally {
			applyBusy = false;
		}
	}
	let dismissDraftFor = $state<string | null>(null);
	let dismissReason = $state('');
	// `hideDismissed` is a long-lived UI preference (unlike `filter` or
	// `expandedFolders`, which are tied to the current scan), so it survives
	// reloads via localStorage.
	const HIDE_DISMISSED_KEY = 'open-sec:hide-dismissed';
	let hideDismissed = $state<boolean>(
		typeof window !== 'undefined'
			? window.localStorage.getItem(HIDE_DISMISSED_KEY) !== 'false'
			: true
	);
	$effect(() => {
		if (typeof window === 'undefined') return;
		try {
			window.localStorage.setItem(HIDE_DISMISSED_KEY, String(hideDismissed));
		} catch {
			// quota / disabled — silent
		}
	});

	let selectedFile = $state<string | null>(null);
	let selectedFindingId = $state<string | null>(null);
	let filter = $state('');

	/** Root the *current* result set was produced for. Used to auto-clear
	 *  stale panes when the user picks/types a different folder. */
	let resultRoot = $state<string | null>(null);

	let unlisten: UnlistenFn | null = null;
	let settingsOpen = $state(false);
	let exportMenuOpen = $state(false);
	let exportMenuRef = $state<HTMLDivElement | null>(null);

	// ---------- lifecycle ------------------------------------------------
	onMount(async () => {
		keyConfigured = await hasAnthropicKey();
		unlisten = await listenScanEvents((ev) => {
			switch (ev.kind) {
				case 'started':
					stage = 'scanning…';
					break;
				case 'ingest_complete':
					walk = ev.walk;
					stage = `triaging ${ev.walk.candidates.length} file(s)…`;
					break;
				case 'triage_complete':
					triaged = ev.triaged;
					const keepers = ev.triaged.filter((t) => t.result.priority !== 'skip').length;
					stage = `detecting on ${keepers} file(s)…`;
					break;
				case 'detect_file_complete': {
					const next = new Map(findingsByFile);
					next.set(ev.rel_path, ev.findings);
					findingsByFile = next;
					break;
				}
				case 'detect_file_errored': {
					const f = new Map(findingsByFile);
					f.set(ev.rel_path, []);
					findingsByFile = f;
					const e = new Map(detectErrors);
					e.set(ev.rel_path, ev.error);
					detectErrors = e;
					break;
				}
				case 'detect_complete':
					stage = `verifying ${ev.total} finding(s)…`;
					break;
				case 'verify_complete': {
					const next = new Map(verdictById);
					for (const v of ev.verified) next.set(v.finding.id, v.verdict);
					verdictById = next;
					stage = 'proposing patches…';
					break;
				}
				case 'patch_complete': {
					const next = new Map(patchById);
					for (const p of ev.patches) next.set(p.finding_id, p);
					patchById = next;
					stage = 'done';
					break;
				}
				case 'usage_update':
					usage = ev.usage;
					rateLimitNotice = null;
					break;
				case 'durations_update':
					durations = ev.durations;
					break;
				case 'rate_limited':
					rateLimitNotice = {
						attempt: ev.attempt,
						retry_after_secs: ev.retry_after_secs
					};
					break;
			}
		});
	});

	onDestroy(() => {
		if (unlisten) unlisten();
	});

	// ---------- actions --------------------------------------------------
	async function runScan() {
		if (!root || scanning) return;
		scanning = true;
		cancelling = false;
		error = null;
		stage = 'starting…';
		walk = null;
		triaged = [];
		findingsByFile = new Map();
		detectErrors = new Map();
		verdictById = new Map();
		patchById = new Map();
		usage = EMPTY_STAGE_USAGE;
		durations = EMPTY_STAGE_DURATIONS;
		rateLimitNotice = null;
		scanResult = null;
		selectedFile = null;
		selectedFindingId = null;
		// Clear per-finding session state too — stale badges from the previous
		// run shouldn't paint onto freshly-emitted findings while the scan is
		// in flight. Both maps are repopulated post-scan from the DB.
		triageById = new Map();
		appliedPatchIds = new Set();
		try {
			scanResult = await runPipeline(root, asScanConfig(settings.value));
			resultRoot = root;
			// Trust the backend-supplied status over our local `cancelling`
			// flag — the latter can race late-arriving stage events.
			stage = scanResult.status === 'cancelled' ? 'cancelled' : 'done';
			await reloadTriageForCurrentRoot();
			await reloadAppliedForCurrentRoot();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			stage = 'error';
		} finally {
			scanning = false;
			cancelling = false;
		}
	}

	$effect(() => {
		if (!exportMenuOpen) return;
		const onDoc = (e: PointerEvent) => {
			if (exportMenuRef && !exportMenuRef.contains(e.target as Node)) {
				exportMenuOpen = false;
			}
		};
		const onEsc = (e: KeyboardEvent) => {
			if (e.key === 'Escape') exportMenuOpen = false;
		};
		document.addEventListener('pointerdown', onDoc);
		document.addEventListener('keydown', onEsc);
		return () => {
			document.removeEventListener('pointerdown', onDoc);
			document.removeEventListener('keydown', onEsc);
		};
	});

	async function exportPdf() {
		if (!root) return;
		try {
			const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
			const existing = await WebviewWindow.getByLabel('report');
			const url = `/report?root=${encodeURIComponent(root)}&auto=1`;
			if (existing) {
				existing.setFocus();
				return;
			}
			const win = new WebviewWindow('report', {
				url,
				title: 'open-sec · report',
				width: 900,
				height: 1100,
				resizable: true
			});
			win.once('tauri://error', (e) => {
				error = `failed to open report window: ${JSON.stringify(e.payload)}`;
			});
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function exportTo(format: 'markdown' | 'sarif') {
		if (!root) return;
		try {
			const content =
				format === 'markdown' ? await exportMarkdown(root) : await exportSarif(root);
			const { save } = await import('@tauri-apps/plugin-dialog');
			const ext = format === 'markdown' ? 'md' : 'sarif.json';
			const stem = root.split(/[\\/]/).pop() || 'scan';
			const target = await save({
				title: format === 'markdown' ? 'Save markdown report' : 'Save SARIF report',
				defaultPath: `${stem}-open-sec.${ext}`,
				filters: [
					{
						name: format === 'markdown' ? 'Markdown' : 'SARIF',
						extensions: format === 'markdown' ? ['md'] : ['json', 'sarif']
					}
				]
			});
			if (typeof target !== 'string') return;
			await saveTextFile(target, content);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function requestCancel() {
		if (!scanning || cancelling) return;
		cancelling = true;
		stage = 'cancelling…';
		try {
			await cancelScan();
		} catch (e) {
			console.error('cancel_scan failed', e);
		}
	}

	/** Wipe previous-scan state when the user edits the folder field. We keep
	 *  the user's filter and selection cleared too — they belong to the old
	 *  scan. Skipped while a scan is running to avoid yanking state mid-flight. */
	$effect(() => {
		if (scanning) return;
		if (resultRoot === null) return;
		if (root === resultRoot) return;
		// Root diverged from the result set — reset everything.
		walk = null;
		triaged = [];
		findingsByFile = new Map();
		detectErrors = new Map();
		verdictById = new Map();
		patchById = new Map();
		usage = EMPTY_STAGE_USAGE;
		durations = EMPTY_STAGE_DURATIONS;
		rateLimitNotice = null;
		scanResult = null;
		selectedFile = null;
		selectedFindingId = null;
		filter = '';
		stage = 'idle';
		resultRoot = null;
	});

	function resetWorkspace() {
		walk = null;
		triaged = [];
		findingsByFile = new Map();
		detectErrors = new Map();
		verdictById = new Map();
		patchById = new Map();
		usage = EMPTY_STAGE_USAGE;
		durations = EMPTY_STAGE_DURATIONS;
		rateLimitNotice = null;
		scanResult = null;
		selectedFile = null;
		selectedFindingId = null;
		filter = '';
		stage = 'idle';
		error = null;
		resultRoot = null;
		triageById = new Map();
		dismissDraftFor = null;
		dismissReason = '';
	}

	async function reloadTriageForCurrentRoot() {
		if (!root) return;
		try {
			const rs = await getTriageForRoot(root);
			const m = new Map<string, TriageRecord>();
			for (const r of rs) m.set(r.finding_id, r);
			triageById = m;
		} catch (e) {
			console.error('getTriageForRoot failed', e);
		}
	}

	async function reloadAppliedForCurrentRoot() {
		if (!root) return;
		try {
			const rs = await getAppliedForRoot(root);
			const s = new Set<string>();
			for (const r of rs) s.add(r.finding_id);
			appliedPatchIds = s;
		} catch (e) {
			console.error('getAppliedForRoot failed', e);
		}
	}

	/** + New project: just enter workspace with the root set. No scan triggered. */
	function openProjectFresh(path: string) {
		resetWorkspace();
		root = path;
		view = 'workspace';
	}

	/** Recent click: load the past scan from SQLite and hydrate workspace state. */
	async function openProjectPast(group: ScanGroup) {
		resetWorkspace();
		root = group.root;
		view = 'workspace';
		try {
			const r = await loadScan(group.latest_scan_id);
			hydrateFromScanResult(r);
			await reloadTriageForCurrentRoot();
			await reloadAppliedForCurrentRoot();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	/** Replay a loaded ScanResult into the live workspace state, so the
	 *  panes look identical to what the user saw when the scan finished. */
	function hydrateFromScanResult(r: ScanResult) {
		walk = r.ingest;
		triaged = r.triaged;
		const fbf = new Map<string, Finding[]>();
		for (const ff of r.findings_by_file) fbf.set(ff.rel_path, ff.findings);
		findingsByFile = fbf;
		const errs = new Map<string, string>();
		for (const e of r.detect_errors ?? []) errs.set(e.rel_path, e.error);
		detectErrors = errs;
		const vbi = new Map<string, Verdict | null>();
		for (const v of r.verified) vbi.set(v.finding.id, v.verdict ?? null);
		verdictById = vbi;
		const pbi = new Map<string, Patch>();
		for (const p of r.patches) pbi.set(p.finding_id, p);
		patchById = pbi;
		usage = r.usage;
		durations = r.durations ?? EMPTY_STAGE_DURATIONS;
		rateLimitNotice = null;
		scanResult = r;
		resultRoot = r.root;
		stage = r.status === 'cancelled' ? 'cancelled' : 'done';
	}

	function backToLauncher() {
		view = 'launcher';
	}

	async function saveKey() {
		if (!keyInput.trim()) return;
		savingKey = true;
		try {
			await setAnthropicKey(keyInput.trim());
			keyConfigured = await hasAnthropicKey();
			keyInput = '';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			savingKey = false;
		}
	}

	/** Translate raw backend errors (anyhow chains, ProviderError display) into
	 *  short, plain-English messages. Returns the original string as a fallback
	 *  for anything we don't recognize. */
	function humanizeError(raw: string): { title: string; detail?: string } {
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
		if (lower.includes('context') && (lower.includes('window') || lower.includes('length') || lower.includes('too long') || lower.includes('exceeds'))) {
			return {
				title: 'File is too large for the model',
				detail: 'Detect skipped this file because its context exceeded the model limit.'
			};
		}
		if (lower.includes('network error') || lower.includes('connection') || lower.includes('timed out') || lower.includes('timeout')) {
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
				detail: 'Detect got a reply but it wasn\'t valid JSON. Re-scanning usually fixes this.'
			};
		}
		if (lower.includes('iteration') || lower.includes('tool-use cap') || lower.includes('25 iterations')) {
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
		// Strip the layered anyhow prefix so the user sees the leaf error.
		const stripped = raw.replace(/^detect failed:\s*/i, '').replace(/^anthropic generate call failed:\s*/i, '');
		return { title: 'Detect failed', detail: stripped };
	}

	// ---------- derived --------------------------------------------------
	const SEVERITY_ORDER: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

	function severityRank(s: Severity): number {
		return SEVERITY_ORDER.indexOf(s);
	}

	function severityClass(s: Severity): string {
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

	function severityDot(s: Severity): string {
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

	function priorityClass(p: Priority): string {
		switch (p) {
			case 'high':
				return 'bg-orange-500/15 text-orange-600 dark:text-orange-300';
			case 'normal':
				return 'bg-zinc-500/15 text-zinc-600 dark:text-zinc-300';
			case 'low':
				return 'bg-blue-500/15 text-blue-600 dark:text-blue-300';
			case 'skip':
				return 'bg-zinc-300/30 text-zinc-500 italic';
		}
	}

	function skipReasonLabel(r: SkipReason): string {
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
			case 'unsupported_ext':
				return 'unsupported ext';
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
	type FindingStatus =
		| 'verifying' // mid-flight verify (vuln only, scan running)
		| 'pending'   // no verdict yet, scan not running (vuln only)
		| 'open'      // confirmed real & unaddressed (vuln post-verify OR hardening)
		| 'dropped'   // verifier said not exploitable (vuln only)
		| 'snoozed'   // user deferred
		| 'dismissed' // user dismissed
		| 'accepted'  // user accepted as-is
		| 'patched';  // patch applied to disk

	function findingStatus(f: Finding): FindingStatus {
		if (appliedPatchIds.has(f.id)) return 'patched';
		const t = triageById.get(f.id);
		if (t?.status === 'dismissed') return 'dismissed';
		if (t?.status === 'snoozed') return 'snoozed';
		if (t?.status === 'accepted') return 'accepted';
		if (f.kind === 'hardening') return 'open';
		if (!verdictById.has(f.id)) return scanning ? 'verifying' : 'pending';
		const v = verdictById.get(f.id);
		if (v === null || v === undefined) return 'pending';
		return v.is_reachable && v.concrete_exploit ? 'open' : 'dropped';
	}

	function statusClass(s: FindingStatus): string {
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

	function statusDotClass(s: FindingStatus): string {
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

	function statusLabel(s: FindingStatus): string {
		if (s === 'verifying') return 'verifying…';
		if (s === 'snoozed') {
			// caller can pass the snooze-extended label via statusLabelFor()
			return 'snoozed';
		}
		return s;
	}

	/** Label-with-detail for cases that need extra info (e.g. snooze date). */
	function statusLabelFor(f: Finding): string {
		const s = findingStatus(f);
		if (s === 'snoozed') {
			const t = triageById.get(f.id);
			if (t?.snooze_until) {
				const days = Math.max(0, Math.ceil((t.snooze_until - Date.now()) / 86_400_000));
				return `snoozed · ${days}d`;
			}
		}
		return statusLabel(s);
	}

	// All findings, flattened, with their file rel_path attached.
	let allFindings = $derived.by(() => {
		const out: { rel: string; f: Finding }[] = [];
		for (const [rel, fs] of findingsByFile) {
			for (const f of fs) out.push({ rel, f });
		}
		out.sort((a, b) => severityRank(a.f.severity) - severityRank(b.f.severity));
		return out;
	});

	let visibleFindings = $derived.by(() => {
		let xs = allFindings;
		if (selectedFile) xs = xs.filter((x) => x.rel === selectedFile);
		if (hideDismissed) {
			xs = xs.filter((x) => triageById.get(x.f.id)?.status !== 'dismissed');
		}
		const q = filter.trim().toLowerCase();
		if (q) {
			xs = xs.filter(
				(x) =>
					x.f.title.toLowerCase().includes(q) ||
					x.f.cwe.toLowerCase().includes(q) ||
					x.rel.toLowerCase().includes(q) ||
					x.f.description.toLowerCase().includes(q)
			);
		}
		return xs;
	});

	let dismissedCount = $derived.by(() => {
		let n = 0;
		for (const t of triageById.values()) if (t.status === 'dismissed') n++;
		return n;
	});

	let selectedFinding = $derived.by(() => {
		if (!selectedFindingId) return null;
		return allFindings.find((x) => x.f.id === selectedFindingId)?.f ?? null;
	});

	let selectedVerdict = $derived.by(() => {
		if (!selectedFinding) return null;
		return verdictById.get(selectedFinding.id) ?? null;
	});

	let selectedPatch = $derived.by(() => {
		if (!selectedFinding) return null;
		return patchById.get(selectedFinding.id) ?? null;
	});

	// rel_path -> triage priority lookup, so the file pane can show why each
	// file is in the list (and not just rely on the aggregate badge).
	let priorityByFile = $derived.by(() => {
		const m = new Map<string, Priority>();
		for (const t of triaged) m.set(t.candidate.rel_path, t.result.priority);
		return m;
	});

	const PRIORITY_RANK: Record<Priority, number> = { high: 0, normal: 1, low: 2, skip: 3 };

	// ---------- file tree -------------------------------------------------
	type FileStatus =
		| 'pending'
		| 'scanned'
		| 'errored'
		| 'triage_skipped'
		| 'pre_triage_skipped';

	type FileNode = {
		type: 'file';
		path: string;
		name: string;
		status: FileStatus;
		priority: Priority | null;
		count: number;
		topSeverity: Severity | null;
		skipReason?: SkipReason;
		triageReason?: string;
		detectError?: string;
	};

	type FolderNode = {
		type: 'folder';
		path: string;
		name: string;
		children: TreeNode[];
		count: number;
		topSeverity: Severity | null;
		skippedCount: number;
		/** True iff every file under this folder is skipped (pre-triage or
		 *  triage). Lets the UI mute the whole folder so the user can see at
		 *  a glance that e.g. node_modules/ was wholly excluded. */
		allSkipped: boolean;
	};

	type TreeNode = FileNode | FolderNode;

	/** Folders the user has explicitly expanded. Default: empty (everything
	 *  collapsed). New folders that appear mid-scan stay collapsed unless the
	 *  user clicks. */
	let expandedFolders = $state<Set<string>>(new Set());

	function toggleFolder(p: string) {
		const next = new Set(expandedFolders);
		if (next.has(p)) next.delete(p);
		else next.add(p);
		expandedFolders = next;
	}

	function topSeverityOf(fs: Finding[]): Severity | null {
		let topRank = SEVERITY_ORDER.length;
		let top: Severity | null = null;
		for (const f of fs) {
			const r = severityRank(f.severity);
			if (r < topRank) {
				topRank = r;
				top = f.severity;
			}
		}
		return top;
	}

	function basename(p: string): string {
		const idx = p.lastIndexOf('/');
		return idx >= 0 ? p.slice(idx + 1) : p;
	}

	function buildFileNodes(): FileNode[] {
		const map = new Map<string, FileNode>();

		if (walk) {
			// Seed candidates (every file that survived pre-triage).
			for (const c of walk.candidates) {
				map.set(c.rel_path, {
					type: 'file',
					path: c.rel_path,
					name: basename(c.rel_path),
					status: 'pending',
					priority: null,
					count: 0,
					topSeverity: null
				});
			}
			// Pre-triage skipped (vendor dir, binary, minified, too large, io error).
			for (const s of walk.skipped) {
				// Skip pure unsupported_ext — they aren't useful to show in tree.
				if (s.reason === 'unsupported_ext') continue;
				map.set(s.rel_path, {
					type: 'file',
					path: s.rel_path,
					name: basename(s.rel_path),
					status: 'pre_triage_skipped',
					priority: null,
					count: 0,
					topSeverity: null,
					skipReason: s.reason
				});
			}
		}

		// Layer in triage decisions.
		for (const t of triaged) {
			const n = map.get(t.candidate.rel_path);
			if (!n) continue;
			n.priority = t.result.priority;
			if (t.result.priority === 'skip') {
				n.status = 'triage_skipped';
				n.triageReason = t.result.reason;
			}
		}

		// Layer in detect results.
		for (const [rel, fs] of findingsByFile) {
			const n = map.get(rel);
			if (!n) continue;
			n.count = fs.length;
			n.topSeverity = topSeverityOf(fs);
			if (n.status === 'pending') n.status = 'scanned';
		}

		// Layer in detect errors (overrides 'scanned' if applicable).
		for (const [rel, err] of detectErrors) {
			const n = map.get(rel);
			if (!n) continue;
			n.status = 'errored';
			n.detectError = err;
		}

		return [...map.values()];
	}

	function nestFiles(files: FileNode[]): TreeNode[] {
		const rootChildren: TreeNode[] = [];
		const folderCache = new Map<string, FolderNode>();

		const getOrCreateFolder = (segments: string[]): FolderNode | null => {
			if (segments.length === 0) return null;
			const path = segments.join('/');
			const existing = folderCache.get(path);
			if (existing) return existing;
			const folder: FolderNode = {
				type: 'folder',
				path,
				name: segments[segments.length - 1],
				children: [],
				count: 0,
				topSeverity: null,
				skippedCount: 0,
				allSkipped: false
			};
			folderCache.set(path, folder);
			if (segments.length === 1) {
				rootChildren.push(folder);
			} else {
				const parent = getOrCreateFolder(segments.slice(0, -1))!;
				parent.children.push(folder);
			}
			return folder;
		};

		for (const f of files) {
			const parts = f.path.split('/');
			if (parts.length === 1) {
				rootChildren.push(f);
			} else {
				const folder = getOrCreateFolder(parts.slice(0, -1))!;
				folder.children.push(f);
			}
		}

		// Sort: folders first then files, both alphabetically.
		const sortRecursively = (children: TreeNode[]) => {
			children.sort((a, b) => {
				if (a.type !== b.type) return a.type === 'folder' ? -1 : 1;
				return a.name.localeCompare(b.name);
			});
			for (const c of children) {
				if (c.type === 'folder') sortRecursively(c.children);
			}
		};
		sortRecursively(rootChildren);

		// Aggregate counts + top severity onto folders, plus an `allSkipped`
		// flag so the UI can mute folders that contain nothing-but-skips.
		const annotate = (node: TreeNode): {
			count: number;
			topSev: Severity | null;
			skipped: number;
			allSkipped: boolean;
		} => {
			if (node.type === 'file') {
				const isSkipped =
					node.status === 'pre_triage_skipped' || node.status === 'triage_skipped';
				return {
					count: node.count,
					topSev: node.topSeverity,
					skipped: isSkipped ? 1 : 0,
					allSkipped: isSkipped
				};
			}
			let count = 0;
			let topSev: Severity | null = null;
			let topRank = SEVERITY_ORDER.length;
			let skipped = 0;
			let allSkipped = node.children.length > 0;
			for (const c of node.children) {
				const a = annotate(c);
				count += a.count;
				skipped += a.skipped;
				if (!a.allSkipped) allSkipped = false;
				if (a.topSev) {
					const r = severityRank(a.topSev);
					if (r < topRank) {
						topRank = r;
						topSev = a.topSev;
					}
				}
			}
			node.count = count;
			node.topSeverity = topSev;
			node.skippedCount = skipped;
			node.allSkipped = allSkipped;
			return { count, topSev, skipped, allSkipped };
		};
		for (const c of rootChildren) annotate(c);

		return rootChildren;
	}

	let fileTree = $derived.by(() => nestFiles(buildFileNodes()));

	// Evict expanded-folder entries that no longer exist in the tree. Without
	// this, paths from a prior scan accumulate and the Set grows unbounded.
	// Skip while scanning so we don't yank state mid-flight.
	$effect(() => {
		if (scanning) return;
		const alive = new Set<string>();
		const visit = (children: TreeNode[]) => {
			for (const c of children) {
				if (c.type === 'folder') {
					alive.add(c.path);
					visit(c.children);
				}
			}
		};
		visit(fileTree);
		let changed = false;
		const next = new Set<string>();
		for (const p of expandedFolders) {
			if (alive.has(p)) next.add(p);
			else changed = true;
		}
		if (changed) expandedFolders = next;
	});

	type VisibleRow = { node: TreeNode; depth: number };
	let visibleTree = $derived.by(() => {
		const out: VisibleRow[] = [];
		const walkTree = (children: TreeNode[], depth: number) => {
			for (const c of children) {
				out.push({ node: c, depth });
				if (c.type === 'folder' && expandedFolders.has(c.path)) {
					walkTree(c.children, depth + 1);
				}
			}
		};
		walkTree(fileTree, 0);
		return out;
	});

	let totalFileNodes = $derived.by(() => {
		let n = 0;
		const count = (children: TreeNode[]) => {
			for (const c of children) {
				if (c.type === 'file') n++;
				else count(c.children);
			}
		};
		count(fileTree);
		return n;
	});

	/** The currently-selected file's metadata, or null when nothing is
	 *  selected. Used by the panes to render a skip explanation when the
	 *  user clicks a skipped/errored file. */
	let selectedFileNode = $derived.by<FileNode | null>(() => {
		if (!selectedFile) return null;
		let found: FileNode | null = null;
		const walkTree = (children: TreeNode[]) => {
			for (const c of children) {
				if (found) return;
				if (c.type === 'file') {
					if (c.path === selectedFile) found = c;
				} else {
					walkTree(c.children);
				}
			}
		};
		walkTree(fileTree);
		return found;
	});

	function priorityChipClass(p: Priority | null): string {
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

	/** Compact "12.4k" / "3.2M" style for token counts. Returns the raw number
	 *  when below 1k since precision matters at small scales. */
	function compactTokens(n: number): string {
		if (n < 1000) return n.toString();
		if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
		return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 1 : 0)}M`;
	}

	/** Format a millisecond duration as `12ms` / `3.4s` / `1m 23s` /
	 *  `1h 5m`. Below a second the precision matters; above an hour the
	 *  seconds become noise. */
	function formatDuration(ms: number): string {
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

	let totalTokens = $derived(
		usage.total.input_tokens + usage.total.output_tokens + usage.total.cache_read_input_tokens
	);

	let usageRows = $derived.by(() => [
		{ name: 'triage', u: usage.triage, ms: durations.triage_ms },
		{ name: 'detect', u: usage.detect, ms: durations.detect_ms },
		{ name: 'verify', u: usage.verify, ms: durations.verify_ms },
		{ name: 'patch', u: usage.patch, ms: durations.patch_ms }
	]);

	type StageSlot = { key: string; label: string; model: string | null };
	const PIPELINE_STAGES: StageSlot[] = [
		{ key: 'ingest', label: 'Ingest', model: null },
		{ key: 'triage', label: 'Triage', model: 'Haiku' },
		{ key: 'detect', label: 'Detect', model: 'Sonnet' },
		{ key: 'verify', label: 'Verify', model: 'Opus' },
		{ key: 'patch', label: 'Patch', model: 'Sonnet' }
	];

	/** Current stage as an index into PIPELINE_STAGES. -1 = not started,
	 *  5 = all complete. */
	let stageIndex = $derived.by(() => {
		if (stage === 'idle' || stage === 'starting…') return -1;
		if (stage === 'scanning…') return 0;
		if (stage.startsWith('triaging')) return 1;
		if (stage.startsWith('detecting')) return 2;
		if (stage.startsWith('verifying')) return 3;
		if (stage.startsWith('proposing')) return 4;
		if (stage === 'done' || stage === 'cancelled') return 5;
		return -1;
	});

	// ---------- triage actions ------------------------------------------
	const SNOOZE_DAYS = 7;

	async function applyTriage(
		findingId: string,
		status: TriageStatus,
		reason?: string
	) {
		if (!root) return;
		triageBusy = true;
		try {
			const snoozeUntil =
				status === 'snoozed' ? Date.now() + SNOOZE_DAYS * 24 * 60 * 60 * 1000 : undefined;
			await setTriage(findingId, root, status, reason, snoozeUntil);
			const m = new Map(triageById);
			m.set(findingId, {
				finding_id: findingId,
				status,
				reason: reason ?? null,
				snooze_until: snoozeUntil ?? null,
				updated_at: Date.now()
			});
			triageById = m;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			triageBusy = false;
		}
	}

	async function clearTriageFor(findingId: string) {
		if (!root) return;
		triageBusy = true;
		try {
			await clearTriage(findingId, root);
			const m = new Map(triageById);
			m.delete(findingId);
			triageById = m;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			triageBusy = false;
		}
	}

	function startDismiss(findingId: string) {
		dismissDraftFor = findingId;
		dismissReason = triageById.get(findingId)?.reason ?? '';
	}

	function cancelDismiss() {
		dismissDraftFor = null;
		dismissReason = '';
	}

	async function submitDismiss(findingId: string) {
		const r = dismissReason.trim();
		if (!r) return;
		await applyTriage(findingId, 'dismissed', r);
		dismissDraftFor = null;
		dismissReason = '';
	}


	function priorityChipLabel(p: Priority | null): string {
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

	let totals = $derived.by(() => {
		const c: Record<FindingStatus, number> = {
			open: 0,
			patched: 0,
			accepted: 0,
			snoozed: 0,
			dismissed: 0,
			dropped: 0,
			pending: 0,
			verifying: 0
		};
		for (const { f } of allFindings) c[findingStatus(f)]++;
		return { ...c, total: allFindings.length };
	});

	let severityCounts = $derived.by(() => {
		const c: Record<Severity, number> = { critical: 0, high: 0, medium: 0, low: 0, info: 0 };
		for (const { f } of allFindings) c[f.severity]++;
		return c;
	});

	let triageFunnel = $derived.by(() => {
		let high = 0;
		let normal = 0;
		let low = 0;
		let skip = 0;
		for (const t of triaged) {
			if (t.result.priority === 'high') high++;
			else if (t.result.priority === 'normal') normal++;
			else if (t.result.priority === 'low') low++;
			else skip++;
		}
		return { high, normal, low, skip };
	});

	function selectFile(rel: string | null) {
		selectedFile = rel;
		selectedFindingId = null;
	}

	function selectFinding(id: string) {
		selectedFindingId = id;
	}

	/** Arrow-key navigation through the visible findings. Ignored when the
	 *  user is typing into an input/textarea. */
	function handleKeydown(e: KeyboardEvent) {
		const target = e.target as HTMLElement | null;
		if (target && /^(input|textarea)$/i.test(target.tagName)) return;
		const xs = visibleFindings;
		if (xs.length === 0) return;
		if (e.key === 'ArrowDown' || e.key === 'j') {
			e.preventDefault();
			const idx = xs.findIndex((x) => x.f.id === selectedFindingId);
			const next = idx < 0 ? 0 : Math.min(idx + 1, xs.length - 1);
			selectedFindingId = xs[next].f.id;
			scrollFindingIntoView(selectedFindingId);
		} else if (e.key === 'ArrowUp' || e.key === 'k') {
			e.preventDefault();
			const idx = xs.findIndex((x) => x.f.id === selectedFindingId);
			const prev = idx < 0 ? xs.length - 1 : Math.max(idx - 1, 0);
			selectedFindingId = xs[prev].f.id;
			scrollFindingIntoView(selectedFindingId);
		} else if (e.key === 'Escape') {
			selectedFindingId = null;
		}
	}

	function scrollFindingIntoView(id: string) {
		queueMicrotask(() => {
			const el = document.querySelector<HTMLElement>(
				`[data-finding-id="${CSS.escape(id)}"]`
			);
			el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
		});
	}

	function diffLineClass(line: string): string {
		if (line.startsWith('+++') || line.startsWith('---')) return 'text-zinc-500';
		if (line.startsWith('+')) return 'bg-green-500/10 text-green-700 dark:text-green-300';
		if (line.startsWith('-')) return 'bg-red-500/10 text-red-700 dark:text-red-300';
		if (line.startsWith('@@')) return 'text-blue-600 dark:text-blue-300';
		return 'text-zinc-600 dark:text-zinc-300';
	}

	/** Split a data-flow narrative on arrows ("→" or "->") into discrete steps. */
	function parseDataFlow(text: string): string[] {
		const parts = text
			.split(/\s*(?:→|->)\s*/g)
			.map((s) => s.trim())
			.filter(Boolean);
		// If the model returned one giant blob with no arrows, fall back to the
		// original string as a single step.
		return parts.length > 0 ? parts : [text];
	}

	let dataFlowSteps = $derived.by(() =>
		selectedFinding ? parseDataFlow(selectedFinding.data_flow) : []
	);

	/** Shiki-highlighted HTML for the currently-selected patch diff, kept in
	 *  sync via an effect (shiki is async — we can't $derive directly on it). */
	let diffHtml = $state<string | null>(null);
	$effect(() => {
		const diff = selectedPatch?.diff ?? null;
		if (!diff) {
			diffHtml = null;
			return;
		}
		let cancelled = false;
		// Shiki emits both light + dark palettes inline; the .dark class
		// override in layout.css swaps them at runtime, so we don't depend on
		// theme.resolved here.
		highlightDiff(diff)
			.then((html) => {
				if (!cancelled) diffHtml = html;
			})
			.catch((e) => {
				console.error('highlightDiff failed', e);
				if (!cancelled) diffHtml = null;
			});
		return () => {
			cancelled = true;
		};
	});

	/** Code excerpt (enclosing function/class or ±N line window) for the
	 *  currently-selected finding, fetched on demand from the Rust side
	 *  via tree-sitter and highlighted with Shiki. */
	let excerpt = $state<Excerpt | null>(null);
	let excerptHtml = $state<string | null>(null);
	let excerptError = $state<string | null>(null);
	// Reset any per-finding action errors when the user moves focus to a
	// different finding. Errors are scoped to a single attempt; carrying them
	// across selections just confuses people.
	$effect(() => {
		void selectedFindingId;
		applyError = null;
		regenError = null;
	});
	$effect(() => {
		const f = selectedFinding;
		if (!f) {
			excerpt = null;
			excerptHtml = null;
			excerptError = null;
			return;
		}
		let cancelled = false;
		excerptError = null;
		getExcerpt(f.file, f.line_start, f.line_end)
			.then((ex) => {
				if (cancelled) return;
				excerpt = ex;
				return highlightCode(ex.text, ex.language);
			})
			.then((html) => {
				if (!cancelled && html !== undefined) excerptHtml = html;
			})
			.catch((e) => {
				if (cancelled) return;
				excerpt = null;
				excerptHtml = null;
				excerptError = e instanceof Error ? e.message : String(e);
			});
		return () => {
			cancelled = true;
		};
	});
</script>

<svelte:head>
	<title>open-sec</title>
</svelte:head>

<svelte:window onkeydown={handleKeydown} />

{#if view === 'launcher'}
	<Launcher onOpenFresh={openProjectFresh} onOpenPast={openProjectPast} />
{:else}

<div class="flex h-screen flex-col">
	<!-- Top bar -->
	<header class="border-border flex items-center gap-3 border-b px-4 py-2">
		<button
			type="button"
			onclick={backToLauncher}
			class="hover:bg-muted text-muted-foreground hover:text-foreground inline-flex h-7 w-7 items-center justify-center rounded transition-colors"
			title="Back to start"
			aria-label="Back to start"
		>
			<svg
				xmlns="http://www.w3.org/2000/svg"
				width="14"
				height="14"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				<path d="m15 18-6-6 6-6" />
			</svg>
		</button>
		<img src={logo} alt="" width="20" height="20" class="rounded-[5px]" />
		<h1 class="text-base font-semibold tracking-tight">Open Security</h1>
		<div class="bg-border h-5 w-px"></div>
		<div class="text-foreground/80 flex flex-1 items-center gap-2 truncate font-mono text-xs">
			<span class="truncate" title={root}>{root}</span>
		</div>
		{#if scanning}
			<Button size="sm" variant="outline" onclick={requestCancel} disabled={cancelling}>
				{cancelling ? 'Cancelling…' : 'Cancel'}
			</Button>
		{:else}
			<Button size="sm" onclick={runScan} disabled={!root || !keyConfigured}>
				{scanResult || stage === 'done' || stage === 'cancelled' ? 'Re-scan' : 'Scan'}
			</Button>
		{/if}
		{#if scanResult || resultRoot}
			<div class="relative" bind:this={exportMenuRef}>
				<Button size="sm" variant="outline" onclick={() => (exportMenuOpen = !exportMenuOpen)} aria-haspopup="menu" aria-expanded={exportMenuOpen}>
					Export
					<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="ml-1"><path d="m6 9 6 6 6-6"/></svg>
				</Button>
				{#if exportMenuOpen}
					<div class="border-border bg-popover text-popover-foreground absolute right-0 top-full z-10 mt-1 w-48 overflow-hidden rounded-md border shadow-md" role="menu">
						<button type="button" role="menuitem" class="hover:bg-muted block w-full px-3 py-2 text-left text-xs" onclick={() => { exportMenuOpen = false; exportTo('markdown'); }}>
							<div class="font-medium">Markdown</div>
							<div class="text-muted-foreground">.md report</div>
						</button>
						<button type="button" role="menuitem" class="hover:bg-muted block w-full px-3 py-2 text-left text-xs" onclick={() => { exportMenuOpen = false; exportPdf(); }}>
							<div class="font-medium">PDF</div>
							<div class="text-muted-foreground">Print to file</div>
						</button>
						<button type="button" role="menuitem" class="hover:bg-muted block w-full px-3 py-2 text-left text-xs" onclick={() => { exportMenuOpen = false; exportTo('sarif'); }}>
							<div class="font-medium">SARIF</div>
							<div class="text-muted-foreground">For CI</div>
						</button>
					</div>
				{/if}
			</div>
		{/if}
		<div class="text-muted-foreground flex items-center gap-2 text-xs">
			<button
				type="button"
				onclick={() => (settingsOpen = true)}
				class="hover:bg-muted text-muted-foreground hover:text-foreground inline-flex h-7 w-7 items-center justify-center rounded transition-colors"
				title="Settings"
				aria-label="Settings"
			>
				<svg
					xmlns="http://www.w3.org/2000/svg"
					width="14"
					height="14"
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					stroke-linejoin="round"
				>
					<circle cx="12" cy="12" r="3" />
					<path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h0a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
				</svg>
			</button>
			<ThemeToggle />
		</div>
	</header>

	{#if !keyConfigured}
		<div class="border-border bg-amber-50/40 dark:bg-amber-950/20 border-b px-4 py-3">
			<div class="flex items-center gap-2">
				<span class="text-sm font-medium">Anthropic API key required</span>
				<Input
					type="password"
					bind:value={keyInput}
					placeholder="sk-ant-…"
					autocomplete="off"
					class="h-8 max-w-md text-xs"
				/>
				<Button size="sm" onclick={saveKey} disabled={savingKey || !keyInput.trim()}>
					{savingKey ? 'Saving…' : 'Save to keychain'}
				</Button>
			</div>
		</div>
	{/if}

	{#if error}
		{@const h = humanizeError(error)}
		<div class="border-destructive/40 bg-destructive/5 border-b px-4 py-2 text-xs">
			<div class="flex items-baseline gap-2">
				<span class="text-destructive font-medium">{h.title}</span>
				{#if h.detail}
					<span class="text-destructive/80">— {h.detail}</span>
				{/if}
			</div>
		</div>
	{/if}

	{#if scanning || scanResult || stage === 'done' || stage === 'cancelled'}
		<div class="border-border bg-muted/20 flex items-center gap-3 border-b px-4 py-2">
			<ol class="flex flex-1 items-center gap-1">
				{#each PIPELINE_STAGES as s, i (s.key)}
					{@const state = stageIndex === 5
						? 'done'
						: i < stageIndex
							? 'done'
							: i === stageIndex
								? 'active'
								: 'pending'}
					<li class="flex items-center gap-1">
						<div class="flex items-center gap-1.5 rounded px-2 py-1 {state === 'active' ? 'bg-foreground text-background' : state === 'done' ? 'text-foreground' : 'text-muted-foreground/60'}">
							<span class="flex h-3.5 w-3.5 shrink-0 items-center justify-center">
								{#if state === 'done'}
									<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>
								{:else if state === 'active'}
									<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="animate-spin" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-6.2-8.55"/></svg>
								{:else}
									<span class="font-mono text-[0.625rem]">{i + 1}</span>
								{/if}
							</span>
							<span class="text-xs font-medium">{s.label}</span>
						</div>
						{#if i < PIPELINE_STAGES.length - 1}
							<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-muted-foreground/30"><path d="m9 18 6-6-6-6"/></svg>
						{/if}
					</li>
				{/each}
			</ol>
			{#if rateLimitNotice}
				<span class="shrink-0 inline-flex items-center gap-1 rounded bg-amber-500/15 px-2 py-0.5 font-mono text-xs text-amber-700 dark:text-amber-300" title="Anthropic rate limit; auto-retrying">
					<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="animate-spin" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-6.2-8.55"/></svg>
					rate-limited · retry #{rateLimitNotice.attempt} in {rateLimitNotice.retry_after_secs}s
				</span>
			{/if}
			{#if durations.total_ms > 0}
				<span class="text-muted-foreground shrink-0 font-mono text-xs" title="Total scan duration">
					{formatDuration(durations.total_ms)}
				</span>
			{/if}
			<span class="text-muted-foreground shrink-0 font-mono text-xs">{stage}</span>
		</div>
	{/if}

	{#if !scanResult && !scanning && stage === 'idle'}
		<!-- Onboarding: shown after a folder is picked, before the first scan -->
		<div class="flex flex-1 items-center justify-center overflow-y-auto px-8 py-10">
			<div class="flex w-full max-w-3xl flex-col gap-6">
				<div class="space-y-1.5">
					<h2 class="text-xl font-semibold tracking-tight">Ready to scan</h2>
					<p class="text-muted-foreground text-sm">
						An AI pipeline reads this folder and drafts patches. Nothing touches disk until you approve.
					</p>
				</div>

				<div class="border-border bg-muted/30 flex items-center gap-3 rounded-md border px-3.5 py-2.5">
					<svg
						xmlns="http://www.w3.org/2000/svg"
						width="14"
						height="14"
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
						class="text-muted-foreground shrink-0"
					>
						<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
					</svg>
					<span class="truncate font-mono text-xs" title={root}>{root || '—'}</span>
				</div>

				<section class="space-y-2.5">
					<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
						Pipeline
					</h3>
					<div class="flex items-stretch gap-1.5">
						{#each [
							{ n: '1', name: 'Ingest', model: null, desc: 'Walk & filter' },
							{ n: '2', name: 'Triage', model: 'Haiku', desc: 'Prioritize' },
							{ n: '3', name: 'Detect', model: 'Sonnet', desc: 'Find issues' },
							{ n: '4', name: 'Verify', model: 'Opus', desc: 'Confirm exploits' },
							{ n: '5', name: 'Patch', model: 'Sonnet', desc: 'Draft fixes' }
						] as step, i (step.n)}
							<div class="border-border bg-background flex flex-1 flex-col gap-1 rounded-md border px-3 py-2.5">
								<div class="flex items-center justify-between">
									<span class="text-muted-foreground/70 font-mono text-[0.625rem]">{step.n}</span>
									{#if step.model}
										<span class="text-muted-foreground/70 font-mono text-[0.5625rem] uppercase tracking-wider">
											{step.model}
										</span>
									{/if}
								</div>
								<div class="text-sm font-medium">{step.name}</div>
								<div class="text-muted-foreground text-[0.6875rem]">{step.desc}</div>
							</div>
							{#if i < 4}
								<div class="text-muted-foreground/50 flex items-center">
									<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>
								</div>
							{/if}
						{/each}
					</div>
				</section>

				<div class="space-y-2">
					<Button
						size="lg"
						onclick={runScan}
						disabled={!root || !keyConfigured}
						class="w-full"
					>
						<svg
							xmlns="http://www.w3.org/2000/svg"
							width="14"
							height="14"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							class="mr-2"
						>
							<polygon points="6 3 20 12 6 21 6 3" />
						</svg>
						Start scan
					</Button>
					<p class="text-muted-foreground text-center text-xs">
						{#if !keyConfigured}
							Add your Anthropic API key above to enable scanning.
						{:else}
							Typically a few cents and under a minute for a small project.
						{/if}
					</p>
				</div>
			</div>
		</div>
	{:else}
	<!-- Three panes -->
	<div class="grid flex-1 grid-cols-[260px_minmax(320px,1fr)_minmax(400px,1.4fr)] overflow-hidden">
		<!-- Left: file tree -->
		<aside class="border-border flex flex-col overflow-hidden border-r">
			<div class="border-border flex items-center justify-between border-b px-3 py-2">
				<span class="text-muted-foreground text-xs font-medium uppercase tracking-wide">
					Files
				</span>
				<span class="text-muted-foreground text-xs">{totalFileNodes}</span>
			</div>
			<div class="flex-1 overflow-y-auto">
				{#if totalFileNodes === 0 && !scanning}
					{#if stage === 'done' && walk && walk.candidates.length === 0}
						<p class="text-muted-foreground px-3 py-3 text-xs">
							No scannable files in this folder.
						</p>
					{:else}
						<p class="text-muted-foreground px-3 py-3 text-xs">
							Hit Scan to see the file tree.
						</p>
					{/if}
				{:else if totalFileNodes === 0 && scanning}
					<p class="text-muted-foreground animate-pulse px-3 py-3 text-xs">{stage}</p>
				{:else}
					<button
						type="button"
						class="hover:bg-muted/50 flex w-full items-center justify-between px-3 py-1.5 text-left text-xs {selectedFile === null ? 'bg-muted' : ''}"
						onclick={() => selectFile(null)}
					>
						<span class="font-medium">All files</span>
						<span class="text-muted-foreground">{totals.total}</span>
					</button>
					{#each visibleTree as row (row.node.path)}
						{#if row.node.type === 'folder'}
							{@const f = row.node}
							{@const expanded = expandedFolders.has(f.path)}
							<button
								type="button"
								class="hover:bg-muted/50 flex w-full items-center gap-1 py-1 pr-3 text-left {f.allSkipped
									? 'opacity-50'
									: ''}"
								style="padding-left: {0.5 + row.depth * 0.75}rem"
								onclick={() => toggleFolder(f.path)}
								title={f.allSkipped
									? `All ${f.skippedCount} file(s) skipped`
									: ''}
							>
								<span
									class="text-muted-foreground inline-flex h-3 w-3 shrink-0 items-center justify-center transition-transform"
									style={expanded ? 'transform: rotate(90deg)' : ''}
								>
									<svg
										xmlns="http://www.w3.org/2000/svg"
										width="10"
										height="10"
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="2.5"
										stroke-linecap="round"
										stroke-linejoin="round"
									>
										<path d="m9 18 6-6-6-6" />
									</svg>
								</span>
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="12"
									height="12"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
									class="text-muted-foreground/80 shrink-0"
								>
									<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
								</svg>
								<span
									class="flex-1 truncate font-mono text-xs {f.allSkipped ? 'italic' : ''}"
								>
									{f.name}
								</span>
								{#if f.topSeverity}
									<span class="h-2 w-2 shrink-0 rounded-full {severityDot(f.topSeverity)}"
									></span>
								{/if}
								{#if f.count > 0}
									<span class="text-muted-foreground text-xs tabular-nums">{f.count}</span>
								{:else if f.allSkipped}
									<span class="text-muted-foreground/70 text-[0.625rem] italic">skip</span>
								{:else if f.skippedCount > 0}
									<span
										class="text-muted-foreground/60 text-[0.625rem] italic tabular-nums"
										title="{f.skippedCount} skipped file(s) inside"
									>
										{f.skippedCount}
									</span>
								{/if}
							</button>
						{:else}
							{@const f = row.node}
							{@const isSkipped =
								f.status === 'pre_triage_skipped' || f.status === 'triage_skipped'}
							<button
								type="button"
								class="hover:bg-muted/50 flex w-full items-center gap-1.5 py-1 pr-3 text-left {selectedFile ===
								f.path
									? 'bg-muted'
									: ''} {isSkipped ? 'opacity-60' : ''}"
								style="padding-left: {0.5 + row.depth * 0.75}rem"
								onclick={() => selectFile(f.path)}
								title={f.detectError
									? `detect errored: ${f.detectError}`
									: f.skipReason
										? `skipped: ${skipReasonLabel(f.skipReason)}`
										: f.triageReason
											? `triage skip: ${f.triageReason}`
											: ''}
							>
								<!-- Indent slot to align files with folder rows (where chevron sits) -->
								<span class="inline-block w-3 shrink-0"></span>
								<!-- Priority chip (replaced by skip indicator when applicable) -->
								{#if isSkipped}
									<span
										class="text-muted-foreground/70 inline-flex h-4 w-4 shrink-0 items-center justify-center rounded bg-zinc-300/30 font-mono text-[0.625rem] italic dark:bg-zinc-700/40"
									>
										S
									</span>
								{:else}
									<span
										class="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded font-mono text-[0.625rem] font-semibold {priorityChipClass(
											f.priority
										)}"
										title="triage priority: {f.priority ?? 'unknown'}"
									>
										{priorityChipLabel(f.priority)}
									</span>
								{/if}
								<!-- Status indicator -->
								{#if f.status === 'errored'}
									<span class="text-destructive shrink-0">
										<svg
											xmlns="http://www.w3.org/2000/svg"
											width="10"
											height="10"
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="2.5"
											stroke-linecap="round"
											stroke-linejoin="round"
										>
											<circle cx="12" cy="12" r="10" />
											<line x1="12" y1="8" x2="12" y2="12" />
											<line x1="12" y1="16" x2="12.01" y2="16" />
										</svg>
									</span>
								{:else if f.topSeverity}
									<span class="h-2 w-2 shrink-0 rounded-full {severityDot(f.topSeverity)}"
									></span>
								{:else}
									<span class="h-2 w-2 shrink-0 rounded-full bg-zinc-200 dark:bg-zinc-700"
									></span>
								{/if}
								<span class="flex-1 truncate font-mono text-xs">{f.name}</span>
								{#if f.count > 0}
									<span class="text-muted-foreground text-xs tabular-nums">{f.count}</span>
								{:else if f.skipReason}
									<span class="text-muted-foreground/70 text-[0.625rem] italic">
										{skipReasonLabel(f.skipReason)}
									</span>
								{:else if f.status === 'triage_skipped'}
									<span class="text-muted-foreground/70 text-[0.625rem] italic">skip</span>
								{/if}
							</button>
						{/if}
					{/each}
				{/if}
			</div>

			{#if triaged.length > 0}
				<div class="border-border text-muted-foreground border-t px-3 py-2 text-[0.6875rem] leading-relaxed">
					<span class="font-medium">Priority</span>
					<div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 font-mono">
						<span><span class="text-orange-600 dark:text-orange-300">H</span> high</span>
						<span><span class="text-zinc-600 dark:text-zinc-300">N</span> normal</span>
						<span><span class="text-blue-600 dark:text-blue-300">L</span> low</span>
						<span><span class="text-zinc-500 italic">S</span> skip</span>
					</div>
				</div>
			{/if}
		</aside>

		<!-- Middle: findings -->
		<section class="border-border flex flex-col overflow-hidden border-r">
			<div class="border-border flex items-center justify-between border-b px-3 py-2">
				<span class="text-muted-foreground text-xs font-medium uppercase tracking-wide">
					Findings{selectedFile ? ` · ${selectedFile}` : ''}
				</span>
				<span class="text-muted-foreground text-xs">
					{visibleFindings.length}{filter && allFindings.length !== visibleFindings.length
						? ` / ${allFindings.length}`
						: ''}
				</span>
			</div>
			{#if allFindings.length > 0 || filter}
				<div class="border-border space-y-1.5 border-b px-3 py-2">
					<Input
						bind:value={filter}
						placeholder="Filter by title, CWE, file…"
						spellcheck={false}
						class="h-7 text-xs"
					/>
					{#if dismissedCount > 0}
						<label class="text-muted-foreground flex items-center gap-1.5 text-xs">
							<input
								type="checkbox"
								bind:checked={hideDismissed}
								class="h-3 w-3 accent-current"
							/>
							Hide dismissed ({dismissedCount})
						</label>
					{/if}
				</div>
			{/if}
			<div class="flex-1 overflow-y-auto">
				{#if visibleFindings.length === 0}
					{#if scanning}
						<div class="space-y-1 px-4 py-6 text-xs">
							<p class="text-muted-foreground animate-pulse">
								{stage}
							</p>
							{#if walk}
								<p class="text-muted-foreground/70">
									Findings will appear here as each file is scanned.
								</p>
							{/if}
						</div>
					{:else if filter}
						<div class="space-y-1 px-4 py-6 text-xs">
							<p>No findings match <span class="font-mono">&quot;{filter}&quot;</span>.</p>
							<button
								type="button"
								class="text-muted-foreground hover:text-foreground underline underline-offset-2"
								onclick={() => (filter = '')}
							>
								Clear filter
							</button>
						</div>
					{:else if selectedFileNode && (selectedFileNode.status === 'pre_triage_skipped' || selectedFileNode.status === 'triage_skipped' || selectedFileNode.status === 'errored')}
						<div class="space-y-2 px-4 py-6 text-xs">
							{#if selectedFileNode.status === 'pre_triage_skipped'}
								<p>
									Pre-triage skip ·
									<span class="text-muted-foreground">
										{skipReasonLabel(selectedFileNode.skipReason!)}
									</span>
								</p>
								<p class="text-muted-foreground/80 leading-relaxed">
									This file never reached the LLM. See detail pane for context.
								</p>
							{:else if selectedFileNode.status === 'triage_skipped'}
								<p>Triage skipped this file.</p>
								<p class="text-muted-foreground/80 leading-relaxed">
									Haiku triage decided this file has no security surface. See detail
									pane for its reason.
								</p>
							{:else}
								<p class="text-destructive">Detect errored on this file.</p>
								<p class="text-muted-foreground/80 leading-relaxed">
									See detail pane for the error message.
								</p>
							{/if}
						</div>
					{:else if stage === 'done' && allFindings.length === 0}
						{#if detectErrors.size > 0}
							<div class="space-y-3 px-4 py-6 text-sm">
								<div class="space-y-2 text-center">
									<div class="text-destructive text-2xl leading-none">!</div>
									<p class="font-medium">Scan finished with errors</p>
									<p class="text-muted-foreground text-xs leading-relaxed">
										{detectErrors.size} of {walk?.candidates.length ?? 0} file(s) couldn't be scanned. No findings produced.
									</p>
								</div>
								<ul class="border-border max-h-72 space-y-1 overflow-y-auto rounded-md border p-1">
									{#each Array.from(detectErrors) as [rel, msg] (rel)}
										{@const h = humanizeError(msg)}
										<li class="hover:bg-muted/40 rounded px-2 py-2">
											<button
												type="button"
												class="text-primary block w-full text-left text-xs hover:underline"
												onclick={() => selectFile(rel)}
												title={rel}
											>
												<span class="font-mono">{rel}</span>
											</button>
											<div class="mt-1 text-xs">{h.title}</div>
											{#if h.detail}
												<div class="text-muted-foreground mt-0.5 text-[0.6875rem] leading-relaxed">
													{h.detail}
												</div>
											{/if}
										</li>
									{/each}
								</ul>
							</div>
						{:else}
							<div class="space-y-2 px-4 py-8 text-center text-sm">
								<div class="text-2xl">✓</div>
								<p class="font-medium">Clean scan</p>
								<p class="text-muted-foreground text-xs leading-relaxed">
									No vulnerabilities found in
									{walk?.candidates.length ?? 0} file(s).
								</p>
							</div>
						{/if}
					{:else if stage === 'done' && selectedFile}
						<p class="text-muted-foreground px-4 py-4 text-xs">
							No findings in <span class="font-mono">{selectedFile}</span>.
						</p>
					{:else}
						<p class="text-muted-foreground px-4 py-4 text-xs">
							Pick a folder above and hit Scan to see findings here.
						</p>
					{/if}
				{:else}
					{#each visibleFindings as { f, rel } (f.id)}
						{@const status = findingStatus(f)}
						<button
							type="button"
							data-finding-id={f.id}
							class="hover:bg-muted/40 border-border block w-full border-b px-4 py-3 text-left {selectedFindingId === f.id ? 'bg-muted/60' : ''} {status === 'dismissed' || status === 'dropped' ? 'opacity-60' : ''}"
							onclick={() => selectFinding(f.id)}
						>
							<div class="flex items-start justify-between gap-2">
								<div class="flex-1 space-y-1">
									<div class="flex flex-wrap items-center gap-1.5">
										<Badge class={severityClass(f.severity)}>{f.severity}</Badge>
										<Badge class={statusClass(status)}>{statusLabelFor(f)}</Badge>
										<span class="text-muted-foreground font-mono text-xs">{f.cwe}</span>
										{#if f.owasp}
											<span class="text-muted-foreground font-mono text-xs">
												· {f.owasp}
											</span>
										{/if}
									</div>
									<div class="text-sm font-medium">{f.title}</div>
									<div class="text-muted-foreground font-mono text-xs">
										{rel}:{f.line_start}{f.line_end !== f.line_start ? `-${f.line_end}` : ''}
									</div>
								</div>
							</div>
						</button>
					{/each}
				{/if}
			</div>
		</section>

		<!-- Right: detail or summary -->
		<section class="flex flex-col overflow-hidden">
			<div class="border-border flex items-center border-b px-3 py-2">
				<span class="text-muted-foreground text-xs font-medium uppercase tracking-wide">
					{selectedFinding
						? 'Finding detail'
						: selectedFileNode &&
						    (selectedFileNode.status === 'pre_triage_skipped' ||
						        selectedFileNode.status === 'triage_skipped' ||
						        selectedFileNode.status === 'errored')
						  ? 'File status'
						  : 'Summary'}
				</span>
			</div>
			<div class="flex-1 overflow-y-auto">
				{#if selectedFinding}
					<article class="divide-border divide-y">
						<!-- Header -->
						<header class="space-y-2 px-5 py-4">
							<div class="flex flex-wrap items-center gap-1.5">
								<Badge class={severityClass(selectedFinding.severity)}>
									{selectedFinding.severity}
								</Badge>
								<Badge class={statusClass(findingStatus(selectedFinding))}>
									{statusLabelFor(selectedFinding)}
								</Badge>
								<Badge variant="outline">{selectedFinding.kind}</Badge>
								<span class="text-muted-foreground font-mono text-xs">
									{selectedFinding.cwe}
								</span>
								{#if selectedFinding.owasp}
									<span class="text-muted-foreground font-mono text-xs">
										· OWASP {selectedFinding.owasp}
									</span>
								{/if}
							</div>
							<h2 class="text-base font-semibold leading-snug tracking-tight">
								{selectedFinding.title}
							</h2>
							<p class="text-muted-foreground break-all font-mono text-xs">
								{selectedFinding.file}:{selectedFinding.line_start}{selectedFinding.line_end !==
								selectedFinding.line_start
									? `-${selectedFinding.line_end}`
									: ''}
							</p>

							<!-- Triage actions -->
							{#if dismissDraftFor === selectedFinding.id}
								<div class="border-border space-y-2 rounded-md border p-2">
									<div class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
										Reason for dismissal
									</div>
									<Input
										bind:value={dismissReason}
										placeholder="e.g. false positive: this param is server-controlled"
										class="h-8 text-xs"
										autofocus
										onkeydown={(e) => {
											if (e.key === 'Enter' && dismissReason.trim()) {
												submitDismiss(selectedFinding!.id);
											} else if (e.key === 'Escape') {
												cancelDismiss();
											}
										}}
									/>
									<div class="flex gap-2">
										<Button
											size="sm"
											onclick={() => submitDismiss(selectedFinding!.id)}
											disabled={!dismissReason.trim() || triageBusy}
										>
											Confirm dismiss
										</Button>
										<Button size="sm" variant="outline" onclick={cancelDismiss}>
											Cancel
										</Button>
									</div>
								</div>
							{:else}
								<div class="flex flex-wrap gap-2 pt-1">
									{#if triageById.get(selectedFinding.id)?.status === 'accepted'}
										<Button
											size="sm"
											variant="outline"
											onclick={() => clearTriageFor(selectedFinding!.id)}
											disabled={triageBusy}
										>
											Un-accept
										</Button>
									{:else}
										<Button
											size="sm"
											onclick={() => applyTriage(selectedFinding!.id, 'accepted')}
											disabled={triageBusy}
										>
											Accept
										</Button>
									{/if}
									{#if triageById.get(selectedFinding.id)?.status === 'dismissed'}
										<Button
											size="sm"
											variant="outline"
											onclick={() => clearTriageFor(selectedFinding!.id)}
											disabled={triageBusy}
										>
											Un-dismiss
										</Button>
									{:else}
										<Button
											size="sm"
											variant="outline"
											onclick={() => startDismiss(selectedFinding!.id)}
											disabled={triageBusy}
										>
											Dismiss…
										</Button>
									{/if}
									{#if triageById.get(selectedFinding.id)?.status === 'snoozed'}
										<Button
											size="sm"
											variant="outline"
											onclick={() => clearTriageFor(selectedFinding!.id)}
											disabled={triageBusy}
										>
											Un-snooze
										</Button>
									{:else}
										<Button
											size="sm"
											variant="outline"
											onclick={() => applyTriage(selectedFinding!.id, 'snoozed')}
											disabled={triageBusy}
										>
											Snooze {SNOOZE_DAYS}d
										</Button>
									{/if}
								</div>
								{#if triageById.get(selectedFinding.id)?.status === 'dismissed' && triageById.get(selectedFinding.id)?.reason}
									<p class="text-muted-foreground pt-1 text-xs italic">
										Reason: {triageById.get(selectedFinding.id)!.reason}
									</p>
								{/if}
							{/if}
						</header>

						<!-- Description -->
						<section class="space-y-2 px-5 py-4">
							<h3
								class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
							>
								Description
							</h3>
							<div class="md text-sm leading-relaxed">
								{@html renderMd(selectedFinding.description)}
							</div>
						</section>

						<!-- Data flow -->
						<section class="space-y-2 px-5 py-4">
							<h3
								class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
							>
								Data flow
							</h3>
							<ol class="marker:text-muted-foreground ml-5 list-decimal space-y-1 text-sm leading-relaxed marker:font-mono marker:text-xs">
								{#each dataFlowSteps as step, i (i)}
									<li class="md pl-1">{@html renderInlineMd(step)}</li>
								{/each}
							</ol>
						</section>


						<!-- Excerpt -->
						{#if excerpt && excerpt.text.trim().length > 0}
							<section class="space-y-2 px-5 py-4">
								<div class="flex items-center justify-between gap-2">
									<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
										{excerpt.source === 'enclosing_function' ? 'Enclosing function' : 'Excerpt'}
									</h3>
									<span class="text-muted-foreground font-mono text-[0.625rem]">
										L{excerpt.start_line}-{excerpt.end_line}
									</span>
								</div>
								{#if excerptHtml}
									<div class="shiki-wrap">{@html excerptHtml}</div>
								{:else}
									<pre class="border-border bg-muted/40 overflow-auto rounded-md border p-3 font-mono text-xs leading-relaxed">{excerpt.text}</pre>
								{/if}
							</section>
						{:else if excerptError}
							<section class="px-5 py-4">
								<p class="text-muted-foreground text-xs italic">
									Excerpt unavailable: {excerptError}
								</p>
							</section>
						{/if}
						<!-- Verifier -->
						{#if selectedVerdict}
							<section class="space-y-2 px-5 py-4">
								<div class="flex items-center justify-between gap-2">
									<h3
										class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
									>
										Verifier
									</h3>
									<div class="flex items-center gap-1.5 text-xs">
										{#if selectedVerdict.is_reachable}
											<Badge class="bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">
												reachable
											</Badge>
										{:else}
											<Badge class="bg-zinc-400/15 text-zinc-500">not reachable</Badge>
										{/if}
										{#if selectedVerdict.source_is_untrusted}
											<Badge class="bg-amber-500/15 text-amber-700 dark:text-amber-300">
												untrusted source
											</Badge>
										{/if}
									</div>
								</div>
								<div class="md text-muted-foreground text-sm leading-relaxed">
									{@html renderMd(selectedVerdict.reasoning)}
								</div>
							</section>

							<!-- Exploit -->
							{#if selectedVerdict.concrete_exploit}
								{@const ex = selectedVerdict.concrete_exploit}
								<section class="space-y-2 px-5 py-4">
									<div class="flex items-center justify-between gap-2">
										<h3
											class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
										>
											Exploit
										</h3>
										<Badge variant="outline" class="font-mono text-[0.625rem]">
											{ex.kind}
										</Badge>
									</div>
									<p class="md text-sm">{@html renderInlineMd(ex.expected_effect)}</p>
									<div class="bg-muted/40 space-y-1 rounded-md p-3 font-mono text-xs">
										{#if ex.request}
											<div class="flex gap-2">
												<span class="text-muted-foreground w-14 shrink-0">request</span>
												<span class="break-all">{ex.request.method} {ex.request.path}</span>
											</div>
										{/if}
										<div class="flex gap-2">
											<span class="text-muted-foreground w-14 shrink-0">payload</span>
											<span class="break-all">{ex.payload}</span>
										</div>
									</div>
								</section>
							{/if}
						{/if}

						<!-- Patch -->
						{#if selectedPatch}
							{@const applied = appliedPatchIds.has(selectedFinding.id)}
							<section class="space-y-3 px-5 py-4">
								<div class="flex items-center justify-between gap-2">
									<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
										Patch
									</h3>
									<div class="flex items-center gap-1.5">
										<Badge variant="outline" class="font-mono text-[0.625rem]">
											{selectedPatch.located.kind === 'not_found'
												? 'not located'
												: selectedPatch.located.kind}
										</Badge>
										{#if applied}
											<Badge class="bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">applied ✓</Badge>
										{/if}
									</div>
								</div>
								{#if selectedPatchVariants.length > 1}
									<div class="flex flex-wrap items-center gap-1">
										<span class="text-muted-foreground text-[0.625rem] uppercase tracking-wider mr-1">Variants</span>
										{#each selectedPatchVariants as _v, i (i)}
											<button
												type="button"
												onclick={() => selectPatchVariant(i)}
												class="inline-flex h-5 min-w-5 items-center justify-center rounded px-1.5 font-mono text-[0.625rem] font-semibold {i === selectedPatchVariantIdx ? 'bg-foreground text-background' : 'bg-muted text-muted-foreground hover:bg-muted/80'}"
											>v{i + 1}</button>
										{/each}
									</div>
								{/if}
								<div class="md text-sm leading-relaxed">
									{@html renderMd(selectedPatch.proposal.explanation)}
								</div>
								<div class="flex flex-wrap items-center gap-2">
									{#if applied}
										<Button size="sm" variant="outline" disabled>Applied to disk</Button>
										<span class="text-muted-foreground text-xs">Use git to review or revert.</span>
									{:else if selectedPatch.located.kind === 'not_found'}
										<Button size="sm" disabled>Cannot apply (not located)</Button>
									{:else}
										<Button size="sm" onclick={applySelectedPatch} disabled={applyBusy}>
											{applyBusy ? 'Applying…' : 'Apply patch'}
										</Button>
										{#if selectedPatch.located.kind === 'fuzzy'}
											<span class="text-muted-foreground text-xs italic">Fuzzy match — review the diff before applying.</span>
										{/if}
									{/if}
									<Button
										size="sm"
										variant="outline"
										onclick={regenerateAlternative}
										disabled={regenBusy || applied}
										title="Ask the patcher for a structurally different fix"
									>
										{regenBusy ? 'Generating…' : 'Try another fix'}
									</Button>
								</div>
								{#if regenError}
									<p class="text-destructive text-xs">Regenerate failed: {regenError}</p>
								{/if}
								{#if applyError}
									<p class="text-destructive text-xs">{applyError}</p>
								{/if}
								{#if selectedPatch.diff}
									{#if diffHtml}
										<div class="shiki-wrap">{@html diffHtml}</div>
									{:else}
										<pre
											class="border-border bg-muted/40 overflow-auto rounded-md border font-mono text-xs leading-relaxed">{#each selectedPatch.diff.split('\n') as line, i (i)}<div
													class="px-3 {diffLineClass(line)}"
												>{line || ' '}</div>{/each}</pre>
									{/if}
								{:else}
									<div class="text-muted-foreground text-xs italic">
										old_block not located in current file — raw proposal below.
									</div>
									<pre
										class="border-border bg-muted/40 overflow-auto rounded-md border p-3 font-mono text-xs"><span
											class="text-red-700 dark:text-red-300">- {selectedPatch.proposal
												.old_block}</span>
{'\n'}<span class="text-green-700 dark:text-green-300">+ {selectedPatch.proposal
												.new_block}</span></pre>
								{/if}
							</section>
						{:else if !scanning && verdictById.has(selectedFinding.id) && selectedVerdict?.is_reachable === false}
							<section class="px-5 py-4">
								<p class="text-muted-foreground text-xs italic">
									Dropped by verifier — no patch generated.
								</p>
							</section>
						{/if}
					</article>
				{:else if selectedFileNode && (selectedFileNode.status === 'pre_triage_skipped' || selectedFileNode.status === 'triage_skipped' || selectedFileNode.status === 'errored')}
					<article class="space-y-4 px-5 py-4">
						<header class="space-y-1">
							<div class="flex flex-wrap items-center gap-1.5">
								{#if selectedFileNode.status === 'errored'}
									<Badge class="bg-destructive text-destructive-foreground">errored</Badge>
								{:else if selectedFileNode.status === 'pre_triage_skipped'}
									<Badge class="bg-zinc-500/15 text-zinc-600 dark:text-zinc-300">pre-triage skip</Badge>
								{:else}
									<Badge class="bg-zinc-500/15 text-zinc-600 dark:text-zinc-300">triage skip</Badge>
								{/if}
							</div>
							<h2 class="text-base font-semibold leading-snug tracking-tight">
								{selectedFileNode.name}
							</h2>
							<p class="text-muted-foreground break-all font-mono text-xs">
								{selectedFileNode.path}
							</p>
						</header>

						{#if selectedFileNode.status === 'pre_triage_skipped' && selectedFileNode.skipReason}
							<section class="space-y-2">
								<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
									Reason
								</h3>
								<p class="text-sm">{skipReasonLabel(selectedFileNode.skipReason)}</p>
								<p class="text-muted-foreground text-xs leading-relaxed">
									Filtered before triage by ingest heuristics — never sent to the LLM.
									Reasons include vendor/build directories, files over 500&nbsp;KB,
									binary content (null bytes), and minified output (avg line length
									&gt; 200).
								</p>
							</section>
						{:else if selectedFileNode.status === 'triage_skipped'}
							<section class="space-y-2">
								<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
									Triage reason
								</h3>
								<div class="md text-sm leading-relaxed">
									{@html renderMd(selectedFileNode.triageReason ?? '(no reason emitted)')}
								</div>
								<p class="text-muted-foreground text-xs leading-relaxed">
									Haiku read this file and classified it as having no meaningful
									security surface — pure UI / types / config. It was not sent to
									detect.
								</p>
							</section>
						{:else if selectedFileNode.status === 'errored'}
							{@const h = humanizeError(selectedFileNode.detectError ?? '')}
							<section class="space-y-3">
								<div class="space-y-1">
									<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
										Error
									</h3>
									<p class="text-sm font-medium">{h.title}</p>
									{#if h.detail}
										<p class="text-muted-foreground text-xs leading-relaxed">{h.detail}</p>
									{/if}
								</div>
								{#if selectedFileNode.detectError}
									<details class="text-xs">
										<summary class="text-muted-foreground hover:text-foreground cursor-pointer">
											Show technical details
										</summary>
										<pre class="bg-muted/40 border-border mt-2 overflow-auto whitespace-pre-wrap rounded-md border p-3 font-mono text-[0.6875rem]">{selectedFileNode.detectError}</pre>
									</details>
								{/if}
							</section>
						{/if}
					</article>
				{:else}
					<div class="p-4">
					<div class="space-y-6">
						{#if !scanResult && !scanning}
							<div class="space-y-3 rounded-md border border-dashed p-6 text-center">
								<p class="text-muted-foreground text-xs">
									Ready to scan this folder. The pipeline runs triage → detect → verify →
									patch and persists the result.
								</p>
								<Button
									size="lg"
									onclick={runScan}
									disabled={!root || !keyConfigured}
									class="w-full"
								>
									Scan this folder
								</Button>
								{#if !keyConfigured}
									<p class="text-muted-foreground text-xs">
										Set the Anthropic API key from the start screen first.
									</p>
								{/if}
							</div>
						{/if}
						<div class="space-y-4">
							<div class="space-y-1">
								<div class="flex items-center gap-2">
									<h2 class="text-base font-medium">Scan summary</h2>
									{#if scanResult?.status === 'cancelled'}
										<Badge class="bg-amber-500/15 text-amber-700 dark:text-amber-300">cancelled</Badge>
									{/if}
								</div>
								<p class="text-muted-foreground break-all font-mono text-xs" title={root}>
									{root || '—'}
								</p>
							</div>

							<dl class="grid grid-cols-[1fr_auto] gap-x-3 text-xs">
								<dt class="text-muted-foreground py-0.5">Files scanned</dt>
								<dd class="py-0.5 text-right font-mono tabular-nums">
									{walk?.candidates.length ?? 0}{#if walk && walk.skipped.length > 0}<span class="text-muted-foreground/70"> · {walk.skipped.length} skipped</span>{/if}
								</dd>
								<dt class="text-muted-foreground py-0.5">Patches drafted</dt>
								<dd class="py-0.5 text-right font-mono tabular-nums">{patchById.size}</dd>
								{#if durations.total_ms > 0}
									<dt class="text-muted-foreground py-0.5">Duration</dt>
									<dd class="py-0.5 text-right font-mono tabular-nums">{formatDuration(durations.total_ms)}</dd>
								{/if}
							</dl>

							{#if allFindings.length > 0}
								<div class="space-y-1.5">
									<div class="flex items-baseline justify-between gap-2">
										<div class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
											Severity
										</div>
										<div class="text-muted-foreground/70 text-[0.625rem]">
											{allFindings.length} total
										</div>
									</div>
									<dl class="grid grid-cols-[1fr_auto] gap-x-3 text-xs">
										{#each [
											{ key: 'critical' as Severity, label: 'Critical' },
											{ key: 'high' as Severity, label: 'High' },
											{ key: 'medium' as Severity, label: 'Medium' },
											{ key: 'low' as Severity, label: 'Low' }
										] as row (row.key)}
											{@const n = severityCounts[row.key]}
											<dt class="flex items-center gap-2 py-0.5 {n === 0 ? 'text-muted-foreground/50' : ''}">
												<span class="h-2 w-2 shrink-0 rounded-full {severityDot(row.key)} {n === 0 ? 'opacity-30' : ''}"></span>
												{row.label}
											</dt>
											<dd class="py-0.5 text-right font-mono tabular-nums {n === 0 ? 'text-muted-foreground/50' : ''}">
												{n}
											</dd>
										{/each}
										{#if severityCounts.info > 0}
											<dt class="flex items-center gap-2 py-0.5">
												<span class="h-2 w-2 shrink-0 rounded-full {severityDot('info')}"></span>
												Info
											</dt>
											<dd class="py-0.5 text-right font-mono tabular-nums">{severityCounts.info}</dd>
										{/if}
									</dl>
								</div>
							{/if}

							<div class="space-y-1.5">
								<div class="flex items-baseline justify-between gap-2">
									<div class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
										Status
									</div>
									<div class="text-muted-foreground/70 text-[0.625rem]">
										one per finding
									</div>
								</div>
								<dl class="grid grid-cols-[1fr_auto] gap-x-3 text-xs">
									{#each [
										{ key: 'open' as FindingStatus, label: 'Open' },
										{ key: 'patched' as FindingStatus, label: 'Patched' },
										{ key: 'accepted' as FindingStatus, label: 'Accepted' },
										{ key: 'snoozed' as FindingStatus, label: 'Snoozed' },
										{ key: 'dismissed' as FindingStatus, label: 'Dismissed' },
										{ key: 'dropped' as FindingStatus, label: 'Dropped' },
										{ key: 'pending' as FindingStatus, label: 'Pending' },
										{ key: 'verifying' as FindingStatus, label: 'Verifying' }
									] as row (row.key)}
										{@const n = totals[row.key]}
										{#if n > 0 || row.key === 'open' || row.key === 'patched'}
											<dt class="flex items-center gap-2 py-0.5 {n === 0 ? 'text-muted-foreground/50' : ''}">
												<span class="h-2 w-2 shrink-0 rounded-full {statusDotClass(row.key)} {n === 0 ? 'opacity-30' : ''}"></span>
												{row.label}
											</dt>
											<dd class="py-0.5 text-right font-mono tabular-nums {n === 0 ? 'text-muted-foreground/50' : ''}">
												{n}
											</dd>
										{/if}
									{/each}
								</dl>
							</div>
						</div>

						{#if totalTokens > 0 || durations.total_ms > 0}
							<details class="group border-border overflow-hidden rounded-md border">
								<summary class="hover:bg-muted/30 flex cursor-pointer items-center justify-between px-3 py-2 [&::-webkit-details-marker]:hidden">
									<div class="flex items-center gap-2">
										<svg
											xmlns="http://www.w3.org/2000/svg"
											width="10"
											height="10"
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="2.5"
											stroke-linecap="round"
											stroke-linejoin="round"
											class="text-muted-foreground transition-transform group-open:rotate-90"
										>
											<path d="m9 18 6-6-6-6" />
										</svg>
										<span class="text-xs font-medium">Token usage</span>
									</div>
									<span class="text-muted-foreground font-mono text-xs">
										{compactTokens(totalTokens)}
									</span>
								</summary>
								<div class="border-border bg-muted/10 border-t">
									<table class="w-full text-xs">
										<thead>
											<tr class="text-muted-foreground/70 border-border/60 border-b text-[0.625rem] uppercase tracking-wider">
												<th class="px-3 py-1.5 text-left font-medium">Stage</th>
												<th class="px-2 py-1.5 text-right font-medium">In</th>
												<th class="px-2 py-1.5 text-right font-medium">Out</th>
												<th class="px-2 py-1.5 text-right font-medium">Cache</th>
												<th class="px-3 py-1.5 text-right font-medium">Time</th>
											</tr>
										</thead>
										<tbody class="font-mono">
											{#if durations.ingest_ms > 0}
												<tr class="border-border/30 border-b last:border-b-0 text-muted-foreground/80">
													<td class="px-3 py-1">ingest</td>
													<td class="px-2 py-1 text-right tabular-nums">—</td>
													<td class="px-2 py-1 text-right tabular-nums">—</td>
													<td class="px-2 py-1 text-right tabular-nums">—</td>
													<td class="px-3 py-1 text-right tabular-nums">{formatDuration(durations.ingest_ms)}</td>
												</tr>
											{/if}
											{#each usageRows as row (row.name)}
												<tr class="border-border/30 border-b last:border-b-0">
													<td class="px-3 py-1">{row.name}</td>
													<td class="px-2 py-1 text-right tabular-nums">{row.u.input_tokens.toLocaleString()}</td>
													<td class="px-2 py-1 text-right tabular-nums">{row.u.output_tokens.toLocaleString()}</td>
													<td class="text-muted-foreground px-2 py-1 text-right tabular-nums">{row.u.cache_read_input_tokens.toLocaleString()}</td>
													<td class="px-3 py-1 text-right tabular-nums">{row.ms > 0 ? formatDuration(row.ms) : '—'}</td>
												</tr>
											{/each}
										</tbody>
										<tfoot>
											<tr class="border-border/60 bg-muted/30 border-t font-semibold">
												<td class="px-3 py-1.5">Total</td>
												<td class="px-2 py-1.5 text-right tabular-nums">{usage.total.input_tokens.toLocaleString()}</td>
												<td class="px-2 py-1.5 text-right tabular-nums">{usage.total.output_tokens.toLocaleString()}</td>
												<td class="text-muted-foreground px-2 py-1.5 text-right tabular-nums">{usage.total.cache_read_input_tokens.toLocaleString()}</td>
												<td class="px-3 py-1.5 text-right tabular-nums">{durations.total_ms > 0 ? formatDuration(durations.total_ms) : '—'}</td>
											</tr>
										</tfoot>
									</table>
								</div>
							</details>
						{/if}

						{#if walk && walk.skipped.length > 0}
							<details class="text-sm">
								<summary class="text-muted-foreground cursor-pointer text-xs">
									Pre-triage skips ({walk.skipped.length})
								</summary>
								<ul class="text-muted-foreground mt-2 space-y-0.5 font-mono text-xs">
									{#each walk.skipped.slice(0, 30) as s (s.rel_path)}
										<li>
											<span class="text-amber-600">{skipReasonLabel(s.reason)}</span>
											· {s.rel_path}
										</li>
									{/each}
									{#if walk.skipped.length > 30}
										<li class="italic">… and {walk.skipped.length - 30} more</li>
									{/if}
								</ul>
							</details>
						{/if}

						{#if !scanResult && !scanning && totalFileNodes === 0}
							<p class="text-muted-foreground text-xs">
								Select a folder above and hit Scan to begin.
							</p>
						{/if}
						</div>
					</div>
				{/if}
			</div>
		</section>
	</div>
	{/if}
</div>
{/if}

{#if settingsOpen}
	<Settings onClose={() => (settingsOpen = false)} />
{/if}
