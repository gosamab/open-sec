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
		<h2 class="text-base font-semibold leading-snug tracking-tight">{node.name}</h2>
		<p class="text-muted-foreground break-all font-mono text-xs">{node.path}</p>
	</header>

	{#if node.status === 'pre_triage_skipped' && node.skipReason}
		<section class="space-y-2">
			<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
				Reason
			</h3>
			<p class="text-sm">{skipReasonLabel(node.skipReason)}</p>
			<p class="text-muted-foreground text-xs leading-relaxed">
				Filtered before triage by ingest heuristics — never sent to the LLM. Reasons include
				vendor/build directories, files over 500&nbsp;KB, binary content (null bytes), and
				minified output (avg line length &gt; 200).
			</p>
		</section>
	{:else if node.status === 'triage_skipped'}
		<section class="space-y-2">
			<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
				Triage reason
			</h3>
			<div class="md text-sm leading-relaxed">
				{@html renderMd(node.triageReason ?? '(no reason emitted)')}
			</div>
			<p class="text-muted-foreground text-xs leading-relaxed">
				Haiku read this file and classified it as having no meaningful security surface — pure
				UI / types / config. It was not sent to detect.
			</p>
		</section>
	{:else if node.status === 'errored' && humanized}
		<section class="space-y-3">
			<div class="space-y-1">
				<h3
					class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider"
				>
					Error
				</h3>
				<p class="text-sm font-medium">{humanized.title}</p>
				{#if humanized.detail}
					<p class="text-muted-foreground text-xs leading-relaxed">{humanized.detail}</p>
				{/if}
			</div>
			{#if node.detectError}
				<details class="text-xs">
					<summary class="text-muted-foreground hover:text-foreground cursor-pointer">
						Show technical details
					</summary>
					<pre
						class="bg-muted/40 border-border mt-2 overflow-auto whitespace-pre-wrap rounded-md border p-3 font-mono text-[0.6875rem]">{node.detectError}</pre>
				</details>
			{/if}
		</section>
	{/if}
</article>
