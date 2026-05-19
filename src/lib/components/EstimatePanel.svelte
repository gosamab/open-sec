<script lang="ts">
	/**
	 * Shared cost/time/LoC readout for a `CostEstimate`. Rendered both inline
	 * on the onboarding panel and inside the rescan confirmation dialog —
	 * keep the layout here so the two consumers stay in sync.
	 */
	import { settings } from '$lib/settings.svelte';
	import { formatSAR, formatUSD, usdToSAR } from '$lib/pricing';
	import { formatDuration } from '$lib/scan-display';
	import type { CostEstimate } from '$lib/estimate';

	interface Props {
		estimate: CostEstimate | null;
		/** When true, lists per-stage USD rows between LoC and the totals. */
		showStageBreakdown?: boolean;
		/** Background tone — `card` for page sections, `subtle` for dialogs. */
		variant?: 'card' | 'subtle';
	}
	let { estimate, showStageBreakdown = false, variant = 'card' }: Props = $props();

	let totalSAR = $derived(
		estimate ? usdToSAR(estimate.costs.total, settings.value.sar_per_usd) : 0
	);
</script>

{#if estimate}
	{#if estimate.candidate_files === 0}
		<p class="text-[0.6875rem] leading-relaxed text-muted-foreground/80">
			No scannable files found in this folder.
		</p>
	{:else}
		<div
			class="grid grid-cols-[1fr_auto] gap-x-4 gap-y-1 rounded-md border border-border {variant ===
			'subtle'
				? 'bg-muted/20 px-3.5 py-2.5'
				: 'bg-background px-4 py-3'}"
		>
			<span class="text-xs text-muted-foreground">Files to scan</span>
			<span class="text-right font-mono text-xs tabular-nums">
				{estimate.candidate_files.toLocaleString()}
				{#if estimate.skipped_files > 0}
					<span class="text-muted-foreground/60">
						· {estimate.skipped_files.toLocaleString()} skipped
					</span>
				{/if}
			</span>

			<span class="text-xs text-muted-foreground">Lines of code</span>
			<span class="text-right font-mono text-xs tabular-nums">
				{estimate.total_loc.toLocaleString()}
			</span>

			<div class="col-span-2 my-1.5 border-t border-border/60"></div>

			{#if showStageBreakdown}
				<span class="text-xs text-muted-foreground">Triage</span>
				<span class="text-right font-mono text-xs tabular-nums text-muted-foreground">
					~{formatUSD(estimate.costs.triage)}
				</span>
				<span class="text-xs text-muted-foreground">Detect</span>
				<span class="text-right font-mono text-xs tabular-nums text-muted-foreground">
					~{formatUSD(estimate.costs.detect)}
				</span>
				<span class="text-xs text-muted-foreground">Verify</span>
				<span class="text-right font-mono text-xs tabular-nums text-muted-foreground">
					~{formatUSD(estimate.costs.verify)}
				</span>
				<span class="text-xs text-muted-foreground">Patch</span>
				<span class="text-right font-mono text-xs tabular-nums text-muted-foreground">
					~{formatUSD(estimate.costs.patch)}
				</span>

				<div class="col-span-2 my-1.5 border-t border-border/60"></div>
			{/if}

			<span class="text-xs font-medium">Estimated cost</span>
			<span class="text-right font-mono text-xs font-semibold tabular-nums">
				~{formatUSD(estimate.costs.total)}
				<span class="text-muted-foreground/80">· ~{formatSAR(totalSAR)}</span>
			</span>

			<span class="text-xs text-muted-foreground">Estimated time</span>
			<span class="text-right font-mono text-xs tabular-nums">
				~{formatDuration(estimate.time.total_sec * 1000)}
			</span>
		</div>
		<p class="text-[0.6875rem] leading-relaxed text-muted-foreground/80">
			Rough estimate from line counts. Prompt caching usually lowers the
			real bill; complex files with many findings push it up.
		</p>
	{/if}
{/if}
