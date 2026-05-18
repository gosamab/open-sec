/**
 * File-tree types and pure builders. The route owns the source data
 * (walk / triaged / findingsByFile / detectErrors) and the user state
 * (expandedFolders / selectedFile); FileTree.svelte just renders the
 * `VisibleRow[]` we compute here.
 */

import type {
	Finding,
	Priority,
	Severity,
	SkipReason,
	TriagedFile,
	WalkResult
} from './ipc';
import { SEVERITY_ORDER, basename, severityRank } from './scan-display';

export type FileStatus =
	| 'pending'
	| 'scanned'
	| 'errored'
	| 'triage_skipped'
	| 'pre_triage_skipped';

export type FileNode = {
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

export type FolderNode = {
	type: 'folder';
	path: string;
	name: string;
	children: TreeNode[];
	count: number;
	topSeverity: Severity | null;
	skippedCount: number;
	/** True iff every file under this folder is skipped (pre-triage or triage). */
	allSkipped: boolean;
};

export type TreeNode = FileNode | FolderNode;

export type VisibleRow = { node: TreeNode; depth: number };

export interface BuildInputs {
	walk: WalkResult | null;
	triaged: TriagedFile[];
	findingsByFile: Map<string, Finding[]>;
	detectErrors: Map<string, string>;
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

export function buildFileNodes(s: BuildInputs): FileNode[] {
	const map = new Map<string, FileNode>();

	if (s.walk) {
		// Seed candidates (every file that survived pre-triage).
		for (const c of s.walk.candidates) {
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
		for (const sk of s.walk.skipped) {
			map.set(sk.rel_path, {
				type: 'file',
				path: sk.rel_path,
				name: basename(sk.rel_path),
				status: 'pre_triage_skipped',
				priority: null,
				count: 0,
				topSeverity: null,
				skipReason: sk.reason
			});
		}
	}

	// Layer in triage decisions.
	for (const t of s.triaged) {
		const n = map.get(t.candidate.rel_path);
		if (!n) continue;
		n.priority = t.result.priority;
		if (t.result.priority === 'skip') {
			n.status = 'triage_skipped';
			n.triageReason = t.result.reason;
		}
	}

	// Layer in detect results.
	for (const [rel, fs] of s.findingsByFile) {
		const n = map.get(rel);
		if (!n) continue;
		n.count = fs.length;
		n.topSeverity = topSeverityOf(fs);
		if (n.status === 'pending') n.status = 'scanned';
	}

	// Layer in detect errors (overrides 'scanned' if applicable).
	for (const [rel, err] of s.detectErrors) {
		const n = map.get(rel);
		if (!n) continue;
		n.status = 'errored';
		n.detectError = err;
	}

	return [...map.values()];
}

export function nestFiles(files: FileNode[]): TreeNode[] {
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

	const annotate = (
		node: TreeNode
	): { count: number; topSev: Severity | null; skipped: number; allSkipped: boolean } => {
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

/** Flatten the tree into a depth-tagged list, respecting `expanded`. */
export function flattenTree(tree: TreeNode[], expanded: Set<string>): VisibleRow[] {
	const out: VisibleRow[] = [];
	const walk = (children: TreeNode[], depth: number) => {
		for (const c of children) {
			out.push({ node: c, depth });
			if (c.type === 'folder' && expanded.has(c.path)) {
				walk(c.children, depth + 1);
			}
		}
	};
	walk(tree, 0);
	return out;
}

/** Find a single FileNode by path in the (already-built) tree. */
export function findFileNode(tree: TreeNode[], path: string): FileNode | null {
	let found: FileNode | null = null;
	const walk = (children: TreeNode[]) => {
		for (const c of children) {
			if (found) return;
			if (c.type === 'file') {
				if (c.path === path) found = c;
			} else {
				walk(c.children);
			}
		}
	};
	walk(tree);
	return found;
}

/** Count file nodes (leaves) anywhere in the tree. */
export function countFileNodes(tree: TreeNode[]): number {
	let n = 0;
	const walk = (children: TreeNode[]) => {
		for (const c of children) {
			if (c.type === 'file') n++;
			else walk(c.children);
		}
	};
	walk(tree);
	return n;
}

/** Every folder path currently in the tree — for evicting stale expansions. */
export function collectFolderPaths(tree: TreeNode[]): Set<string> {
	const out = new Set<string>();
	const walk = (children: TreeNode[]) => {
		for (const c of children) {
			if (c.type === 'folder') {
				out.add(c.path);
				walk(c.children);
			}
		}
	};
	walk(tree);
	return out;
}
