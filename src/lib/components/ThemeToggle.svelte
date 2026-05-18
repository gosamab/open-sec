<script lang="ts">
	import { theme, type ThemeChoice } from '$lib/theme.svelte';

	let menuOpen = $state(false);
	let menuRef = $state<HTMLDivElement | null>(null);

	$effect(() => {
		if (!menuOpen) return;
		const onDoc = (e: MouseEvent) => {
			if (menuRef && !menuRef.contains(e.target as Node)) menuOpen = false;
		};
		const onEsc = (e: KeyboardEvent) => {
			if (e.key === 'Escape') menuOpen = false;
		};
		document.addEventListener('mousedown', onDoc);
		document.addEventListener('keydown', onEsc);
		return () => {
			document.removeEventListener('mousedown', onDoc);
			document.removeEventListener('keydown', onEsc);
		};
	});

	function choose(c: ThemeChoice) {
		theme.set(c);
		menuOpen = false;
	}

	const OPTIONS: { value: ThemeChoice; label: string }[] = [
		{ value: 'system', label: 'System' },
		{ value: 'light', label: 'Light' },
		{ value: 'midnight', label: 'Midnight' },
		{ value: 'dark', label: 'Dark' }
	];
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
		{#if theme.resolved === 'light'}
			<!-- Sun -->
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
				<circle cx="12" cy="12" r="4" />
				<path d="M12 2v2" />
				<path d="M12 20v2" />
				<path d="m4.93 4.93 1.41 1.41" />
				<path d="m17.66 17.66 1.41 1.41" />
				<path d="M2 12h2" />
				<path d="M20 12h2" />
				<path d="m6.34 17.66-1.41 1.41" />
				<path d="m19.07 4.93-1.41 1.41" />
			</svg>
		{:else if theme.resolved === 'midnight'}
			<!-- Moon + star -->
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
				<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
				<path d="M19 3v4" />
				<path d="M21 5h-4" />
			</svg>
		{:else}
			<!-- Moon -->
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
				<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
			</svg>
		{/if}
	</button>

	{#if menuOpen}
		<div
			class="border-border bg-popover text-popover-foreground absolute right-0 top-full z-20 mt-1 w-48 overflow-hidden rounded-md border shadow-md"
			role="menu"
		>
			{#each OPTIONS as opt (opt.value)}
				{@const selected = theme.value === opt.value}
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
							{#if opt.value === 'system'}
								<!-- Monitor -->
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="13"
									height="13"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<rect x="2" y="3" width="20" height="14" rx="2" />
									<path d="M8 21h8" />
									<path d="M12 17v4" />
								</svg>
							{:else if opt.value === 'light'}
								<!-- Sun -->
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="13"
									height="13"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<circle cx="12" cy="12" r="4" />
									<path d="M12 2v2" />
									<path d="M12 20v2" />
									<path d="m4.93 4.93 1.41 1.41" />
									<path d="m17.66 17.66 1.41 1.41" />
									<path d="M2 12h2" />
									<path d="M20 12h2" />
									<path d="m6.34 17.66-1.41 1.41" />
									<path d="m19.07 4.93-1.41 1.41" />
								</svg>
							{:else if opt.value === 'midnight'}
								<!-- Moon + star -->
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="13"
									height="13"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
									<path d="M19 3v4" />
									<path d="M21 5h-4" />
								</svg>
							{:else}
								<!-- Moon -->
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="13"
									height="13"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
								</svg>
							{/if}
						</span>
						<span class="truncate font-medium">{opt.label}</span>
					</div>
					{#if selected}
						<svg
							xmlns="http://www.w3.org/2000/svg"
							width="12"
							height="12"
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="3"
							stroke-linecap="round"
							stroke-linejoin="round"
							class="shrink-0"
						>
							<path d="M20 6 9 17l-5-5" />
						</svg>
					{/if}
				</button>
			{/each}
		</div>
	{/if}
</div>
