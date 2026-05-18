<script lang="ts">
	import { Check, Monitor, Moon, MoonStar, Sun } from 'lucide-svelte';
	import { theme, type ThemeChoice } from '$lib/theme.svelte';

	let menuOpen = $state(false);
	let menuRef = $state<HTMLDivElement | null>(null);

	$effect(() => {
		if (!menuOpen) return;
		// `pointerdown` covers mouse, touch, and pen — `mousedown` alone
		// misses tap-to-close on touch hardware.
		const onDoc = (e: PointerEvent) => {
			if (menuRef && !menuRef.contains(e.target as Node)) menuOpen = false;
		};
		const onEsc = (e: KeyboardEvent) => {
			if (e.key === 'Escape') menuOpen = false;
		};
		document.addEventListener('pointerdown', onDoc);
		document.addEventListener('keydown', onEsc);
		return () => {
			document.removeEventListener('pointerdown', onDoc);
			document.removeEventListener('keydown', onEsc);
		};
	});

	function choose(c: ThemeChoice) {
		theme.set(c);
		menuOpen = false;
	}

	const ICONS = {
		system: Monitor,
		light: Sun,
		midnight: MoonStar,
		dark: Moon
	} as const;

	const OPTIONS: { value: ThemeChoice; label: string }[] = [
		{ value: 'system', label: 'System' },
		{ value: 'light', label: 'Light' },
		{ value: 'midnight', label: 'Midnight' },
		{ value: 'dark', label: 'Dark' }
	];

	const TriggerIcon = $derived(
		theme.resolved === 'light' ? Sun : theme.resolved === 'midnight' ? MoonStar : Moon
	);
</script>

<div class="relative" bind:this={menuRef}>
	<button
		type="button"
		onclick={() => (menuOpen = !menuOpen)}
		class="hover:bg-muted text-muted-foreground hover:text-foreground inline-flex h-7 w-7 items-center justify-center rounded transition-colors"
		title="Theme: {theme.value}"
		aria-label="Theme"
		aria-haspopup="menu"
		aria-expanded={menuOpen}
	>
		<TriggerIcon size={14} />
	</button>

	{#if menuOpen}
		<div
			class="border-border bg-popover text-popover-foreground absolute right-0 top-full z-20 mt-1 w-48 overflow-hidden rounded-md border shadow-md"
			role="menu"
		>
			{#each OPTIONS as opt (opt.value)}
				{@const selected = theme.value === opt.value}
				{@const Icon = ICONS[opt.value]}
				<button
					type="button"
					role="menuitemradio"
					aria-checked={selected}
					class="hover:bg-muted flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-xs {selected
						? 'bg-muted/60'
						: ''}"
					onclick={() => choose(opt.value)}
				>
					<div class="flex min-w-0 flex-1 items-center gap-2">
						<span class="text-muted-foreground inline-flex h-4 w-4 shrink-0 items-center justify-center">
							<Icon size={13} />
						</span>
						<span class="truncate font-medium">{opt.label}</span>
					</div>
					{#if selected}
						<Check size={12} class="shrink-0" />
					{/if}
				</button>
			{/each}
		</div>
	{/if}
</div>
