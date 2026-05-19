<script lang="ts">
	import { ChevronRight, Folder, Play } from 'lucide-svelte';
	import { Button } from '$lib/components/ui/button';
	import EstimatePanel from '$lib/components/EstimatePanel.svelte';
	import { PIPELINE_STAGES } from '$lib/pipeline';
	import type { CostEstimate } from '$lib/estimate';

	interface Props {
		root: string;
		keyConfigured: boolean;
		costEstimate: CostEstimate | null;
		onScan: () => void;
	}
	let { root, keyConfigured, costEstimate, onScan }: Props = $props();
</script>

<div class="flex flex-1 justify-center overflow-y-auto px-8 pt-16 pb-10">
	<div class="flex w-full max-w-3xl flex-col gap-6">
		<div class="space-y-1.5">
			<h2 class="text-xl font-semibold tracking-tight">Ready to scan</h2>
			<p class="text-sm text-muted-foreground">
				An AI pipeline reads this folder and drafts patches. Nothing touches disk until you approve.
			</p>
		</div>

		<div class="flex items-center gap-3 rounded-md border border-border bg-muted/30 px-3.5 py-2.5">
			<Folder size={14} class="shrink-0 text-muted-foreground" />
			<span class="truncate font-mono text-xs" title={root}>{root || '—'}</span>
		</div>

		{#if costEstimate}
			<section class="space-y-2.5">
				<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					Estimate
				</h3>
				<EstimatePanel estimate={costEstimate} showStageBreakdown />
			</section>
		{/if}

		<section class="space-y-2.5">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Pipeline
			</h3>
			<div class="flex items-stretch gap-1.5">
				{#each PIPELINE_STAGES as step, i (step.key)}
					<div
						class="flex flex-1 flex-col gap-1 rounded-md border border-border bg-background px-3 py-2.5"
					>
						<div class="flex items-center justify-between">
							<span class="font-mono text-[0.625rem] text-muted-foreground/70">
								{i + 1}
							</span>
							{#if step.model}
								<span
									class="font-mono text-[0.5625rem] tracking-wider text-muted-foreground/70 uppercase"
								>
									{step.model}
								</span>
							{/if}
						</div>
						<div class="text-sm font-medium">{step.label}</div>
						<div class="text-[0.6875rem] text-muted-foreground">{step.desc}</div>
					</div>
					{#if i < PIPELINE_STAGES.length - 1}
						<div class="flex items-center text-muted-foreground/50">
							<ChevronRight size={10} />
						</div>
					{/if}
				{/each}
			</div>
		</section>

		<div class="space-y-2">
			<Button size="lg" onclick={onScan} disabled={!root || !keyConfigured} class="w-full">
				<Play size={14} class="mr-2" />
				Start scan
			</Button>
			<p class="text-center text-xs text-muted-foreground">
				{#if !keyConfigured}
					Add your Anthropic API key above to enable scanning.
				{:else if !costEstimate}
					Typically a few cents and under a minute for a small project.
				{:else}
					Estimate may differ from actual cost — prompt caching usually helps.
				{/if}
			</p>
		</div>
	</div>
</div>
