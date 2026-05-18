<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { theme } from '$lib/theme.svelte';
	import { installExternalLinkHandler } from '$lib/markdown';

	let { children } = $props();

	// Mirror theme state onto <html class="dark midnight"> whenever it changes.
	$effect(() => {
		if (typeof document !== 'undefined') theme.apply(document);
	});

	// Track OS color-scheme so the 'system' choice flips live.
	$effect(() => theme.watchSystem());

	// Route every LLM-rendered <a> click through the Rust shell so the webview
	// never navigates to an external page. Idempotent — safe to call from
	// both routes.
	$effect(() => installExternalLinkHandler());
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>
{@render children()}
