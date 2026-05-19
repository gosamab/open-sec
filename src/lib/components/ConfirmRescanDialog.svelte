<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Play } from 'lucide-svelte';
	import EstimatePanel from '$lib/components/EstimatePanel.svelte';
	import type { CostEstimate } from '$lib/estimate';

	interface Props {
		root: string;
		estimate: CostEstimate | null;
		onConfirm: () => void;
		onCancel: () => void;
	}
	let { root, estimate, onConfirm, onCancel }: Props = $props();

	let dialog = $state<HTMLDivElement | null>(null);
	const opener =
		typeof document !== 'undefined' ? (document.activeElement as HTMLElement | null) : null;

	$effect(() => {
		if (!dialog) return;
		// Focus the primary action on open so the user can press Enter to confirm.
		const primary = dialog.querySelector<HTMLElement>('[data-primary]');
		primary?.focus();
		return () => {
			opener?.focus?.();
		};
	});

	function onWindowKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			onCancel();
			return;
		}
		if (e.key !== 'Tab' || !dialog) return;
		// Roll our own focus trap, same pattern as Settings.svelte.
		const focusables = Array.from(
			dialog.querySelectorAll<HTMLElement>(
				'a[href], button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])'
			)
		).filter((el) => !el.hasAttribute('aria-hidden') && el.offsetParent !== null);
		if (focusables.length === 0) return;
		const first = focusables[0];
		const last = focusables[focusables.length - 1];
		const active = document.activeElement as HTMLElement | null;
		if (e.shiftKey && active === first) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && active === last) {
			e.preventDefault();
			first.focus();
		}
	}

	function basename(p: string): string {
		const idx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
		return idx >= 0 ? p.slice(idx + 1) : p;
	}
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="fixed inset-0 z-50 flex items-center justify-center p-6">
	<button
		type="button"
		class="absolute inset-0 bg-background/80 backdrop-blur-sm"
		aria-label="Cancel rescan"
		tabindex="-1"
		onclick={onCancel}
	></button>
	<div
		bind:this={dialog}
		class="relative w-full max-w-md space-y-5 rounded-lg border border-border bg-background p-6 shadow-xl"
		role="dialog"
		aria-modal="true"
		aria-label="Confirm rescan"
	>
		<header class="space-y-1">
			<h2 class="text-base font-semibold tracking-tight">Re-scan this project?</h2>
			<p class="font-mono text-xs break-all text-muted-foreground" title={root}>
				{basename(root) || root || '—'}
			</p>
		</header>

		{#if estimate}
			<EstimatePanel {estimate} variant="subtle" />
		{:else}
			<p class="text-sm text-muted-foreground">Computing estimate…</p>
		{/if}

		<footer class="flex items-center justify-end gap-2 border-t border-border pt-4">
			<Button variant="outline" size="sm" onclick={onCancel}>Cancel</Button>
			<Button
				size="sm"
				data-primary
				onclick={onConfirm}
				disabled={estimate?.candidate_files === 0}
			>
				<Play class="mr-1.5 h-3.5 w-3.5" />
				Start scan
			</Button>
		</footer>
	</div>
</div>
