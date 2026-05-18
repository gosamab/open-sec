<script lang="ts">
	import { Input } from '$lib/components/ui/input';
	import FindingBadges from '$lib/components/FindingBadges.svelte';
	import type { Finding, Severity } from '$lib/ipc';
	import type { FileNode } from '$lib/tree';
	import {
		DEFAULT_FINDINGS_FILTER,
		SEVERITY_ORDER,
		activeFilterCount,
		findingStatus,
		humanizeError,
		severityClass,
		skipReasonLabel,
		statusClass,
		STATUS_OPTIONS,
		type FindingStatus,
		type FindingStatusInputs,
		type FindingsFilter,
		type KindFilter,
		type SortDir,
		type SortKey
	} from '$lib/scan-display';

	export type VisibleFinding = { rel: string; f: Finding };

	interface Props {
		visibleFindings: VisibleFinding[];
		allFindingsCount: number;
		filter: string;
		hideDismissed: boolean;
		dismissedCount: number;
		filterConfig: FindingsFilter;
		selectedFindingId: string | null;
		selectedFile: string | null;
		selectedFileNode: FileNode | null;
		scanning: boolean;
		stage: string;
		hasWalk: boolean;
		walkCandidateCount: number;
		detectErrors: Map<string, string>;
		statusInputs: FindingStatusInputs;
		onSelectFinding: (id: string) => void;
		onSelectFile: (rel: string) => void;
	}
	let {
		visibleFindings,
		allFindingsCount,
		filter = $bindable(''),
		hideDismissed = $bindable(true),
		dismissedCount,
		filterConfig = $bindable(DEFAULT_FINDINGS_FILTER),
		selectedFindingId,
		selectedFile,
		selectedFileNode,
		scanning,
		stage,
		hasWalk,
		walkCandidateCount,
		detectErrors,
		statusInputs,
		onSelectFinding,
		onSelectFile
	}: Props = $props();

	let activeCount = $derived(activeFilterCount(filterConfig));

	// Popover state
	let panelOpen = $state(false);
	let panelRef = $state<HTMLDivElement | null>(null);

	$effect(() => {
		if (!panelOpen) return;
		const onDoc = (e: PointerEvent) => {
			if (panelRef && !panelRef.contains(e.target as Node)) panelOpen = false;
		};
		const onEsc = (e: KeyboardEvent) => {
			if (e.key === 'Escape') panelOpen = false;
		};
		document.addEventListener('pointerdown', onDoc);
		document.addEventListener('keydown', onEsc);
		return () => {
			document.removeEventListener('pointerdown', onDoc);
			document.removeEventListener('keydown', onEsc);
		};
	});

	function toggleSeverity(s: Severity) {
		const next = new Set(filterConfig.severities);
		if (next.has(s)) next.delete(s);
		else next.add(s);
		filterConfig = { ...filterConfig, severities: next };
	}

	function toggleStatus(s: FindingStatus) {
		const next = new Set(filterConfig.statuses);
		if (next.has(s)) next.delete(s);
		else next.add(s);
		filterConfig = { ...filterConfig, statuses: next };
	}

	function setSort(key: SortKey) {
		filterConfig = { ...filterConfig, sortKey: key };
	}

	function toggleSortDir() {
		const dir: SortDir = filterConfig.sortDir === 'asc' ? 'desc' : 'asc';
		filterConfig = { ...filterConfig, sortDir: dir };
	}

	function setKind(k: KindFilter) {
		filterConfig = { ...filterConfig, kind: k };
	}

	function resetFilter() {
		filterConfig = { ...DEFAULT_FINDINGS_FILTER };
	}

	const SORT_OPTIONS: { key: SortKey; label: string }[] = [
		{ key: 'severity', label: 'Severity' },
		{ key: 'status', label: 'Status' },
		{ key: 'file', label: 'File' },
		{ key: 'line', label: 'Line' }
	];

	const KIND_OPTIONS: { key: KindFilter; label: string }[] = [
		{ key: 'all', label: 'All' },
		{ key: 'vuln', label: 'Vuln' },
		{ key: 'hardening', label: 'Hardening' }
	];
</script>

<section class="flex flex-col overflow-hidden border-r border-border">
	<div class="flex h-10 items-center justify-between gap-2 border-b border-border px-3">
		<span class="truncate text-xs font-medium tracking-wide text-muted-foreground uppercase">
			Findings{selectedFile ? ` · ${selectedFile}` : ''}
		</span>
		<div class="flex shrink-0 items-center gap-2">
			<div class="relative" bind:this={panelRef}>
				<button
					type="button"
					class="inline-flex h-6 items-center gap-1 rounded px-1.5 text-[0.6875rem] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground {activeCount >
					0
						? 'bg-muted/60 text-foreground'
						: ''}"
					title="Filter & sort"
					aria-label="Filter and sort findings"
					aria-haspopup="menu"
					aria-expanded={panelOpen}
					onclick={() => (panelOpen = !panelOpen)}
				>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						width="11"
						height="11"
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<line x1="4" y1="6" x2="20" y2="6" />
						<line x1="7" y1="12" x2="17" y2="12" />
						<line x1="10" y1="18" x2="14" y2="18" />
					</svg>
					<span>Filter</span>
					{#if activeCount > 0}
						<span
							class="inline-flex h-3.5 min-w-3.5 items-center justify-center rounded-full bg-foreground px-1 font-mono text-[0.5625rem] leading-none font-semibold text-background"
						>
							{activeCount}
						</span>
					{/if}
				</button>
				{#if panelOpen}
					<div
						class="absolute top-full right-0 z-20 mt-1 w-72 space-y-3 rounded-md border border-border bg-popover p-3 text-popover-foreground shadow-md"
						role="menu"
					>
						<!-- Sort -->
						<div class="space-y-1.5">
							<div
								class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase"
							>
								Sort
							</div>
							<div class="flex flex-wrap gap-1">
								{#each SORT_OPTIONS as opt (opt.key)}
									<button
										type="button"
										onclick={() => setSort(opt.key)}
										class="rounded border px-1.5 py-0.5 text-[0.6875rem] {filterConfig.sortKey ===
										opt.key
											? 'border-foreground bg-foreground text-background'
											: 'border-border text-muted-foreground hover:text-foreground'}"
									>
										{opt.label}
									</button>
								{/each}
								<button
									type="button"
									onclick={toggleSortDir}
									class="ml-auto inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-[0.6875rem] text-muted-foreground hover:text-foreground"
									title="Toggle sort direction"
								>
									{#if filterConfig.sortDir === 'asc'}
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
											<path d="m6 15 6-6 6 6" />
										</svg>
										<span>Asc</span>
									{:else}
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
											<path d="m6 9 6 6 6-6" />
										</svg>
										<span>Desc</span>
									{/if}
								</button>
							</div>
						</div>

						<!-- Severity -->
						<div class="space-y-1.5">
							<div
								class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase"
							>
								Severity
							</div>
							<div class="flex flex-wrap gap-1">
								{#each SEVERITY_ORDER as s (s)}
									{@const active = filterConfig.severities.has(s)}
									<button
										type="button"
										onclick={() => toggleSeverity(s)}
										class="rounded border px-1.5 py-0.5 text-[0.6875rem] capitalize {active
											? severityClass(s) + ' border-transparent'
											: 'border-border text-muted-foreground hover:text-foreground'}"
									>
										{s}
									</button>
								{/each}
							</div>
						</div>

						<!-- Status -->
						<div class="space-y-1.5">
							<div
								class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase"
							>
								Status
							</div>
							<div class="flex flex-wrap gap-1">
								{#each STATUS_OPTIONS as opt (opt.key)}
									{@const active = filterConfig.statuses.has(opt.key)}
									<button
										type="button"
										onclick={() => toggleStatus(opt.key)}
										class="rounded border px-1.5 py-0.5 text-[0.6875rem] {active
											? statusClass(opt.key) + ' border-transparent'
											: 'border-border text-muted-foreground hover:text-foreground'}"
									>
										{opt.label}
									</button>
								{/each}
							</div>
						</div>

						<!-- Kind -->
						<div class="space-y-1.5">
							<div
								class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase"
							>
								Kind
							</div>
							<div class="flex flex-wrap gap-1">
								{#each KIND_OPTIONS as opt (opt.key)}
									<button
										type="button"
										onclick={() => setKind(opt.key)}
										class="rounded border px-1.5 py-0.5 text-[0.6875rem] {filterConfig.kind ===
										opt.key
											? 'border-foreground bg-foreground text-background'
											: 'border-border text-muted-foreground hover:text-foreground'}"
									>
										{opt.label}
									</button>
								{/each}
							</div>
						</div>

						<div class="flex items-center justify-end gap-2 border-t border-border pt-2">
							<button
								type="button"
								class="text-[0.6875rem] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
								onclick={resetFilter}
							>
								Reset
							</button>
							<button
								type="button"
								class="text-[0.6875rem] text-muted-foreground hover:text-foreground"
								onclick={() => (panelOpen = false)}
							>
								Done
							</button>
						</div>
					</div>
				{/if}
			</div>
			<span class="text-xs text-muted-foreground">
				{visibleFindings.length}{(filter || activeCount > 0) &&
				allFindingsCount !== visibleFindings.length
					? ` / ${allFindingsCount}`
					: ''}
			</span>
		</div>
	</div>
	{#if allFindingsCount > 0 || filter}
		<div class="space-y-1.5 border-b border-border px-3 py-2">
			<Input
				bind:value={filter}
				placeholder="Filter by title, CWE, file…"
				spellcheck={false}
				class="h-7 text-xs"
			/>
			{#if dismissedCount > 0}
				<label class="flex items-center gap-1.5 text-xs text-muted-foreground">
					<input type="checkbox" bind:checked={hideDismissed} class="h-3 w-3 accent-current" />
					Hide dismissed ({dismissedCount})
				</label>
			{/if}
		</div>
	{/if}
	<div class="flex-1 overflow-y-auto">
		{#if visibleFindings.length === 0}
			{#if scanning}
				<div class="space-y-1 px-4 py-6 text-xs">
					<p class="animate-pulse text-muted-foreground">{stage}</p>
					{#if hasWalk}
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
						class="text-muted-foreground underline underline-offset-2 hover:text-foreground"
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
						<p class="leading-relaxed text-muted-foreground/80">
							This file never reached the LLM. See detail pane for context.
						</p>
					{:else if selectedFileNode.status === 'triage_skipped'}
						<p>Triage skipped this file.</p>
						<p class="leading-relaxed text-muted-foreground/80">
							Haiku triage decided this file has no security surface. See detail pane for its
							reason.
						</p>
					{:else}
						<p class="text-destructive">Detect errored on this file.</p>
						<p class="leading-relaxed text-muted-foreground/80">
							See detail pane for the error message.
						</p>
					{/if}
				</div>
			{:else if stage === 'done' && allFindingsCount === 0}
				{#if detectErrors.size > 0}
					<div class="space-y-2 px-4 py-8 text-center text-sm">
						<div class="text-2xl leading-none text-destructive">!</div>
						<p class="font-medium">Scan finished with errors</p>
						<p class="text-xs leading-relaxed text-muted-foreground">
							{detectErrors.size} of {hasWalk ? walkCandidateCount : 0} file(s) couldn't be scanned. See
							the summary pane for details and retry options.
						</p>
					</div>
				{:else}
					<div class="space-y-2 px-4 py-8 text-center text-sm">
						<div class="text-2xl">✓</div>
						<p class="font-medium">Clean scan</p>
						<p class="text-xs leading-relaxed text-muted-foreground">
							No vulnerabilities found in
							{hasWalk ? walkCandidateCount : 0} file(s).
						</p>
					</div>
				{/if}
			{:else if stage === 'done' && selectedFile}
				<p class="px-4 py-4 text-xs text-muted-foreground">
					No findings in <span class="font-mono">{selectedFile}</span>.
				</p>
			{:else}
				<p class="px-4 py-4 text-xs text-muted-foreground">
					Pick a folder above and hit Scan to see findings here.
				</p>
			{/if}
		{:else}
			{#each visibleFindings as { f, rel } (f.id)}
				{@const status = findingStatus(f, statusInputs)}
				<button
					type="button"
					data-finding-id={f.id}
					class="block w-full border-b border-border px-4 py-3 text-left hover:bg-muted/40 {selectedFindingId ===
					f.id
						? 'bg-muted/60'
						: ''} {status === 'dismissed' || status === 'dropped' ? 'opacity-60' : ''}"
					onclick={() => onSelectFinding(f.id)}
				>
					<div class="flex items-start justify-between gap-2">
						<div class="flex-1 space-y-1">
							<div class="flex flex-wrap items-center gap-1.5">
								<FindingBadges finding={f} {statusInputs} />
								<span class="font-mono text-xs text-muted-foreground">{f.cwe}</span>
								{#if f.owasp}
									<span class="font-mono text-xs text-muted-foreground">
										· {f.owasp}
									</span>
								{/if}
							</div>
							<div class="text-sm font-medium">{f.title}</div>
							<div class="font-mono text-xs text-muted-foreground">
								{rel}:{f.line_start}{f.line_end !== f.line_start ? `-${f.line_end}` : ''}
							</div>
						</div>
					</div>
				</button>
			{/each}
		{/if}
	</div>
</section>
