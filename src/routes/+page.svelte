<script lang="ts">
	import { onDestroy, onMount } from 'svelte';

	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import Launcher from '$lib/components/Launcher.svelte';
	import { renderMd, renderInlineMd } from '$lib/markdown';
	import { theme } from '$lib/theme.svelte';
	import {
		EMPTY_STAGE_USAGE,
		hasAnthropicKey,
		listenScanEvents,
		listScanGroups,
		loadScan,
		runPipeline,
		setAnthropicKey,
		type Finding,
		type FileFindings,
		type Patch,
		type Priority,
		type ScanGroup,
		type ScanResult,
		type Severity,
		type SkipReason,
		type StageUsage,
		type TriagedFile,
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
	let scanResult = $state<ScanResult | null>(null);

	let selectedFile = $state<string | null>(null);
	let selectedFindingId = $state<string | null>(null);
	let filter = $state('');

	/** Root the *current* result set was produced for. Used to auto-clear
	 *  stale panes when the user picks/types a different folder. */
	let resultRoot = $state<string | null>(null);

	let unlisten: UnlistenFn | null = null;

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
		error = null;
		stage = 'starting…';
		walk = null;
		triaged = [];
		findingsByFile = new Map();
		detectErrors = new Map();
		verdictById = new Map();
		patchById = new Map();
		usage = EMPTY_STAGE_USAGE;
		scanResult = null;
		selectedFile = null;
		selectedFindingId = null;
		try {
			scanResult = await runPipeline(root);
			resultRoot = root;
			stage = 'done';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			stage = 'error';
		} finally {
			scanning = false;
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
		scanResult = null;
		selectedFile = null;
		selectedFindingId = null;
		filter = '';
		stage = 'idle';
		error = null;
		resultRoot = null;
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
		const vbi = new Map<string, Verdict | null>();
		for (const v of r.verified) vbi.set(v.finding.id, v.verdict ?? null);
		verdictById = vbi;
		const pbi = new Map<string, Patch>();
		for (const p of r.patches) pbi.set(p.finding_id, p);
		patchById = pbi;
		usage = r.usage;
		scanResult = r;
		resultRoot = r.root;
		stage = 'done';
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

	function verdictStatus(f: Finding): 'pending' | 'verifying' | 'kept' | 'dropped' | 'hardening' {
		if (f.kind === 'hardening') return 'hardening';
		if (!verdictById.has(f.id)) return scanning ? 'verifying' : 'pending';
		const v = verdictById.get(f.id);
		if (v === null || v === undefined) return 'pending';
		return v.is_reachable && v.concrete_exploit ? 'kept' : 'dropped';
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

	let totalTokensCompact = $derived.by(() => {
		const t = usage.total;
		const sum = t.input_tokens + t.output_tokens + t.cache_read_input_tokens;
		return compactTokens(sum);
	});

	let usageRows = $derived.by(() => [
		{ name: 'triage', u: usage.triage },
		{ name: 'detect', u: usage.detect },
		{ name: 'verify', u: usage.verify },
		{ name: 'patch', u: usage.patch }
	]);

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
		let kept = 0;
		let dropped = 0;
		let hardening = 0;
		let pending = 0;
		for (const { f } of allFindings) {
			const s = verdictStatus(f);
			if (s === 'kept') kept++;
			else if (s === 'dropped') dropped++;
			else if (s === 'hardening') hardening++;
			else pending++;
		}
		return { kept, dropped, hardening, pending, total: allFindings.length };
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
			const el = document.querySelector<HTMLElement>(`[data-finding-id="${id}"]`);
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
		<h1 class="text-base font-semibold tracking-tight">open-sec</h1>
		<div class="bg-border h-5 w-px"></div>
		<div class="text-foreground/80 flex flex-1 items-center gap-2 truncate font-mono text-xs">
			<span class="truncate" title={root}>{root}</span>
		</div>
		<Button size="sm" onclick={runScan} disabled={scanning || !root || !keyConfigured}>
			{scanning ? 'Scanning…' : scanResult || stage === 'done' ? 'Re-scan' : 'Scan'}
		</Button>
		<div class="text-muted-foreground flex items-center gap-3 text-xs">
			{#if usage.total.input_tokens + usage.total.output_tokens > 0}
				<span
					class="font-mono"
					title="input: {usage.total.input_tokens.toLocaleString()} · output: {usage.total.output_tokens.toLocaleString()} · cache read: {usage.total.cache_read_input_tokens.toLocaleString()}"
				>
					{totalTokensCompact} tok
				</span>
			{/if}
			<span class="font-mono">{stage}</span>
			<button
				type="button"
				onclick={() => theme.cycle()}
				class="hover:bg-muted text-muted-foreground hover:text-foreground inline-flex h-7 w-7 items-center justify-center rounded transition-colors"
				title={theme.value === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
				aria-label="Toggle theme"
			>
				{#if theme.value === 'dark'}
					<!-- Sun icon -->
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
						<circle cx="12" cy="12" r="4" />
						<path d="M12 2v2" />
						<path d="M12 20v2" />
						<path d="m4.93 4.93 1.41 1.41" />
						<path d="m17.66 17.66 1.41 1.41" />
						<path d="M2 12h2" />
						<path d="M20 12h2" />
						<path d="m6.34 17.66-1.41 1.41" />
						<path d="m19.07 4.93-1.41 1.41" />
					</svg>
				{:else}
					<!-- Moon icon -->
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
						<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
					</svg>
				{/if}
			</button>
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
		<div class="border-destructive/40 bg-destructive/5 border-b px-4 py-2 text-xs">
			<span class="text-destructive font-medium">Error:</span>
			<span class="text-destructive ml-2 font-mono">{error}</span>
		</div>
	{/if}

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
					{#each visibleTree as row, idx (row.node.path + ':' + idx)}
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
				<div class="border-border space-y-1 border-t px-3 py-2 text-xs">
					<div class="text-muted-foreground text-[0.625rem] uppercase tracking-wide">
						Triage
					</div>
					<div class="flex gap-2">
						<Badge class={priorityClass('high')}>{triageFunnel.high} high</Badge>
						<Badge class={priorityClass('normal')}>{triageFunnel.normal}</Badge>
						<Badge class={priorityClass('low')}>{triageFunnel.low}</Badge>
						<Badge class={priorityClass('skip')}>{triageFunnel.skip} skip</Badge>
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
				<div class="border-border border-b px-3 py-2">
					<Input
						bind:value={filter}
						placeholder="Filter by title, CWE, file…"
						spellcheck={false}
						class="h-7 text-xs"
					/>
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
						<div class="space-y-2 px-4 py-8 text-center text-sm">
							<div class="text-2xl">✓</div>
							<p class="font-medium">Clean scan</p>
							<p class="text-muted-foreground text-xs leading-relaxed">
								No vulnerabilities found in
								{walk?.candidates.length ?? 0} file(s).
							</p>
						</div>
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
						{@const status = verdictStatus(f)}
						<button
							type="button"
							data-finding-id={f.id}
							class="hover:bg-muted/40 border-border block w-full border-b px-4 py-3 text-left {selectedFindingId === f.id ? 'bg-muted/60' : ''}"
							onclick={() => selectFinding(f.id)}
						>
							<div class="flex items-start justify-between gap-2">
								<div class="flex-1 space-y-1">
									<div class="flex items-center gap-1.5">
										<Badge class={severityClass(f.severity)}>{f.severity}</Badge>
										<span class="text-muted-foreground font-mono text-xs">{f.cwe}</span>
										{#if f.owasp}
											<span class="text-muted-foreground font-mono text-xs">
												· {f.owasp}
											</span>
										{/if}
										{#if status === 'verifying'}
											<span class="text-muted-foreground animate-pulse text-xs">
												verifying…
											</span>
										{:else if status === 'kept'}
											<Badge class="bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">
												kept
											</Badge>
										{:else if status === 'dropped'}
											<Badge class="bg-zinc-400/15 text-zinc-500 line-through">dropped</Badge>
										{:else if status === 'hardening'}
											<Badge class="bg-sky-500/15 text-sky-700 dark:text-sky-300">
												hardening
											</Badge>
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
							<section class="space-y-3 px-5 py-4">
								<div class="flex items-center justify-between gap-2">
									<h3
										class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
									>
										Patch
									</h3>
									<Badge variant="outline" class="font-mono text-[0.625rem]">
										{selectedPatch.located.kind === 'not_found'
											? 'not located'
											: selectedPatch.located.kind}
									</Badge>
								</div>
								<div class="md text-sm leading-relaxed">
									{@html renderMd(selectedPatch.proposal.explanation)}
								</div>
								{#if selectedPatch.diff}
									<pre
										class="border-border bg-muted/40 overflow-auto rounded-md border font-mono text-xs leading-relaxed">{#each selectedPatch.diff.split('\n') as line, i (i)}<div
												class="px-3 {diffLineClass(line)}"
											>{line || ' '}</div>{/each}</pre>
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
							<section class="space-y-2">
								<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
									Error
								</h3>
								<pre class="bg-muted/40 border-border overflow-auto whitespace-pre-wrap rounded-md border p-3 font-mono text-xs">{selectedFileNode.detectError ?? '(no error message)'}</pre>
								<p class="text-muted-foreground text-xs leading-relaxed">
									The detect agent could not produce a JSON result for this file
									(model error, parse failure, or 25-iteration tool-use cap).
								</p>
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
						<div>
							<h2 class="mb-2 text-base font-medium">Scan summary</h2>
							<dl class="text-sm">
								<div class="grid grid-cols-[140px_1fr] py-0.5">
									<dt class="text-muted-foreground">Root</dt>
									<dd class="truncate font-mono text-xs">{root || '—'}</dd>
								</div>
								<div class="grid grid-cols-[140px_1fr] py-0.5">
									<dt class="text-muted-foreground">Status</dt>
									<dd>{stage}</dd>
								</div>
								{#if walk}
									<div class="grid grid-cols-[140px_1fr] py-0.5">
										<dt class="text-muted-foreground">Ingest</dt>
										<dd>
											{walk.candidates.length} candidate(s), {walk.skipped.length} skipped
										</dd>
									</div>
								{/if}
								{#if triaged.length > 0}
									<div class="grid grid-cols-[140px_1fr] py-0.5">
										<dt class="text-muted-foreground">Triage</dt>
										<dd>
											{triageFunnel.high} high / {triageFunnel.normal} normal / {triageFunnel.low}
											low / {triageFunnel.skip} skip
										</dd>
									</div>
								{/if}
								<div class="grid grid-cols-[140px_1fr] py-0.5">
									<dt class="text-muted-foreground">Findings</dt>
									<dd>
										{totals.kept} kept / {totals.dropped} dropped / {totals.hardening} hardening
										{#if totals.pending > 0}
											· {totals.pending} pending
										{/if}
									</dd>
								</div>
								<div class="grid grid-cols-[140px_1fr] py-0.5">
									<dt class="text-muted-foreground">Patches</dt>
									<dd>{patchById.size}</dd>
								</div>
							</dl>
						</div>

						{#if usage.total.input_tokens + usage.total.output_tokens > 0}
							<div>
								<h3
									class="text-muted-foreground mb-2 text-[0.625rem] font-medium uppercase tracking-wider"
								>
									Token usage
								</h3>
								<table class="w-full text-xs">
									<thead>
										<tr class="text-muted-foreground border-border border-b">
											<th class="text-left font-normal">stage</th>
											<th class="text-right font-normal">in</th>
											<th class="text-right font-normal">out</th>
											<th class="text-right font-normal">cache rd</th>
										</tr>
									</thead>
									<tbody class="font-mono">
										{#each usageRows as row (row.name)}
											<tr class="border-border/40 border-b">
												<td class="py-0.5">{row.name}</td>
												<td class="text-right">{row.u.input_tokens.toLocaleString()}</td>
												<td class="text-right">{row.u.output_tokens.toLocaleString()}</td>
												<td class="text-muted-foreground text-right">
													{row.u.cache_read_input_tokens.toLocaleString()}
												</td>
											</tr>
										{/each}
										<tr class="font-semibold">
											<td class="py-1">total</td>
											<td class="text-right">{usage.total.input_tokens.toLocaleString()}</td>
											<td class="text-right">{usage.total.output_tokens.toLocaleString()}</td>
											<td class="text-muted-foreground text-right">
												{usage.total.cache_read_input_tokens.toLocaleString()}
											</td>
										</tr>
									</tbody>
								</table>
							</div>
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
</div>
{/if}
