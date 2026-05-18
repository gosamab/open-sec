<script lang="ts">
	import { Button } from '$lib/components/ui/button';

	interface Props {
		onMarkdown: () => void;
		onSarif: () => void;
	}
	let { onMarkdown, onSarif }: Props = $props();

	let open = $state(false);
	let menuRef = $state<HTMLDivElement | null>(null);

	$effect(() => {
		if (!open) return;
		const onDoc = (e: PointerEvent) => {
			if (menuRef && !menuRef.contains(e.target as Node)) open = false;
		};
		const onEsc = (e: KeyboardEvent) => {
			if (e.key === 'Escape') open = false;
		};
		document.addEventListener('pointerdown', onDoc);
		document.addEventListener('keydown', onEsc);
		return () => {
			document.removeEventListener('pointerdown', onDoc);
			document.removeEventListener('keydown', onEsc);
		};
	});

	function pick(action: () => void) {
		open = false;
		action();
	}
</script>

<div class="relative" bind:this={menuRef}>
	<Button
		size="sm"
		variant="outline"
		onclick={() => (open = !open)}
		aria-haspopup="menu"
		aria-expanded={open}
	>
		Export
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
			class="ml-1"
		>
			<path d="m6 9 6 6 6-6" />
		</svg>
	</Button>
	{#if open}
		<div
			class="absolute top-full right-0 z-10 mt-1 w-48 overflow-hidden rounded-md border border-border bg-popover text-popover-foreground shadow-md"
			role="menu"
		>
			<button
				type="button"
				role="menuitem"
				class="block w-full px-3 py-2 text-left text-xs hover:bg-muted"
				onclick={() => pick(onMarkdown)}
			>
				<div class="font-medium">Markdown</div>
				<div class="text-muted-foreground">.md report</div>
			</button>
			<button
				type="button"
				role="menuitem"
				class="block w-full px-3 py-2 text-left text-xs hover:bg-muted"
				onclick={() => pick(onSarif)}
			>
				<div class="font-medium">SARIF</div>
				<div class="text-muted-foreground">For CI</div>
			</button>
		</div>
	{/if}
</div>
