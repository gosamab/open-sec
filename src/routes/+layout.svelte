<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import UpdateBanner from '$lib/components/UpdateBanner.svelte';
	import { theme } from '$lib/theme.svelte';
	import { page } from '$app/state';

	let { children } = $props();

	// Mirror theme state onto <html class="dark"> whenever it changes.
	$effect(() => {
		if (typeof document !== 'undefined') theme.apply(document);
	});

	// Don't mount the updater inside the /report popup window — the parent
	// already owns that responsibility.
	let isMainView = $derived(page.url.pathname !== '/report');
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>
{@render children()}
{#if isMainView}
	<UpdateBanner />
{/if}
