<script lang="ts">
	import { onDestroy, onMount } from 'svelte';

	import Launcher from '$lib/components/Launcher.svelte';
	import Settings from '$lib/components/Settings.svelte';
	import WorkspaceView from '$lib/components/WorkspaceView.svelte';
	import { asScanConfig, settings } from '$lib/settings.svelte';
	import { highlightCode, highlightDiff } from '$lib/shiki.svelte';
	import { stageIndex } from '$lib/pipeline';
	import { scan } from '$lib/stores/scan-state.svelte';
	import { triage } from '$lib/stores/triage-state.svelte';
	import { ui } from '$lib/stores/ui-state.svelte';
	import {
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
		setTriage,
		EMPTY_STAGE_DURATIONS,
		type Excerpt,
		type Finding,
		type Patch,
		type ScanGroup,
		type ScanResult,
		type Severity,
		type TriageStatus,
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
		applyFindingsFilter,
		findingStatus,
		humanizeError,
		severityRank,
		type FindingStatus,
		type FindingStatusInputs
	} from '$lib/scan-display';
	import type { UnlistenFn } from '@tauri-apps/api/event';

	// All persistent UI / scan / triage state lives in $lib/stores. This
	// route owns: the event-stream subscription, action handlers that mutate
	// stores, the derived values WorkspaceView needs, and the few effects
	// that touch DOM/localStorage.

	let unlisten: UnlistenFn | null = null;
	const SNOOZE_DAYS = 7;

	// Persist `hideDismissed` toggle.
	$effect(() => {
		void ui.hideDismissed;
		ui.persistHideDismissed();
	});

	// ---------- helpers shared between the route and children -----------
	let statusInputs: FindingStatusInputs = $derived({
		triageById: triage.triageById,
		appliedPatchIds: triage.appliedPatchIds,
		verdictById: scan.verdictById,
		scanning: scan.scanning
	});

	// ---------- lifecycle ------------------------------------------------
	onMount(async () => {
		scan.keyConfigured = await hasAnthropicKey();
		unlisten = await listenScanEvents((ev) => {
			switch (ev.kind) {
				case 'started':
					scan.stage = 'scanning…';
					break;
				case 'ingest_complete':
					scan.walk = ev.walk;
					scan.stage = `triaging ${ev.walk.candidates.length} file(s)…`;
					break;
				case 'triage_complete':
					scan.triaged = ev.triaged;
					const keepers = ev.triaged.filter((t) => t.result.priority !== 'skip').length;
					scan.stage = keepers > 0 ? `detecting 0/${keepers} file(s)…` : 'detecting…';
					break;
				case 'detect_file_complete': {
					const next = new Map(scan.findingsByFile);
					next.set(ev.rel_path, ev.findings);
					scan.findingsByFile = next;
					const total = scan.triaged.filter((t) => t.result.priority !== 'skip').length;
					if (total > 0) scan.stage = `detecting ${next.size}/${total} file(s)…`;
					break;
				}
				case 'detect_file_errored': {
					const f = new Map(scan.findingsByFile);
					f.set(ev.rel_path, []);
					scan.findingsByFile = f;
					const e = new Map(scan.detectErrors);
					e.set(ev.rel_path, ev.error);
					scan.detectErrors = e;
					const total = scan.triaged.filter((t) => t.result.priority !== 'skip').length;
					if (total > 0) scan.stage = `detecting ${f.size}/${total} file(s)…`;
					break;
				}
				case 'detect_complete':
					scan.stage = ev.total > 0 ? `verifying 0/${ev.total} finding(s)…` : 'verifying…';
					break;
				case 'verify_progress':
					if (ev.total > 0) scan.stage = `verifying ${ev.done}/${ev.total} finding(s)…`;
					break;
				case 'verify_complete': {
					const next = new Map(scan.verdictById);
					for (const v of ev.verified) next.set(v.finding.id, v.verdict);
					scan.verdictById = next;
					scan.stage = 'proposing patches…';
					break;
				}
				case 'patch_progress':
					if (ev.total > 0) scan.stage = `patching ${ev.done}/${ev.total} finding(s)…`;
					break;
				case 'patch_complete': {
					const next = new Map(scan.patchById);
					for (const p of ev.patches) next.set(p.finding_id, p);
					scan.patchById = next;
					scan.stage = 'done';
					break;
				}
				case 'usage_update':
					scan.usage = ev.usage;
					scan.rateLimitNotice = null;
					break;
				case 'durations_update':
					scan.durations = ev.durations;
					break;
				case 'rate_limited':
					scan.rateLimitNotice = {
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
		const history = scan.patchHistoryById.get(findingId);
		if (history && history.length > 0) return history;
		const current = scan.patchById.get(findingId);
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
		const next = new Map(scan.patchById);
		next.set(selectedFinding.id, v);
		scan.patchById = next;
	}

	async function regenerateAlternative() {
		if (!selectedFinding || !scan.root) return;
		const verified = verifiedFor(selectedFinding);
		if (!verified) return;
		triage.regenBusy = true;
		triage.regenError = null;
		try {
			const existing = patchHistoryFor(selectedFinding.id);
			const priors = existing.map((p) => p.proposal);
			const newPatch = await regeneratePatch(scan.root, verified, priors);
			const history = [...existing, newPatch];
			const histMap = new Map(scan.patchHistoryById);
			histMap.set(selectedFinding.id, history);
			scan.patchHistoryById = histMap;
			const pbi = new Map(scan.patchById);
			pbi.set(selectedFinding.id, newPatch);
			scan.patchById = pbi;
		} catch (e) {
			triage.regenError = e instanceof Error ? e.message : String(e);
		} finally {
			triage.regenBusy = false;
		}
	}

	function verifiedFor(f: Finding): VerifiedFinding | null {
		if (!scan.scanResult) return { finding: f, verdict: scan.verdictById.get(f.id) ?? null };
		const v = scan.scanResult.verified.find((x) => x.finding.id === f.id);
		return v ?? { finding: f, verdict: scan.verdictById.get(f.id) ?? null };
	}

	async function applySelectedPatch() {
		if (!selectedPatch || !selectedFinding) return;
		triage.applyBusy = true;
		triage.applyError = null;
		try {
			const result = await applyPatch(
				selectedFinding.id,
				scan.root,
				selectedPatch.proposal.file,
				selectedPatch.proposal.old_block,
				selectedPatch.proposal.new_block
			);
			if (result.located.kind === 'not_found' || result.bytes_written === 0) {
				triage.applyError = 'Patch could not be located in the file — nothing was written.';
				return;
			}
			const next = new Set(triage.appliedPatchIds);
			next.add(selectedFinding.id);
			triage.appliedPatchIds = next;
		} catch (e) {
			triage.applyError = e instanceof Error ? e.message : String(e);
		} finally {
			triage.applyBusy = false;
		}
	}

	// ---------- scan actions --------------------------------------------
	async function runScan() {
		if (!scan.root || scan.scanning) return;
		scan.scanning = true;
		scan.cancelling = false;
		scan.scanStartedAt = Date.now();
		scan.error = null;
		scan.stage = 'starting…';
		scan.resetResults();
		ui.resetSelection();
		triage.triageById = new Map();
		triage.appliedPatchIds = new Set();
		try {
			scan.scanResult = await runPipeline(scan.root, asScanConfig(settings.value));
			scan.resultRoot = scan.root;
			scan.stage = scan.scanResult.status === 'cancelled' ? 'cancelled' : 'done';
			await reloadTriageForCurrentRoot();
			await reloadAppliedForCurrentRoot();
		} catch (e) {
			scan.error = e instanceof Error ? e.message : String(e);
			scan.stage = 'error';
		} finally {
			scan.scanning = false;
			scan.cancelling = false;
			scan.scanStartedAt = null;
		}
	}

	async function exportTo(format: 'markdown' | 'sarif') {
		if (!scan.root) return;
		try {
			const content =
				format === 'markdown' ? await exportMarkdown(scan.root) : await exportSarif(scan.root);
			const { save } = await import('@tauri-apps/plugin-dialog');
			const ext = format === 'markdown' ? 'md' : 'sarif.json';
			const stem = scan.root.split(/[\\/]/).pop() || 'scan';
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
			scan.error = e instanceof Error ? e.message : String(e);
		}
	}

	async function requestCancel() {
		if (!scan.scanning || scan.cancelling) return;
		// Leave scan.stage alone — the progress bar should keep reflecting the
		// real pipeline state. The cancel-in-flight cue is the "Cancelling…"
		// label on the cancel button (WorkspaceTopBar reads scan.cancelling).
		scan.cancelling = true;
		try {
			await cancelScan();
		} catch (e) {
			console.error('cancel_scan failed', e);
		}
	}

	/** Wipe previous-scan state when the user opens a different project. */
	$effect(() => {
		if (scan.scanning) return;
		if (scan.resultRoot === null) return;
		if (scan.root === scan.resultRoot) return;
		scan.resetResults();
		ui.resetSelection();
		scan.stage = 'idle';
		scan.resultRoot = null;
	});

	function resetWorkspace() {
		scan.resetResults();
		ui.resetSelection();
		scan.stage = 'idle';
		scan.error = null;
		scan.resultRoot = null;
		triage.triageById = new Map();
		triage.dismissDraftFor = null;
		triage.dismissReason = '';
	}

	async function reloadTriageForCurrentRoot() {
		if (!scan.root) return;
		try {
			const rs = await getTriageForRoot(scan.root);
			const m = new Map<string, typeof rs[number]>();
			for (const r of rs) m.set(r.finding_id, r);
			triage.triageById = m;
		} catch (e) {
			console.error('getTriageForRoot failed', e);
		}
	}

	async function reloadAppliedForCurrentRoot() {
		if (!scan.root) return;
		try {
			const rs = await getAppliedForRoot(scan.root);
			const s = new Set<string>();
			for (const r of rs) s.add(r.finding_id);
			triage.appliedPatchIds = s;
		} catch (e) {
			console.error('getAppliedForRoot failed', e);
		}
	}

	function openProjectFresh(path: string) {
		resetWorkspace();
		scan.root = path;
		ui.view = 'workspace';
	}

	async function openProjectPast(group: ScanGroup) {
		resetWorkspace();
		scan.root = group.root;
		ui.view = 'workspace';
		try {
			const r = await loadScan(group.latest_scan_id);
			hydrateFromScanResult(r);
			await reloadTriageForCurrentRoot();
			await reloadAppliedForCurrentRoot();
		} catch (e) {
			scan.error = e instanceof Error ? e.message : String(e);
		}
	}

	function hydrateFromScanResult(r: ScanResult) {
		scan.walk = r.ingest;
		scan.triaged = r.triaged;
		const fbf = new Map<string, Finding[]>();
		for (const ff of r.findings_by_file) fbf.set(ff.rel_path, ff.findings);
		scan.findingsByFile = fbf;
		const errs = new Map<string, string>();
		for (const e of r.detect_errors ?? []) errs.set(e.rel_path, e.error);
		scan.detectErrors = errs;
		const vbi = new Map<string, typeof r.verified[number]['verdict']>();
		for (const v of r.verified) vbi.set(v.finding.id, v.verdict ?? null);
		scan.verdictById = vbi;
		const pbi = new Map<string, Patch>();
		for (const p of r.patches) pbi.set(p.finding_id, p);
		scan.patchById = pbi;
		scan.usage = r.usage;
		scan.durations = r.durations ?? EMPTY_STAGE_DURATIONS;
		scan.rateLimitNotice = null;
		scan.scanResult = r;
		scan.resultRoot = r.root;
		// A `running` row in the DB means the previous launch crashed or
		// was killed mid-scan. Surface it like a cancellation — the user
		// can re-scan; the partial findings are what we managed to persist
		// before the interruption.
		scan.stage = r.status === 'cancelled' || r.status === 'running' ? 'cancelled' : 'done';
	}

	function backToLauncher() {
		ui.view = 'launcher';
	}

	/** Re-run detect on every errored file sequentially. Sequential is the
	 *  conservative choice — a fan-out would risk hitting the per-minute
	 *  rate limit on the detect model since we're outside the orchestrator's
	 *  retry decorator here. */
	async function retryAllDetectErrors() {
		if (scan.retryingAll) return;
		scan.retryingAll = true;
		try {
			const rels = Array.from(scan.detectErrors.keys());
			for (const rel of rels) {
				// Re-check the live map — a successful retry of an earlier file
				// drops its entry, but if a new error came in we shouldn't skip
				// retried ones either. Iterate the snapshot.
				if (!scan.detectErrors.has(rel)) continue;
				await retryDetectForFile(rel);
			}
		} finally {
			scan.retryingAll = false;
		}
	}

	/** Re-run detect on a single previously-errored file. Uses the standalone
	 *  `scan_file` IPC, which runs detect-only with the default model — no
	 *  verify/patch. If the user wants verdicts on the retried file, they'll
	 *  need a full re-scan. */
	async function retryDetectForFile(rel: string) {
		if (!scan.root) return;
		if (scan.retryingFiles.has(rel)) return;
		const candidate = scan.walk?.candidates.find((c) => c.rel_path === rel);
		const absolutePath = candidate?.path ?? `${scan.root}/${rel}`;
		const next = new Set(scan.retryingFiles);
		next.add(rel);
		scan.retryingFiles = next;
		try {
			const findings = await scanFile(absolutePath, scan.root);
			const fbf = new Map(scan.findingsByFile);
			fbf.set(rel, findings);
			scan.findingsByFile = fbf;
			const errs = new Map(scan.detectErrors);
			errs.delete(rel);
			scan.detectErrors = errs;
		} catch (e) {
			const errs = new Map(scan.detectErrors);
			errs.set(rel, e instanceof Error ? e.message : String(e));
			scan.detectErrors = errs;
		} finally {
			const after = new Set(scan.retryingFiles);
			after.delete(rel);
			scan.retryingFiles = after;
		}
	}

	async function refreshKeyState() {
		scan.keyConfigured = await hasAnthropicKey();
	}

	// ---------- derived --------------------------------------------------
	let allFindings = $derived.by(() => {
		const out: { rel: string; f: Finding }[] = [];
		for (const [rel, fs] of scan.findingsByFile) {
			for (const f of fs) out.push({ rel, f });
		}
		out.sort((a, b) => severityRank(a.f.severity) - severityRank(b.f.severity));
		return out;
	});

	let visibleFindings = $derived.by(() => {
		let xs = allFindings;
		if (ui.selectedFile) xs = xs.filter((x) => x.rel === ui.selectedFile);
		if (ui.hideDismissed) {
			xs = xs.filter((x) => triage.triageById.get(x.f.id)?.status !== 'dismissed');
		}
		const q = ui.filter.trim().toLowerCase();
		if (q) {
			xs = xs.filter(
				(x) =>
					x.f.title.toLowerCase().includes(q) ||
					x.f.cwe.toLowerCase().includes(q) ||
					x.rel.toLowerCase().includes(q) ||
					x.f.description.toLowerCase().includes(q)
			);
		}
		return applyFindingsFilter(xs, ui.filterConfig, statusInputs);
	});

	let dismissedCount = $derived.by(() => {
		let n = 0;
		for (const t of triage.triageById.values()) if (t.status === 'dismissed') n++;
		return n;
	});

	let selectedFinding = $derived.by(() => {
		if (!ui.selectedFindingId) return null;
		return allFindings.find((x) => x.f.id === ui.selectedFindingId)?.f ?? null;
	});

	let selectedVerdict = $derived.by(() => {
		if (!selectedFinding) return null;
		return scan.verdictById.get(selectedFinding.id) ?? null;
	});

	let selectedPatch = $derived.by(() => {
		if (!selectedFinding) return null;
		return scan.patchById.get(selectedFinding.id) ?? null;
	});

	// ---------- file tree ------------------------------------------------
	function toggleFolder(p: string) {
		const next = new Set(ui.expandedFolders);
		if (next.has(p)) next.delete(p);
		else next.add(p);
		ui.expandedFolders = next;
	}

	let fileTree = $derived.by(() =>
		nestFiles(
			buildFileNodes({
				walk: scan.walk,
				triaged: scan.triaged,
				findingsByFile: scan.findingsByFile,
				detectErrors: scan.detectErrors
			})
		)
	);

	// Evict stale folder paths after a re-scan.
	$effect(() => {
		if (scan.scanning) return;
		const alive = collectFolderPaths(fileTree);
		let changed = false;
		const next = new Set<string>();
		for (const p of ui.expandedFolders) {
			if (alive.has(p)) next.add(p);
			else changed = true;
		}
		if (changed) ui.expandedFolders = next;
	});

	let visibleTree = $derived.by(() => flattenTree(fileTree, ui.expandedFolders));
	let totalFileNodes = $derived.by(() => countFileNodes(fileTree));
	let selectedFileNode = $derived.by(() =>
		ui.selectedFile ? findFileNode(fileTree, ui.selectedFile) : null
	);

	let currentStageIndex = $derived(stageIndex(scan.stage));

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
		scan.usage.total.input_tokens +
			scan.usage.total.output_tokens +
			scan.usage.total.cache_read_input_tokens
	);

	let usageRows = $derived.by(() => [
		{ name: 'triage', u: scan.usage.triage, ms: scan.durations.triage_ms },
		{ name: 'detect', u: scan.usage.detect, ms: scan.durations.detect_ms },
		{ name: 'verify', u: scan.usage.verify, ms: scan.durations.verify_ms },
		{ name: 'patch', u: scan.usage.patch, ms: scan.durations.patch_ms }
	]);

	// ---------- triage actions ------------------------------------------
	async function applyTriage(findingId: string, status: TriageStatus, reason?: string) {
		if (!scan.root) return;
		triage.triageBusy = true;
		try {
			const snoozeUntil =
				status === 'snoozed' ? Date.now() + SNOOZE_DAYS * 24 * 60 * 60 * 1000 : undefined;
			await setTriage(findingId, scan.root, status, reason, snoozeUntil);
			const m = new Map(triage.triageById);
			m.set(findingId, {
				finding_id: findingId,
				status,
				reason: reason ?? null,
				snooze_until: snoozeUntil ?? null,
				updated_at: Date.now()
			});
			triage.triageById = m;
		} catch (e) {
			scan.error = e instanceof Error ? e.message : String(e);
		} finally {
			triage.triageBusy = false;
		}
	}

	async function clearTriageFor(findingId: string) {
		if (!scan.root) return;
		triage.triageBusy = true;
		try {
			await clearTriage(findingId, scan.root);
			const m = new Map(triage.triageById);
			m.delete(findingId);
			triage.triageById = m;
		} catch (e) {
			scan.error = e instanceof Error ? e.message : String(e);
		} finally {
			triage.triageBusy = false;
		}
	}

	function startDismiss(findingId: string) {
		triage.dismissDraftFor = findingId;
		triage.dismissReason = triage.triageById.get(findingId)?.reason ?? '';
	}

	function cancelDismiss() {
		triage.dismissDraftFor = null;
		triage.dismissReason = '';
	}

	async function submitDismiss(findingId: string) {
		const r = triage.dismissReason.trim();
		if (!r) return;
		await applyTriage(findingId, 'dismissed', r);
		triage.dismissDraftFor = null;
		triage.dismissReason = '';
	}

	function selectFile(rel: string | null) {
		ui.selectedFile = rel;
		ui.selectedFindingId = null;
	}

	function selectFinding(id: string) {
		ui.selectedFindingId = id;
	}

	function handleKeydown(e: KeyboardEvent) {
		const target = e.target as HTMLElement | null;
		if (target && /^(input|textarea)$/i.test(target.tagName)) return;
		const xs = visibleFindings;
		if (xs.length === 0) return;
		if (e.key === 'ArrowDown' || e.key === 'j') {
			e.preventDefault();
			const idx = xs.findIndex((x) => x.f.id === ui.selectedFindingId);
			const next = idx < 0 ? 0 : Math.min(idx + 1, xs.length - 1);
			ui.selectedFindingId = xs[next].f.id;
			scrollFindingIntoView(ui.selectedFindingId);
		} else if (e.key === 'ArrowUp' || e.key === 'k') {
			e.preventDefault();
			const idx = xs.findIndex((x) => x.f.id === ui.selectedFindingId);
			const prev = idx < 0 ? xs.length - 1 : Math.max(idx - 1, 0);
			ui.selectedFindingId = xs[prev].f.id;
			scrollFindingIntoView(ui.selectedFindingId);
		} else if (e.key === 'Escape') {
			ui.selectedFindingId = null;
		}
	}

	function scrollFindingIntoView(id: string) {
		queueMicrotask(() => {
			const el = document.querySelector<HTMLElement>(`[data-finding-id="${CSS.escape(id)}"]`);
			el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
		});
	}

	// Reset per-finding action errors when the user moves to a different finding.
	$effect(() => {
		void ui.selectedFindingId;
		triage.applyError = null;
		triage.regenError = null;
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
				highlightCode(ex.text, ex.language)
					.then((html) => {
						if (!cancelled) excerptHtml = html;
					})
					.catch((e) => {
						// Highlighting failed but the plain-text excerpt still renders.
						console.warn('shiki: highlight failed, falling back to plain text', e);
						if (!cancelled) excerptHtml = null;
					});
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

	let humanizedError = $derived(scan.error ? humanizeError(scan.error) : null);
	let showProgress = $derived(
		scan.scanning || !!scan.scanResult || scan.stage === 'done' || scan.stage === 'cancelled'
	);
	let showOnboarding = $derived(!scan.scanResult && !scan.scanning && scan.stage === 'idle');
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

{#if ui.view === 'launcher'}
	<Launcher onOpenFresh={openProjectFresh} onOpenPast={openProjectPast} />
{:else}
	<WorkspaceView
		{statusInputs}
		{humanizedError}
		{showProgress}
		{currentStageIndex}
		{showOnboarding}
		{visibleTree}
		{totalFileNodes}
		{totals}
		{visibleFindings}
		{allFindings}
		{dismissedCount}
		{selectedFileNode}
		{selectedFinding}
		{selectedFileNodeIsStatus}
		{selectedVerdict}
		{selectedPatch}
		{selectedPatchVariants}
		{selectedPatchVariantIdx}
		{excerpt}
		{excerptHtml}
		{excerptError}
		{diffHtml}
		{severityCounts}
		{usageRows}
		{totalTokens}
		snoozeDays={SNOOZE_DAYS}
		onBack={backToLauncher}
		onScan={runScan}
		onCancel={requestCancel}
		onOpenSettings={() => (ui.settingsOpen = true)}
		onExportMarkdown={() => exportTo('markdown')}
		onExportSarif={() => exportTo('sarif')}
		onRefreshKeyState={refreshKeyState}
		onSelectFile={selectFile}
		onSelectFinding={selectFinding}
		onToggleFolder={toggleFolder}
		onApplyTriage={applyTriage}
		onClearTriage={clearTriageFor}
		onStartDismiss={startDismiss}
		onCancelDismiss={cancelDismiss}
		onSubmitDismiss={submitDismiss}
		onApplyPatch={applySelectedPatch}
		onRegenerate={regenerateAlternative}
		onSelectVariant={selectPatchVariant}
		onRetryDetect={retryDetectForFile}
		onRetryAll={retryAllDetectErrors}
		onClearSelection={() => {
			ui.selectedFindingId = null;
			if (selectedFileNodeIsStatus) selectFile(null);
		}}
	/>
{/if}

{#if ui.settingsOpen}
	<Settings onClose={() => (ui.settingsOpen = false)} />
{/if}
