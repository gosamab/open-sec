<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import FindingBadges from '$lib/components/FindingBadges.svelte';
	import { Input } from '$lib/components/ui/input';
	import { renderInlineMd, renderMd } from '$lib/markdown';
	import type { Excerpt, Finding, Patch, TriageRecord, TriageStatus, Verdict } from '$lib/ipc';
	import { referencesFor } from '$lib/references';
	import {
		diffLineClass,
		parseDataFlow,
		type FindingStatusInputs
	} from '$lib/scan-display';

	interface Props {
		finding: Finding;
		verdict: Verdict | null;
		hasVerdictKey: boolean;
		patch: Patch | null;
		patchVariants: Patch[];
		patchVariantIdx: number;
		triageRecord: TriageRecord | null;
		applied: boolean;
		dismissDraftActive: boolean;
		/** Bindable so the child's Input writes back into the parent without a
		 *  shadow $state (which Svelte 5 flags as state_referenced_locally). */
		dismissReason: string;
		triageBusy: boolean;
		applyBusy: boolean;
		applyError: string | null;
		regenBusy: boolean;
		regenError: string | null;
		excerpt: Excerpt | null;
		excerptHtml: string | null;
		excerptError: string | null;
		diffHtml: string | null;
		scanning: boolean;
		statusInputs: FindingStatusInputs;
		snoozeDays: number;
		onApplyTriage: (status: TriageStatus, reason?: string) => void;
		onClearTriage: () => void;
		onStartDismiss: () => void;
		onCancelDismiss: () => void;
		onSubmitDismiss: () => void;
		onApplyPatch: () => void;
		onRegenerate: () => void;
		onSelectVariant: (idx: number) => void;
	}
	let {
		finding,
		verdict,
		hasVerdictKey,
		patch,
		patchVariants,
		patchVariantIdx,
		triageRecord,
		applied,
		dismissDraftActive,
		dismissReason = $bindable(''),
		triageBusy,
		applyBusy,
		applyError,
		regenBusy,
		regenError,
		excerpt,
		excerptHtml,
		excerptError,
		diffHtml,
		scanning,
		statusInputs,
		snoozeDays,
		onApplyTriage,
		onClearTriage,
		onStartDismiss,
		onCancelDismiss,
		onSubmitDismiss,
		onApplyPatch,
		onRegenerate,
		onSelectVariant
	}: Props = $props();

	let dataFlowSteps = $derived(parseDataFlow(finding.data_flow));
	let refs = $derived(referencesFor(finding));
</script>

<article class="divide-y divide-border">
	<header class="space-y-2 px-5 py-4">
		<div class="flex flex-wrap items-center gap-1.5">
			<FindingBadges {finding} {statusInputs} showKind />
			<span class="font-mono text-xs text-muted-foreground">{finding.cwe}</span>
			{#if finding.owasp}
				<span class="font-mono text-xs text-muted-foreground">· OWASP {finding.owasp}</span>
			{/if}
		</div>
		<h2 class="text-base leading-snug font-semibold tracking-tight">{finding.title}</h2>
		<p class="font-mono text-xs break-all text-muted-foreground">
			{finding.file}:{finding.line_start}{finding.line_end !== finding.line_start
				? `-${finding.line_end}`
				: ''}
		</p>

		{#if dismissDraftActive}
			<div class="space-y-2 rounded-md border border-border p-2">
				<div class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					Reason for dismissal
				</div>
				<Input
					bind:value={dismissReason}
					placeholder="e.g. false positive: this param is server-controlled"
					class="h-8 text-xs"
					autofocus
					onkeydown={(e) => {
						if (e.key === 'Enter' && dismissReason.trim()) onSubmitDismiss();
						else if (e.key === 'Escape') onCancelDismiss();
					}}
				/>
				<div class="flex gap-2">
					<Button
						size="sm"
						onclick={onSubmitDismiss}
						disabled={!dismissReason.trim() || triageBusy}
					>
						Confirm dismiss
					</Button>
					<Button size="sm" variant="outline" onclick={onCancelDismiss}>Cancel</Button>
				</div>
			</div>
		{:else}
			<div class="flex flex-wrap gap-2 pt-1">
				{#if triageRecord?.status === 'accepted'}
					<Button size="sm" variant="outline" onclick={onClearTriage} disabled={triageBusy}>
						Un-accept
					</Button>
				{:else}
					<Button size="sm" onclick={() => onApplyTriage('accepted')} disabled={triageBusy}>
						Accept
					</Button>
				{/if}
				{#if triageRecord?.status === 'dismissed'}
					<Button size="sm" variant="outline" onclick={onClearTriage} disabled={triageBusy}>
						Un-dismiss
					</Button>
				{:else}
					<Button size="sm" variant="outline" onclick={onStartDismiss} disabled={triageBusy}>
						Dismiss…
					</Button>
				{/if}
				{#if triageRecord?.status === 'snoozed'}
					<Button size="sm" variant="outline" onclick={onClearTriage} disabled={triageBusy}>
						Un-snooze
					</Button>
				{:else}
					<Button
						size="sm"
						variant="outline"
						onclick={() => onApplyTriage('snoozed')}
						disabled={triageBusy}
					>
						Snooze {snoozeDays}d
					</Button>
				{/if}
			</div>
			{#if triageRecord?.status === 'dismissed' && triageRecord.reason}
				<p class="pt-1 text-xs text-muted-foreground italic">
					Reason: {triageRecord.reason}
				</p>
			{/if}
		{/if}
	</header>

	<section class="space-y-2 px-5 py-4">
		<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
			Description
		</h3>
		<div class="md text-sm leading-relaxed">
			{@html renderMd(finding.description)}
		</div>
	</section>

	{#if refs.length > 0}
		<section class="space-y-2 px-5 py-4">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				References
			</h3>
			<div class="flex flex-wrap gap-2">
				{#each refs as r (r.url)}
					<a
						href={r.url}
						target="_blank"
						rel="noopener noreferrer"
						data-md-link="external"
						class="inline-flex items-center gap-1.5 rounded-md border border-border px-2 py-1 text-xs no-underline transition-colors hover:border-foreground/40 hover:bg-muted/50"
					>
						<span class="font-mono">{r.label}</span>
						<span class="text-[0.625rem] text-muted-foreground">· {r.source}</span>
						<svg
							xmlns="http://www.w3.org/2000/svg"
							width="10"
							height="10"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							class="text-muted-foreground"
						>
							<path d="M7 17 17 7" />
							<path d="M7 7h10v10" />
						</svg>
					</a>
				{/each}
			</div>
		</section>
	{/if}

	<section class="space-y-2 px-5 py-4">
		<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
			Data flow
		</h3>
		<ol
			class="ml-5 list-decimal space-y-1 text-sm leading-relaxed marker:font-mono marker:text-xs marker:text-muted-foreground"
		>
			{#each dataFlowSteps as step, i (i)}
				<li class="md pl-1">{@html renderInlineMd(step)}</li>
			{/each}
		</ol>
	</section>

	{#if excerpt && excerpt.text.trim().length > 0}
		<section class="space-y-2 px-5 py-4">
			<div class="flex items-center justify-between gap-2">
				<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					{excerpt.source === 'enclosing_function' ? 'Enclosing function' : 'Excerpt'}
				</h3>
				<span class="font-mono text-[0.625rem] text-muted-foreground">
					L{excerpt.start_line}-{excerpt.end_line}
				</span>
			</div>
			{#if excerptHtml}
				<div class="shiki-wrap">{@html excerptHtml}</div>
			{:else}
				<pre
					class="overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-xs leading-relaxed">{excerpt.text}</pre>
			{/if}
		</section>
	{:else if excerptError}
		<section class="px-5 py-4">
			<p class="text-xs text-muted-foreground italic">Excerpt unavailable: {excerptError}</p>
		</section>
	{/if}

	{#if verdict}
		<section class="space-y-2 px-5 py-4">
			<div class="flex items-center justify-between gap-2">
				<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					Verifier
				</h3>
				<div class="flex items-center gap-1.5 text-xs">
					{#if verdict.is_reachable}
						<Badge class="bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">
							reachable
						</Badge>
					{:else}
						<Badge class="bg-zinc-400/15 text-zinc-500">not reachable</Badge>
					{/if}
					{#if verdict.source_is_untrusted}
						<Badge class="bg-amber-500/15 text-amber-700 dark:text-amber-300">
							untrusted source
						</Badge>
					{/if}
				</div>
			</div>
			<div class="md text-sm leading-relaxed text-muted-foreground">
				{@html renderMd(verdict.reasoning)}
			</div>
		</section>

		{#if verdict.concrete_exploit}
			{@const ex = verdict.concrete_exploit}
			<section class="space-y-2 px-5 py-4">
				<div class="flex items-center justify-between gap-2">
					<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
						Exploit
					</h3>
					<Badge variant="outline" class="font-mono text-[0.625rem]">{ex.kind}</Badge>
				</div>
				<p class="md text-sm">{@html renderInlineMd(ex.expected_effect)}</p>
				<div class="space-y-1 rounded-md bg-muted/40 p-3 font-mono text-xs">
					{#if ex.request}
						<div class="flex gap-2">
							<span class="w-14 shrink-0 text-muted-foreground">request</span>
							<span class="break-all">{ex.request.method} {ex.request.path}</span>
						</div>
					{/if}
					<div class="flex gap-2">
						<span class="w-14 shrink-0 text-muted-foreground">payload</span>
						<span class="break-all">{ex.payload}</span>
					</div>
				</div>
			</section>
		{/if}
	{/if}

	{#if patch}
		<section class="space-y-3 px-5 py-4">
			<div class="flex items-center justify-between gap-2">
				<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					Patch
				</h3>
				<div class="flex items-center gap-1.5">
					<Badge variant="outline" class="font-mono text-[0.625rem]">
						{patch.located.kind === 'not_found' ? 'not located' : patch.located.kind}
					</Badge>
					{#if applied}
						<Badge class="bg-emerald-500/15 text-emerald-700 dark:text-emerald-300">applied ✓</Badge
						>
					{/if}
				</div>
			</div>
			{#if patchVariants.length > 1}
				<div class="flex flex-wrap items-center gap-1">
					<span class="mr-1 text-[0.625rem] tracking-wider text-muted-foreground uppercase"
						>Variants</span
					>
					{#each patchVariants as _v, i (i)}
						<button
							type="button"
							onclick={() => onSelectVariant(i)}
							class="inline-flex h-5 min-w-5 items-center justify-center rounded px-1.5 font-mono text-[0.625rem] font-semibold {i ===
							patchVariantIdx
								? 'bg-foreground text-background'
								: 'bg-muted text-muted-foreground hover:bg-muted/80'}">v{i + 1}</button
						>
					{/each}
				</div>
			{/if}
			<div class="md text-sm leading-relaxed">
				{@html renderMd(patch.proposal.explanation)}
			</div>
			<div class="flex flex-wrap items-center gap-2">
				{#if applied}
					<Button size="sm" variant="outline" disabled>Applied to disk</Button>
					<span class="text-xs text-muted-foreground">Use git to review or revert.</span>
				{:else if patch.located.kind === 'not_found'}
					<Button size="sm" disabled>Cannot apply (not located)</Button>
				{:else}
					<Button size="sm" onclick={onApplyPatch} disabled={applyBusy}>
						{applyBusy ? 'Applying…' : 'Apply patch'}
					</Button>
					{#if patch.located.kind === 'fuzzy'}
						<span class="text-xs text-muted-foreground italic"
							>Fuzzy match — review the diff before applying.</span
						>
					{/if}
				{/if}
				<Button
					size="sm"
					variant="outline"
					onclick={onRegenerate}
					disabled={regenBusy || applied}
					title="Ask the patcher for a structurally different fix"
				>
					{regenBusy ? 'Generating…' : 'Try another fix'}
				</Button>
			</div>
			{#if regenError}
				<p class="text-xs text-destructive">Regenerate failed: {regenError}</p>
			{/if}
			{#if applyError}
				<p class="text-xs text-destructive">{applyError}</p>
			{/if}
			{#if patch.diff}
				{#if diffHtml}
					<div class="shiki-wrap">{@html diffHtml}</div>
				{:else}
					<pre
						class="overflow-auto rounded-md border border-border bg-muted/40 font-mono text-xs leading-relaxed">{#each patch.diff.split('\n') as line, i (i)}<div
								class="px-3 {diffLineClass(line)}">{line || ' '}</div>{/each}</pre>
				{/if}
			{:else}
				<div class="text-xs text-muted-foreground italic">
					old_block not located in current file — raw proposal below.
				</div>
				<pre
					class="overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-xs"><span
						class="text-red-700 dark:text-red-300">- {patch.proposal.old_block}</span
					>
{'\n'}<span class="text-green-700 dark:text-green-300">+ {patch.proposal.new_block}</span></pre>
			{/if}
		</section>
	{:else if !scanning && hasVerdictKey && verdict?.is_reachable === false}
		<section class="px-5 py-4">
			<p class="text-xs text-muted-foreground italic">Dropped by verifier — no patch generated.</p>
		</section>
	{/if}
</article>
