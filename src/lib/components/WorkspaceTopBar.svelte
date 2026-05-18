<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import ThemeToggle from '$lib/components/ThemeToggle.svelte';
	import ExportMenu from '$lib/components/ExportMenu.svelte';
	import logo from '$lib/assets/logo.png';
	import type { ScanResult } from '$lib/ipc';

	interface Props {
		root: string;
		scanning: boolean;
		cancelling: boolean;
		keyConfigured: boolean;
		scanResult: ScanResult | null;
		resultRoot: string | null;
		stage: string;
		canResume: boolean;
		onBack: () => void;
		onScan: () => void;
		onResume: () => void;
		onCancel: () => void;
		onOpenSettings: () => void;
		onExportMarkdown: () => void;
		onExportSarif: () => void;
	}
	let {
		root,
		scanning,
		cancelling,
		keyConfigured,
		scanResult,
		resultRoot,
		stage,
		canResume,
		onBack,
		onScan,
		onResume,
		onCancel,
		onOpenSettings,
		onExportMarkdown,
		onExportSarif
	}: Props = $props();

	let canExport = $derived(!!scanResult || !!resultRoot);
	let isScanComplete = $derived(!!scanResult || stage === 'done' || stage === 'cancelled');
</script>

<header class="flex items-center gap-3 border-b border-border px-4 py-2">
	<button
		type="button"
		onclick={onBack}
		class="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
		title="Back to start"
		aria-label="Back to start"
	>
		<svg
			xmlns="http://www.w3.org/2000/svg"
			width="14"
			height="14"
			viewBox="0 0 24 24"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			stroke-linecap="round"
			stroke-linejoin="round"
		>
			<path d="m15 18-6-6 6-6" />
		</svg>
	</button>
	<img src={logo} alt="" width="20" height="20" class="rounded-[5px]" />
	<h1 class="text-base font-semibold tracking-tight">Open Security</h1>
	<div class="h-5 w-px bg-border"></div>
	<div class="flex flex-1 items-center gap-2 truncate font-mono text-xs text-foreground/80">
		<span class="truncate" title={root}>{root}</span>
	</div>
	{#if scanning}
		<Button size="sm" variant="outline" onclick={onCancel} disabled={cancelling}>
			{cancelling ? 'Cancelling…' : 'Cancel'}
		</Button>
	{:else}
		{#if canResume}
			<Button
				size="sm"
				variant="outline"
				onclick={onResume}
				disabled={!keyConfigured}
				title="Continue from where the previous scan left off — skips files/findings already done"
			>
				Resume
			</Button>
		{/if}
		<Button size="sm" onclick={onScan} disabled={!root || !keyConfigured}>
			{isScanComplete ? 'Re-scan' : 'Scan'}
		</Button>
	{/if}
	{#if canExport}
		<ExportMenu onMarkdown={onExportMarkdown} onSarif={onExportSarif} />
	{/if}
	<div class="flex items-center gap-2 text-xs text-muted-foreground">
		<button
			type="button"
			onclick={onOpenSettings}
			class="inline-flex h-7 w-7 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
			title="Settings"
			aria-label="Settings"
		>
			<svg
				xmlns="http://www.w3.org/2000/svg"
				width="14"
				height="14"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				<circle cx="12" cy="12" r="3" />
				<path
					d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h0a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
				/>
			</svg>
		</button>
		<ThemeToggle />
	</div>
</header>
