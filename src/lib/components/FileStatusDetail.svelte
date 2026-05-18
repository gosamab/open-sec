<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { renderMd } from '$lib/markdown';
	import { humanizeError, skipReasonLabel } from '$lib/scan-display';
	import type { FileNode } from '$lib/tree';

	interface Props {
		node: FileNode;
	}
	let { node }: Props = $props();

	let humanized = $derived(node.detectError ? humanizeError(node.detectError) : null);
</script>

<article class="space-y-4 px-5 py-4">
	<header class="space-y-1">
		<div class="flex flex-wrap items-center gap-1.5">
			{#if node.status === 'errored'}
				<Badge class="bg-destructive text-destructive-foreground">errored</Badge>
			{:else if node.status === 'pre_triage_skipped'}
				<Badge class="bg-zinc-500/15 text-zinc-600 dark:text-zinc-300">pre-triage skip</Badge>
			{:else}
				<Badge class="bg-zinc-500/15 text-zinc-600 dark:text-zinc-300">triage skip</Badge>
			{/if}
		</div>
		<h2 class="text-base leading-snug font-semibold tracking-tight">{node.name}</h2>
		<p class="font-mono text-xs break-all text-muted-foreground">{node.path}</p>
	</header>

	{#if node.status === 'pre_triage_skipped' && node.skipReason}
		<section class="space-y-2">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Reason
			</h3>
			<p class="text-sm">{skipReasonLabel(node.skipReason)}</p>
			<p class="text-xs leading-relaxed text-muted-foreground">
				Filtered before triage by ingest heuristics — never sent to the LLM. Reasons include
				vendor/build directories, files over 500&nbsp;KB, binary content (null bytes), and minified
				output (avg line length &gt; 200).
			</p>
		</section>
	{:else if node.status === 'triage_skipped'}
		<section class="space-y-2">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Triage reason
			</h3>
			<div class="md text-sm leading-relaxed">
				{@html renderMd(node.triageReason ?? '(no reason emitted)')}
			</div>
			<p class="text-xs leading-relaxed text-muted-foreground">
				Haiku read this file and classified it as having no meaningful security surface — pure UI /
				types / config. It was not sent to detect.
			</p>
		</section>
	{:else if node.status === 'errored' && humanized}
		<section class="space-y-3">
			<div class="space-y-1">
				<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					Error
				</h3>
				<p class="text-sm font-medium">{humanized.title}</p>
				{#if humanized.detail}
					<p class="text-xs leading-relaxed text-muted-foreground">{humanized.detail}</p>
				{/if}
			</div>
			{#if node.detectError}
				<details class="text-xs">
					<summary class="cursor-pointer text-muted-foreground hover:text-foreground">
						Show technical details
					</summary>
					<pre
						class="mt-2 overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-[0.6875rem] whitespace-pre-wrap">{node.detectError}</pre>
				</details>
			{/if}
		</section>
	{/if}
</article>
