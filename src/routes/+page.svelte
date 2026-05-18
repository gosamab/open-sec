<script lang="ts">
	import { onDestroy, onMount } from 'svelte';

	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import FileTree from '$lib/components/FileTree.svelte';
	import FindingDetail from '$lib/components/FindingDetail.svelte';
	import FileStatusDetail from '$lib/components/FileStatusDetail.svelte';
	import FindingsList from '$lib/components/FindingsList.svelte';
	import Launcher from '$lib/components/Launcher.svelte';
	import ScanSummary from '$lib/components/ScanSummary.svelte';
	import Settings from '$lib/components/Settings.svelte';
	import WorkspaceTopBar from '$lib/components/WorkspaceTopBar.svelte';
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
		loadScan,
		runPipeline,
		scanFile,
		setAnthropicKey,
		setTriage,
		type Excerpt,
		type Finding,
		type Patch,
		type Priority,
		type ScanGroup,
		type ScanResult,
		type Severity,
		type StageDurations,
		type StageUsage,
		type TriagedFile,
		type TriageRecord,
		type TriageStatus,
		type Verdict,
		type VerifiedFinding
	} from '$lib/ipc';
	import {
		buildFileNodes,
		collectFolderPaths,
		countFileNodes,
		findFileNode,
		flattenTree,
		nestFiles
	} from '$lib/tree';
	import {
		DEFAULT_FINDINGS_FILTER,
		SEVERITY_ORDER,
		applyFindingsFilter,
		findingStatus,
		humanizeError,
		severityRank,
		type FindingStatus,
		type FindingStatusInputs,
		type FindingsFilter
	} from '$lib/scan-display';
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

	let walk = $state<ScanResult['ingest'] | null>(null);
	let triaged = $state<TriagedFile[]>([]);
	let findingsByFile = $state<Map<string, Finding[]>>(new Map());
	let detectErrors = $state<Map<string, string>>(new Map());
	let verdictById = $state<Map<string, Verdict | null>>(new Map());
	let patchById = $state<Map<string, Patch>>(new Map());
	let usage = $state<StageUsage>(EMPTY_STAGE_USAGE);
	let durations = $state<StageDurations>(EMPTY_STAGE_DURATIONS);
	let rateLimitNotice = $state<{ attempt: number; retry_after_secs: number } | null>(null);
	let scanResult = $state<ScanResult | null>(null);

	let triageById = $state<Map<string, TriageRecord>>(new Map());
	let triageBusy = $state(false);

	let appliedPatchIds = $state<Set<string>>(new Set());
	let applyBusy = $state(false);
	let applyError = $state<string | null>(null);

	let patchHistoryById = $state<Map<string, Patch[]>>(new Map());
	let regenBusy = $state(false);
	let regenError = $state<string | null>(null);

	let dismissDraftFor = $state<string | null>(null);
	let dismissReason = $state('');

	// Persisted UI preference — long-lived across sessions.
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
	let filterConfig = $state<FindingsFilter>({ ...DEFAULT_FINDINGS_FILTER });
	/** rel_paths currently being retried after a detect error. The summary
	 *  shows a per-row spinner; on success we drop the error and merge the
	 *  fresh findings in. Note: scan_file only re-runs DETECT — retried files
	 *  won't get verdicts/patches without a full re-scan. */
	let retryingFiles = $state<Set<string>>(new Set());

	let resultRoot = $state<string | null>(null);

	let unlisten: UnlistenFn | null = null;
	let settingsOpen = $state(false);

	const SNOOZE_DAYS = 7;

	// ---------- helpers shared between the route and children -----------
	let statusInputs: FindingStatusInputs = $derived({
		triageById,
		appliedPatchIds,
		verdictById,
		scanning
	});

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

	// ---------- patch variants ------------------------------------------
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

	// ---------- scan actions --------------------------------------------
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
		triageById = new Map();
		appliedPatchIds = new Set();
		filterConfig = { ...DEFAULT_FINDINGS_FILTER };
		try {
			scanResult = await runPipeline(root, asScanConfig(settings.value));
			resultRoot = root;
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

	/** Wipe previous-scan state when the user opens a different project. */
	$effect(() => {
		if (scanning) return;
		if (resultRoot === null) return;
		if (root === resultRoot) return;
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

	function openProjectFresh(path: string) {
		resetWorkspace();
		root = path;
		view = 'workspace';
	}

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

	/** True while a "Retry all" sweep is in progress. Disables the button so
	 *  it can't be double-fired, and lets the summary show a sweep-wide
	 *  spinner. Per-file spinners (`retryingFiles`) still mark which one is
	 *  currently in flight. */
	let retryingAll = $state(false);

	/** Re-run detect on every errored file sequentially. Sequential is the
	 *  conservative choice — a fan-out would risk hitting the per-minute
	 *  rate limit on the detect model since we're outside the orchestrator's
	 *  retry decorator here. */
	async function retryAllDetectErrors() {
		if (retryingAll) return;
		retryingAll = true;
		try {
			const rels = Array.from(detectErrors.keys());
			for (const rel of rels) {
				// Re-check the live map — a successful retry of an earlier file
				// drops its entry, but if a new error came in we shouldn't skip
				// retried ones either. Iterate the snapshot.
				if (!detectErrors.has(rel)) continue;
				await retryDetectForFile(rel);
			}
		} finally {
			retryingAll = false;
		}
	}

	/** Re-run detect on a single previously-errored file. Uses the standalone
	 *  `scan_file` IPC, which runs detect-only with the default model — no
	 *  verify/patch. If the user wants verdicts on the retried file, they'll
	 *  need a full re-scan. */
	async function retryDetectForFile(rel: string) {
		if (!root) return;
		if (retryingFiles.has(rel)) return;
		const candidate = walk?.candidates.find((c) => c.rel_path === rel);
		const absolutePath = candidate?.path ?? `${root}/${rel}`;
		const next = new Set(retryingFiles);
		next.add(rel);
		retryingFiles = next;
		try {
			const findings = await scanFile(absolutePath, root);
			const fbf = new Map(findingsByFile);
			fbf.set(rel, findings);
			findingsByFile = fbf;
			const errs = new Map(detectErrors);
			errs.delete(rel);
			detectErrors = errs;
		} catch (e) {
			const errs = new Map(detectErrors);
			errs.set(rel, e instanceof Error ? e.message : String(e));
			detectErrors = errs;
		} finally {
			const after = new Set(retryingFiles);
			after.delete(rel);
			retryingFiles = after;
		}
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

	// ---------- derived --------------------------------------------------
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
		return applyFindingsFilter(xs, filterConfig, statusInputs);
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

	// ---------- file tree ------------------------------------------------
	let expandedFolders = $state<Set<string>>(new Set());

	function toggleFolder(p: string) {
		const next = new Set(expandedFolders);
		if (next.has(p)) next.delete(p);
		else next.add(p);
		expandedFolders = next;
	}

	let fileTree = $derived.by(() =>
		nestFiles(buildFileNodes({ walk, triaged, findingsByFile, detectErrors }))
	);

	// Evict stale folder paths after a re-scan.
	$effect(() => {
		if (scanning) return;
		const alive = collectFolderPaths(fileTree);
		let changed = false;
		const next = new Set<string>();
		for (const p of expandedFolders) {
			if (alive.has(p)) next.add(p);
			else changed = true;
		}
		if (changed) expandedFolders = next;
	});

	let visibleTree = $derived.by(() => flattenTree(fileTree, expandedFolders));
	let totalFileNodes = $derived.by(() => countFileNodes(fileTree));
	let selectedFileNode = $derived.by(() =>
		selectedFile ? findFileNode(fileTree, selectedFile) : null
	);

	// ---------- stages / progress ---------------------------------------
	type StageSlot = { key: string; label: string; model: string | null };
	const PIPELINE_STAGES: StageSlot[] = [
		{ key: 'ingest', label: 'Ingest', model: null },
		{ key: 'triage', label: 'Triage', model: 'Haiku' },
		{ key: 'detect', label: 'Detect', model: 'Sonnet' },
		{ key: 'verify', label: 'Verify', model: 'Opus' },
		{ key: 'patch', label: 'Patch', model: 'Sonnet' }
	];

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

	import { formatDuration } from '$lib/scan-display';

	// ---------- summary totals ------------------------------------------
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
		for (const { f } of allFindings) c[findingStatus(f, statusInputs)]++;
		return c;
	});

	let severityCounts = $derived.by(() => {
		const c: Record<Severity, number> = { critical: 0, high: 0, medium: 0, low: 0, info: 0 };
		for (const { f } of allFindings) c[f.severity]++;
		return c;
	});

	let totalTokens = $derived(
		usage.total.input_tokens + usage.total.output_tokens + usage.total.cache_read_input_tokens
	);

	let usageRows = $derived.by(() => [
		{ name: 'triage', u: usage.triage, ms: durations.triage_ms },
		{ name: 'detect', u: usage.detect, ms: durations.detect_ms },
		{ name: 'verify', u: usage.verify, ms: durations.verify_ms },
		{ name: 'patch', u: usage.patch, ms: durations.patch_ms }
	]);

	// ---------- triage actions ------------------------------------------
	async function applyTriage(findingId: string, status: TriageStatus, reason?: string) {
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

	function selectFile(rel: string | null) {
		selectedFile = rel;
		selectedFindingId = null;
	}

	function selectFinding(id: string) {
		selectedFindingId = id;
	}

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

	// Reset per-finding action errors when the user moves to a different finding.
	$effect(() => {
		void selectedFindingId;
		applyError = null;
		regenError = null;
	});

	/** Shiki-highlighted HTML for the selected patch diff. */
	let diffHtml = $state<string | null>(null);
	$effect(() => {
		const diff = selectedPatch?.diff ?? null;
		if (!diff) {
			diffHtml = null;
			return;
		}
		let cancelled = false;
		// Shiki emits both palettes inline; CSS in layout.css flips at runtime.
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

	/** Code excerpt for the selected finding. */
	let excerpt = $state<Excerpt | null>(null);
	let excerptHtml = $state<string | null>(null);
	let excerptError = $state<string | null>(null);
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

	let humanizedError = $derived(error ? humanizeError(error) : null);
	let showProgress = $derived(
		scanning || !!scanResult || stage === 'done' || stage === 'cancelled'
	);
	let showOnboarding = $derived(!scanResult && !scanning && stage === 'idle');
	let selectedFileNodeIsStatus = $derived(
		!!selectedFileNode &&
			(selectedFileNode.status === 'pre_triage_skipped' ||
				selectedFileNode.status === 'triage_skipped' ||
				selectedFileNode.status === 'errored')
	);
</script>

<svelte:head>
	<title>open-sec</title>
</svelte:head>

<svelte:window onkeydown={handleKeydown} />

{#if view === 'launcher'}
	<Launcher onOpenFresh={openProjectFresh} onOpenPast={openProjectPast} />
{:else}
	<div class="flex h-screen flex-col">
		<WorkspaceTopBar
			{root}
			{scanning}
			{cancelling}
			{keyConfigured}
			{scanResult}
			{resultRoot}
			{stage}
			onBack={backToLauncher}
			onScan={runScan}
			onCancel={requestCancel}
			onOpenSettings={() => (settingsOpen = true)}
			onExportMarkdown={() => exportTo('markdown')}
			onExportSarif={() => exportTo('sarif')}
		/>

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

		{#if humanizedError}
			<div class="border-destructive/40 bg-destructive/5 border-b px-4 py-2 text-xs">
				<div class="flex items-baseline gap-2">
					<span class="text-destructive font-medium">{humanizedError.title}</span>
					{#if humanizedError.detail}
						<span class="text-destructive/80">— {humanizedError.detail}</span>
					{/if}
				</div>
			</div>
		{/if}

		{#if showProgress}
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
							<div
								class="flex items-center gap-1.5 rounded px-2 py-1 {state === 'active'
									? 'bg-foreground text-background'
									: state === 'done'
										? 'text-foreground'
										: 'text-muted-foreground/60'}"
							>
								<span class="flex h-3.5 w-3.5 shrink-0 items-center justify-center">
									{#if state === 'done'}
										<svg
											xmlns="http://www.w3.org/2000/svg"
											width="10"
											height="10"
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="3"
											stroke-linecap="round"
											stroke-linejoin="round"
										>
											<path d="M20 6 9 17l-5-5" />
										</svg>
									{:else if state === 'active'}
										<svg
											xmlns="http://www.w3.org/2000/svg"
											width="10"
											height="10"
											viewBox="0 0 24 24"
											fill="none"
											stroke="currentColor"
											stroke-width="2.5"
											class="animate-spin"
											stroke-linecap="round"
											stroke-linejoin="round"
										>
											<path d="M21 12a9 9 0 1 1-6.2-8.55" />
										</svg>
									{:else}
										<span class="font-mono text-[0.625rem]">{i + 1}</span>
									{/if}
								</span>
								<span class="text-xs font-medium">{s.label}</span>
							</div>
							{#if i < PIPELINE_STAGES.length - 1}
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="10"
									height="10"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
									class="text-muted-foreground/30"
								>
									<path d="m9 18 6-6-6-6" />
								</svg>
							{/if}
						</li>
					{/each}
				</ol>
				{#if rateLimitNotice}
					<span
						class="shrink-0 inline-flex items-center gap-1 rounded bg-amber-500/15 px-2 py-0.5 font-mono text-xs text-amber-700 dark:text-amber-300"
						title="Anthropic rate limit; auto-retrying"
					>
						<svg
							xmlns="http://www.w3.org/2000/svg"
							width="10"
							height="10"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2.5"
							class="animate-spin"
							stroke-linecap="round"
							stroke-linejoin="round"
						>
							<path d="M21 12a9 9 0 1 1-6.2-8.55" />
						</svg>
						rate-limited · retry #{rateLimitNotice.attempt} in {rateLimitNotice.retry_after_secs}s
					</span>
				{/if}
				{#if durations.total_ms > 0}
					<span
						class="text-muted-foreground shrink-0 font-mono text-xs"
						title="Total scan duration"
					>
						{formatDuration(durations.total_ms)}
					</span>
				{/if}
				<span class="text-muted-foreground shrink-0 font-mono text-xs">{stage}</span>
			</div>
		{/if}

		{#if showOnboarding}
			<div class="flex flex-1 items-center justify-center overflow-y-auto px-8 py-10">
				<div class="flex w-full max-w-3xl flex-col gap-6">
					<div class="space-y-1.5">
						<h2 class="text-xl font-semibold tracking-tight">Ready to scan</h2>
						<p class="text-muted-foreground text-sm">
							An AI pipeline reads this folder and drafts patches. Nothing touches disk until
							you approve.
						</p>
					</div>

					<div
						class="border-border bg-muted/30 flex items-center gap-3 rounded-md border px-3.5 py-2.5"
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
							class="text-muted-foreground shrink-0"
						>
							<path
								d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
							/>
						</svg>
						<span class="truncate font-mono text-xs" title={root}>{root || '—'}</span>
					</div>

					<section class="space-y-2.5">
						<h3
							class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
						>
							Pipeline
						</h3>
						<div class="flex items-stretch gap-1.5">
							{#each [{ n: '1', name: 'Ingest', model: null, desc: 'Walk & filter' }, { n: '2', name: 'Triage', model: 'Haiku', desc: 'Prioritize' }, { n: '3', name: 'Detect', model: 'Sonnet', desc: 'Find issues' }, { n: '4', name: 'Verify', model: 'Opus', desc: 'Confirm exploits' }, { n: '5', name: 'Patch', model: 'Sonnet', desc: 'Draft fixes' }] as step, i (step.n)}
								<div
									class="border-border bg-background flex flex-1 flex-col gap-1 rounded-md border px-3 py-2.5"
								>
									<div class="flex items-center justify-between">
										<span class="text-muted-foreground/70 font-mono text-[0.625rem]">
											{step.n}
										</span>
										{#if step.model}
											<span
												class="text-muted-foreground/70 font-mono text-[0.5625rem] uppercase tracking-wider"
											>
												{step.model}
											</span>
										{/if}
									</div>
									<div class="text-sm font-medium">{step.name}</div>
									<div class="text-muted-foreground text-[0.6875rem]">{step.desc}</div>
								</div>
								{#if i < 4}
									<div class="text-muted-foreground/50 flex items-center">
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
			<div
				class="grid flex-1 grid-cols-[260px_minmax(320px,1fr)_minmax(400px,1.4fr)] overflow-hidden"
			>
				<FileTree
					{visibleTree}
					{totalFileNodes}
					totalFindings={totals.open +
						totals.patched +
						totals.accepted +
						totals.snoozed +
						totals.dismissed +
						totals.dropped +
						totals.pending +
						totals.verifying}
					{selectedFile}
					{scanning}
					{stage}
					hasWalk={!!walk}
					walkCandidateCount={walk?.candidates.length ?? 0}
					hasTriaged={triaged.length > 0}
					{expandedFolders}
					onSelectFile={selectFile}
					onToggleFolder={toggleFolder}
				/>

				<FindingsList
					{visibleFindings}
					allFindingsCount={allFindings.length}
					bind:filter
					bind:hideDismissed
					{dismissedCount}
					bind:filterConfig
					{selectedFindingId}
					{selectedFile}
					{selectedFileNode}
					{scanning}
					{stage}
					hasWalk={!!walk}
					walkCandidateCount={walk?.candidates.length ?? 0}
					{detectErrors}
					{statusInputs}
					onSelectFinding={selectFinding}
					onSelectFile={selectFile}
				/>

				<section class="flex flex-col overflow-hidden">
					<div class="border-border flex h-10 items-center justify-between border-b px-3">
						<span
							class="text-muted-foreground text-xs font-medium uppercase tracking-wide"
						>
							{selectedFinding
								? 'Finding detail'
								: selectedFileNodeIsStatus
									? 'File status'
									: 'Summary'}
						</span>
						{#if selectedFinding || selectedFileNodeIsStatus}
							<button
								type="button"
								class="hover:bg-muted text-muted-foreground hover:text-foreground inline-flex h-6 items-center gap-1 rounded px-2 text-[0.6875rem] transition-colors"
								title="Back to summary (Esc)"
								aria-label="Back to summary"
								onclick={() => {
									selectedFindingId = null;
									if (selectedFileNodeIsStatus) selectFile(null);
								}}
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
									<path d="M18 6 6 18" />
									<path d="m6 6 12 12" />
								</svg>
								<span>Summary</span>
							</button>
						{/if}
					</div>
					<div class="flex-1 overflow-y-auto">
						{#if selectedFinding}
							<FindingDetail
								finding={selectedFinding}
								verdict={selectedVerdict}
								hasVerdictKey={verdictById.has(selectedFinding.id)}
								patch={selectedPatch}
								patchVariants={selectedPatchVariants}
								patchVariantIdx={selectedPatchVariantIdx}
								triageRecord={triageById.get(selectedFinding.id) ?? null}
								applied={appliedPatchIds.has(selectedFinding.id)}
								dismissDraftActive={dismissDraftFor === selectedFinding.id}
								bind:dismissReason
								{triageBusy}
								{applyBusy}
								{applyError}
								{regenBusy}
								{regenError}
								{excerpt}
								{excerptHtml}
								{excerptError}
								{diffHtml}
								{scanning}
								{statusInputs}
								snoozeDays={SNOOZE_DAYS}
								onApplyTriage={(status, reason) => {
									if (selectedFinding) applyTriage(selectedFinding.id, status, reason);
								}}
								onClearTriage={() => {
									if (selectedFinding) clearTriageFor(selectedFinding.id);
								}}
								onStartDismiss={() => {
									if (selectedFinding) startDismiss(selectedFinding.id);
								}}
								onCancelDismiss={cancelDismiss}
								onSubmitDismiss={() => {
									if (selectedFinding) submitDismiss(selectedFinding.id);
								}}
								onApplyPatch={applySelectedPatch}
								onRegenerate={regenerateAlternative}
								onSelectVariant={selectPatchVariant}
							/>
						{:else if selectedFileNode && selectedFileNodeIsStatus}
							<FileStatusDetail node={selectedFileNode} />
						{:else}
							<ScanSummary
								{scanResult}
								{scanning}
								{stage}
								{keyConfigured}
								{root}
								{walk}
								patchCount={patchById.size}
								allFindingsTotal={allFindings.length}
								{severityCounts}
								{totals}
								{durations}
								{usage}
								{usageRows}
								{totalTokens}
								{totalFileNodes}
								{detectErrors}
								{retryingFiles}
								{retryingAll}
								onRunScan={runScan}
								onSelectFile={selectFile}
								onRetryDetect={retryDetectForFile}
								onRetryAll={retryAllDetectErrors}
							/>
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
