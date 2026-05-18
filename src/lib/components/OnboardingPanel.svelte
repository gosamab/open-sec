<script lang="ts">
	import { ChevronRight, Folder, Play } from 'lucide-svelte';
	import { Button } from '$lib/components/ui/button';
	import { PIPELINE_STAGES } from '$lib/pipeline';

	interface Props {
		root: string;
		keyConfigured: boolean;
		onScan: () => void;
	}
	let { root, keyConfigured, onScan }: Props = $props();
</script>

<div class="flex flex-1 items-center justify-center overflow-y-auto px-8 py-10">
	<div class="flex w-full max-w-3xl flex-col gap-6">
		<div class="space-y-1.5">
			<h2 class="text-xl font-semibold tracking-tight">Ready to scan</h2>
			<p class="text-muted-foreground text-sm">
				An AI pipeline reads this folder and drafts patches. Nothing touches disk until you
				approve.
			</p>
		</div>

		<div
			class="border-border bg-muted/30 flex items-center gap-3 rounded-md border px-3.5 py-2.5"
		>
			<Folder size={14} class="text-muted-foreground shrink-0" />
			<span class="truncate font-mono text-xs" title={root}>{root || '—'}</span>
		</div>

		<section class="space-y-2.5">
			<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
				Pipeline
			</h3>
			<div class="flex items-stretch gap-1.5">
				{#each PIPELINE_STAGES as step, i (step.key)}
					<div
						class="border-border bg-background flex flex-1 flex-col gap-1 rounded-md border px-3 py-2.5"
					>
						<div class="flex items-center justify-between">
							<span class="text-muted-foreground/70 font-mono text-[0.625rem]">
								{i + 1}
							</span>
							{#if step.model}
								<span
									class="text-muted-foreground/70 font-mono text-[0.5625rem] uppercase tracking-wider"
								>
									{step.model}
								</span>
							{/if}
						</div>
						<div class="text-sm font-medium">{step.label}</div>
						<div class="text-muted-foreground text-[0.6875rem]">{step.desc}</div>
					</div>
					{#if i < PIPELINE_STAGES.length - 1}
						<div class="text-muted-foreground/50 flex items-center">
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
