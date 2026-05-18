<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import type { ScanResult, Severity, StageDurations, StageUsage, WalkResult } from '$lib/ipc';
	import {
		compactTokens,
		formatDuration,
		humanizeError,
		severityDot,
		skipReasonLabel,
		statusDotClass,
		STATUS_OPTIONS,
		type FindingStatus
	} from '$lib/scan-display';

	export type UsageRow = { name: string; u: StageUsage['triage']; ms: number };

	interface Props {
		scanResult: ScanResult | null;
		scanning: boolean;
		stage: string;
		keyConfigured: boolean;
		root: string;
		walk: WalkResult | null;
		patchCount: number;
		allFindingsTotal: number;
		severityCounts: Record<Severity, number>;
		totals: Record<FindingStatus, number>;
		durations: StageDurations;
		usage: StageUsage;
		usageRows: UsageRow[];
		totalTokens: number;
		totalFileNodes: number;
		/** rel_path → raw error message, surfaced as a collapsible section so
		 *  partial-failure scans (some files OK, some errored) don't bury the
		 *  errors behind the file tree. */
		detectErrors: Map<string, string>;
		/** rel_paths currently being re-run via the per-row retry button. */
		retryingFiles: Set<string>;
		/** Whether a "Retry all" sweep is in progress. Disables the button. */
		retryingAll: boolean;
		onRunScan: () => void;
		onSelectFile: (rel: string) => void;
		onRetryDetect: (rel: string) => void;
		onRetryAll: () => void;
	}
	let {
		scanResult,
		scanning,
		stage,
		keyConfigured,
		root,
		walk,
		patchCount,
		allFindingsTotal,
		severityCounts,
		totals,
		durations,
		usage,
		usageRows,
		totalTokens,
		totalFileNodes,
		detectErrors,
		retryingFiles,
		retryingAll,
		onRunScan,
		onSelectFile,
		onRetryDetect,
		onRetryAll
	}: Props = $props();

	const SEVERITY_ROWS: { key: Severity; label: string }[] = [
		{ key: 'critical', label: 'Critical' },
		{ key: 'high', label: 'High' },
		{ key: 'medium', label: 'Medium' },
		{ key: 'low', label: 'Low' }
	];
</script>

<div class="p-4">
	<div class="space-y-6">
		{#if !scanResult && !scanning}
			<div class="space-y-3 rounded-md border border-dashed p-6 text-center">
				<p class="text-xs text-muted-foreground">
					Ready to scan this folder. The pipeline runs triage → detect → verify → patch and persists
					the result.
				</p>
				<Button size="lg" onclick={onRunScan} disabled={!root || !keyConfigured} class="w-full">
					Scan this folder
				</Button>
				{#if !keyConfigured}
					<p class="text-xs text-muted-foreground">
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
				<p class="font-mono text-xs break-all text-muted-foreground" title={root}>
					{root || '—'}
				</p>
			</div>

			{#if detectErrors.size > 0}
				<details
					open
					class="group overflow-hidden rounded-md border border-destructive/40 bg-destructive/5"
				>
					<summary
						class="flex cursor-pointer items-center justify-between gap-2 px-3 py-2 hover:bg-destructive/10 [&::-webkit-details-marker]:hidden"
					>
						<div class="flex min-w-0 items-center gap-2">
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
								class="shrink-0 text-muted-foreground transition-transform group-open:rotate-90"
							>
								<path d="m9 18 6-6-6-6" />
							</svg>
							<span class="truncate text-xs font-medium text-destructive">
								Detect errors ({detectErrors.size})
							</span>
						</div>
						<div class="flex shrink-0 items-center gap-2">
							<button
								type="button"
								class="inline-flex h-5 items-center gap-1 rounded px-1.5 text-[0.625rem] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
								disabled={retryingAll || retryingFiles.size > 0 || scanning}
								onclick={(e) => {
									// Without this, the click would also toggle the <details>.
									e.preventDefault();
									e.stopPropagation();
									onRetryAll();
								}}
								title="Re-run detect on every errored file"
							>
								{#if retryingAll}
									<svg
										xmlns="http://www.w3.org/2000/svg"
										width="9"
										height="9"
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
									Retrying…
								{:else}
									<svg
										xmlns="http://www.w3.org/2000/svg"
										width="9"
										height="9"
										viewBox="0 0 24 24"
										fill="none"
										stroke="currentColor"
										stroke-width="2.5"
										stroke-linecap="round"
										stroke-linejoin="round"
									>
										<path d="M3 12a9 9 0 1 0 3-6.7" />
										<path d="M3 4v5h5" />
									</svg>
									Retry all
								{/if}
							</button>
							<span class="font-mono text-[0.625rem] text-muted-foreground/80">
								{detectErrors.size} of {walk?.candidates.length ?? 0} file(s)
							</span>
						</div>
					</summary>
					<ul class="max-h-72 space-y-1 overflow-y-auto border-t border-destructive/30 p-1">
						{#each Array.from(detectErrors) as [rel, msg] (rel)}
							{@const h = humanizeError(msg)}
							{@const retrying = retryingFiles.has(rel)}
							<li class="rounded px-2 py-2 hover:bg-muted/40">
								<div class="flex items-start justify-between gap-2">
									<button
										type="button"
										class="flex-1 truncate text-left text-xs text-primary hover:underline"
										onclick={() => onSelectFile(rel)}
										title={rel}
									>
										<span class="font-mono">{rel}</span>
									</button>
									<button
										type="button"
										class="inline-flex h-5 shrink-0 items-center gap-1 rounded px-1.5 text-[0.625rem] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
										disabled={retrying || scanning}
										onclick={() => onRetryDetect(rel)}
										title="Re-run detect on this file (no verify/patch)"
									>
										{#if retrying}
											<svg
												xmlns="http://www.w3.org/2000/svg"
												width="9"
												height="9"
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
											Retrying…
										{:else}
											<svg
												xmlns="http://www.w3.org/2000/svg"
												width="9"
												height="9"
												viewBox="0 0 24 24"
												fill="none"
												stroke="currentColor"
												stroke-width="2.5"
												stroke-linecap="round"
												stroke-linejoin="round"
											>
												<path d="M3 12a9 9 0 1 0 3-6.7" />
												<path d="M3 4v5h5" />
											</svg>
											Retry
										{/if}
									</button>
								</div>
								<div class="mt-1 text-xs">{h.title}</div>
								{#if h.detail}
									<div class="mt-0.5 text-[0.6875rem] leading-relaxed text-muted-foreground">
										{h.detail}
									</div>
								{/if}
							</li>
						{/each}
					</ul>
				</details>
			{/if}

			<dl class="grid grid-cols-[1fr_auto] gap-x-3 text-xs">
				<dt class="py-0.5 text-muted-foreground">Files scanned</dt>
				<dd class="py-0.5 text-right font-mono tabular-nums">
					{walk?.candidates.length ?? 0}{#if walk && walk.skipped.length > 0}<span
							class="text-muted-foreground/70"
						>
							· {walk.skipped.length} skipped</span
						>{/if}
				</dd>
				{#if detectErrors.size > 0}
					<dt class="py-0.5 text-destructive">Detect errors</dt>
					<dd class="py-0.5 text-right font-mono text-destructive tabular-nums">
						{detectErrors.size}
					</dd>
				{/if}
				<dt class="py-0.5 text-muted-foreground">Patches drafted</dt>
				<dd class="py-0.5 text-right font-mono tabular-nums">{patchCount}</dd>
				{#if durations.total_ms > 0}
					<dt class="py-0.5 text-muted-foreground">Duration</dt>
					<dd class="py-0.5 text-right font-mono tabular-nums">
						{formatDuration(durations.total_ms)}
					</dd>
				{/if}
			</dl>

			{#if allFindingsTotal > 0}
				<div class="space-y-1.5">
					<div class="flex items-baseline justify-between gap-2">
						<div class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
							Severity
						</div>
						<div class="text-[0.625rem] text-muted-foreground/70">
							{allFindingsTotal} total
						</div>
					</div>
					<dl class="grid grid-cols-[1fr_auto] gap-x-3 text-xs">
						{#each SEVERITY_ROWS as row (row.key)}
							{@const n = severityCounts[row.key]}
							<dt
								class="flex items-center gap-2 py-0.5 {n === 0 ? 'text-muted-foreground/50' : ''}"
							>
								<span
									class="h-2 w-2 shrink-0 rounded-full {severityDot(row.key)} {n === 0
										? 'opacity-30'
										: ''}"
								></span>
								{row.label}
							</dt>
							<dd
								class="py-0.5 text-right font-mono tabular-nums {n === 0
									? 'text-muted-foreground/50'
									: ''}"
							>
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
					<div class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
						Status
					</div>
					<div class="text-[0.625rem] text-muted-foreground/70">one per finding</div>
				</div>
				<dl class="grid grid-cols-[1fr_auto] gap-x-3 text-xs">
					{#each STATUS_OPTIONS as row (row.key)}
						{@const n = totals[row.key]}
						{#if n > 0 || row.key === 'open' || row.key === 'patched'}
							<dt
								class="flex items-center gap-2 py-0.5 {n === 0 ? 'text-muted-foreground/50' : ''}"
							>
								<span
									class="h-2 w-2 shrink-0 rounded-full {statusDotClass(row.key)} {n === 0
										? 'opacity-30'
										: ''}"
								></span>
								{row.label}
							</dt>
							<dd
								class="py-0.5 text-right font-mono tabular-nums {n === 0
									? 'text-muted-foreground/50'
									: ''}"
							>
								{n}
							</dd>
						{/if}
					{/each}
				</dl>
			</div>
		</div>

		{#if totalTokens > 0 || durations.total_ms > 0}
			<details class="group overflow-hidden rounded-md border border-border">
				<summary
					class="flex cursor-pointer items-center justify-between px-3 py-2 hover:bg-muted/30 [&::-webkit-details-marker]:hidden"
				>
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
					<span class="font-mono text-xs text-muted-foreground">
						{compactTokens(totalTokens)}
					</span>
				</summary>
				<div class="border-t border-border bg-muted/10">
					<table class="w-full text-xs">
						<thead>
							<tr
								class="border-b border-border/60 text-[0.625rem] tracking-wider text-muted-foreground/70 uppercase"
							>
								<th class="px-3 py-1.5 text-left font-medium">Stage</th>
								<th class="px-2 py-1.5 text-right font-medium">In</th>
								<th class="px-2 py-1.5 text-right font-medium">Out</th>
								<th class="px-2 py-1.5 text-right font-medium">Cache</th>
								<th class="px-3 py-1.5 text-right font-medium">Time</th>
							</tr>
						</thead>
						<tbody class="font-mono">
							{#if durations.ingest_ms > 0}
								<tr class="border-b border-border/30 text-muted-foreground/80 last:border-b-0">
									<td class="px-3 py-1">ingest</td>
									<td class="px-2 py-1 text-right tabular-nums">—</td>
									<td class="px-2 py-1 text-right tabular-nums">—</td>
									<td class="px-2 py-1 text-right tabular-nums">—</td>
									<td class="px-3 py-1 text-right tabular-nums">
										{formatDuration(durations.ingest_ms)}
									</td>
								</tr>
							{/if}
							{#each usageRows as row (row.name)}
								<tr class="border-b border-border/30 last:border-b-0">
									<td class="px-3 py-1">{row.name}</td>
									<td class="px-2 py-1 text-right tabular-nums">
										{row.u.input_tokens.toLocaleString()}
									</td>
									<td class="px-2 py-1 text-right tabular-nums">
										{row.u.output_tokens.toLocaleString()}
									</td>
									<td class="px-2 py-1 text-right text-muted-foreground tabular-nums">
										{row.u.cache_read_input_tokens.toLocaleString()}
									</td>
									<td class="px-3 py-1 text-right tabular-nums">
										{row.ms > 0 ? formatDuration(row.ms) : '—'}
									</td>
								</tr>
							{/each}
						</tbody>
						<tfoot>
							<tr class="border-t border-border/60 bg-muted/30 font-semibold">
								<td class="px-3 py-1.5">Total</td>
								<td class="px-2 py-1.5 text-right tabular-nums">
									{usage.total.input_tokens.toLocaleString()}
								</td>
								<td class="px-2 py-1.5 text-right tabular-nums">
									{usage.total.output_tokens.toLocaleString()}
								</td>
								<td class="px-2 py-1.5 text-right text-muted-foreground tabular-nums">
									{usage.total.cache_read_input_tokens.toLocaleString()}
								</td>
								<td class="px-3 py-1.5 text-right tabular-nums">
									{durations.total_ms > 0 ? formatDuration(durations.total_ms) : '—'}
								</td>
							</tr>
						</tfoot>
					</table>
				</div>
			</details>
		{/if}

		{#if walk && walk.skipped.length > 0}
			<details class="text-sm">
				<summary class="cursor-pointer text-xs text-muted-foreground">
					Pre-triage skips ({walk.skipped.length})
				</summary>
				<ul class="mt-2 space-y-0.5 font-mono text-xs text-muted-foreground">
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
			<p class="text-xs text-muted-foreground">Select a folder above and hit Scan to begin.</p>
		{/if}
	</div>
</div>
