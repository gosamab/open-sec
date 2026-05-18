<script lang="ts">
	import { Check, ChevronRight, Loader } from 'lucide-svelte';
	import type { StageDurations } from '$lib/ipc';
	import { formatDuration } from '$lib/scan-display';
	import { PIPELINE_STAGES } from '$lib/pipeline';

	interface Props {
		/** -1 = idle, 0..PIPELINE_STAGES.length-1 = active, length = done. */
		stageIndex: number;
		/** Free-form stage label from the orchestrator, shown verbatim on the right. */
		stage: string;
		rateLimitNotice: { attempt: number; retry_after_secs: number } | null;
		durations: StageDurations;
	}
	let { stageIndex, stage, rateLimitNotice, durations }: Props = $props();

	function stateOf(i: number): 'done' | 'active' | 'pending' {
		if (stageIndex === PIPELINE_STAGES.length) return 'done';
		if (i < stageIndex) return 'done';
		if (i === stageIndex) return 'active';
		return 'pending';
	}
</script>

<div class="flex items-center gap-3 border-b border-border bg-muted/20 px-4 py-2">
	<ol class="flex flex-1 items-center gap-1">
		{#each PIPELINE_STAGES as s, i (s.key)}
			{@const st = stateOf(i)}
			<li class="flex items-center gap-1">
				<div
					class="flex items-center gap-1.5 rounded px-2 py-1 {st === 'active'
						? 'bg-foreground text-background'
						: st === 'done'
							? 'text-foreground'
							: 'text-muted-foreground/60'}"
				>
					<span class="flex h-3.5 w-3.5 shrink-0 items-center justify-center">
						{#if st === 'done'}
							<Check size={10} strokeWidth={3} />
						{:else if st === 'active'}
							<Loader size={10} strokeWidth={2.5} class="animate-spin" />
						{:else}
							<span class="font-mono text-[0.625rem]">{i + 1}</span>
						{/if}
					</span>
					<span class="text-xs font-medium">{s.label}</span>
				</div>
				{#if i < PIPELINE_STAGES.length - 1}
					<ChevronRight size={10} class="text-muted-foreground/30" />
				{/if}
			</li>
		{/each}
	</ol>
	{#if rateLimitNotice}
		<span
			class="inline-flex shrink-0 items-center gap-1 rounded bg-amber-500/15 px-2 py-0.5 font-mono text-xs text-amber-700 dark:text-amber-300"
			title="Anthropic rate limit; auto-retrying"
		>
			<Loader size={10} strokeWidth={2.5} class="animate-spin" />
			rate-limited · retry #{rateLimitNotice.attempt} in {rateLimitNotice.retry_after_secs}s
		</span>
	{/if}
	{#if durations.total_ms > 0}
		<span class="shrink-0 font-mono text-xs text-muted-foreground" title="Total scan duration">
			{formatDuration(durations.total_ms)}
		</span>
	{/if}
	<span class="shrink-0 font-mono text-xs text-muted-foreground">{stage}</span>
</div>
