<script lang="ts">
	import type { Priority } from '$lib/ipc';
	import type { VisibleRow } from '$lib/tree';
	import {
		priorityChipClass,
		priorityChipLabel,
		severityDot,
		skipReasonLabel
	} from '$lib/scan-display';

	interface Props {
		visibleTree: VisibleRow[];
		totalFileNodes: number;
		totalFindings: number;
		selectedFile: string | null;
		scanning: boolean;
		stage: string;
		hasWalk: boolean;
		walkCandidateCount: number;
		hasTriaged: boolean;
		expandedFolders: Set<string>;
		onSelectFile: (rel: string | null) => void;
		onToggleFolder: (path: string) => void;
	}
	let {
		visibleTree,
		totalFileNodes,
		totalFindings,
		selectedFile,
		scanning,
		stage,
		hasWalk,
		walkCandidateCount,
		hasTriaged,
		expandedFolders,
		onSelectFile,
		onToggleFolder
	}: Props = $props();

	function priorityTitle(p: Priority | null): string {
		return `triage priority: ${p ?? 'unknown'}`;
	}
</script>

<aside class="flex flex-col overflow-hidden border-r border-border">
	<div class="flex h-10 items-center justify-between border-b border-border px-3">
		<span class="text-xs font-medium tracking-wide text-muted-foreground uppercase">Files</span>
		<span class="text-xs text-muted-foreground">{totalFileNodes}</span>
	</div>
	<div class="flex-1 overflow-y-auto">
		{#if totalFileNodes === 0 && !scanning}
			{#if stage === 'done' && hasWalk && walkCandidateCount === 0}
				<p class="px-3 py-3 text-xs text-muted-foreground">No scannable files in this folder.</p>
			{:else}
				<p class="px-3 py-3 text-xs text-muted-foreground">Hit Scan to see the file tree.</p>
			{/if}
		{:else if totalFileNodes === 0 && scanning}
			<p class="animate-pulse px-3 py-3 text-xs text-muted-foreground">{stage}</p>
		{:else}
			<button
				type="button"
				class="flex w-full items-center justify-between px-3 py-1.5 text-left text-xs hover:bg-muted/50 {selectedFile ===
				null
					? 'bg-muted'
					: ''}"
				onclick={() => onSelectFile(null)}
			>
				<span class="font-medium">All files</span>
				<span class="text-muted-foreground">{totalFindings}</span>
			</button>
			{#each visibleTree as row (row.node.path)}
				{#if row.node.type === 'folder'}
					{@const f = row.node}
					{@const expanded = expandedFolders.has(f.path)}
					<button
						type="button"
						class="flex w-full items-center gap-1 py-1 pr-3 text-left hover:bg-muted/50 {f.allSkipped
							? 'opacity-50'
							: ''}"
						style="padding-left: {0.5 + row.depth * 0.75}rem"
						onclick={() => onToggleFolder(f.path)}
						title={f.allSkipped ? `All ${f.skippedCount} file(s) skipped` : ''}
					>
						<span
							class="inline-flex h-3 w-3 shrink-0 items-center justify-center text-muted-foreground transition-transform"
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
							class="shrink-0 text-muted-foreground/80"
						>
							<path
								d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
							/>
						</svg>
						<span class="flex-1 truncate font-mono text-xs {f.allSkipped ? 'italic' : ''}">
							{f.name}
						</span>
						{#if f.topSeverity}
							<span class="h-2 w-2 shrink-0 rounded-full {severityDot(f.topSeverity)}"></span>
						{/if}
						{#if f.count > 0}
							<span class="text-xs text-muted-foreground tabular-nums">{f.count}</span>
						{:else if f.allSkipped}
							<span class="text-[0.625rem] text-muted-foreground/70 italic">skip</span>
						{:else if f.skippedCount > 0}
							<span
								class="text-[0.625rem] text-muted-foreground/60 italic tabular-nums"
								title="{f.skippedCount} skipped file(s) inside"
							>
								{f.skippedCount}
							</span>
						{/if}
					</button>
				{:else}
					{@const f = row.node}
					{@const isSkipped = f.status === 'pre_triage_skipped' || f.status === 'triage_skipped'}
					<button
						type="button"
						class="flex w-full items-center gap-1.5 py-1 pr-3 text-left hover:bg-muted/50 {selectedFile ===
						f.path
							? 'bg-muted'
							: ''} {isSkipped ? 'opacity-60' : ''}"
						style="padding-left: {0.5 + row.depth * 0.75}rem"
						onclick={() => onSelectFile(f.path)}
						title={f.detectError
							? f.detectError
							: f.skipReason
								? `skipped: ${skipReasonLabel(f.skipReason)}`
								: f.triageReason
									? `triage skip: ${f.triageReason}`
									: ''}
					>
						<!-- Indent slot to align files with folder rows (where chevron sits) -->
						<span class="inline-block w-3 shrink-0"></span>
						{#if isSkipped}
							<span
								class="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded bg-zinc-300/30 font-mono text-[0.625rem] text-muted-foreground/70 italic dark:bg-zinc-700/40"
							>
								S
							</span>
						{:else}
							<span
								class="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded font-mono text-[0.625rem] font-semibold {priorityChipClass(
									f.priority
								)}"
								title={priorityTitle(f.priority)}
							>
								{priorityChipLabel(f.priority)}
							</span>
						{/if}
						{#if f.status === 'errored'}
							<span class="shrink-0 text-destructive">
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
							<span class="h-2 w-2 shrink-0 rounded-full {severityDot(f.topSeverity)}"></span>
						{:else}
							<span class="h-2 w-2 shrink-0 rounded-full bg-zinc-200 dark:bg-zinc-700"></span>
						{/if}
						<span class="flex-1 truncate font-mono text-xs">{f.name}</span>
						{#if f.count > 0}
							<span class="text-xs text-muted-foreground tabular-nums">{f.count}</span>
						{:else if f.skipReason}
							<span class="text-[0.625rem] text-muted-foreground/70 italic">
								{skipReasonLabel(f.skipReason)}
							</span>
						{:else if f.status === 'triage_skipped'}
							<span class="text-[0.625rem] text-muted-foreground/70 italic">skip</span>
						{/if}
					</button>
				{/if}
			{/each}
		{/if}
	</div>

	{#if hasTriaged}
		<div
			class="border-t border-border px-3 py-2 text-[0.6875rem] leading-relaxed text-muted-foreground"
		>
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
