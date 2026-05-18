<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';
	import { Button } from '$lib/components/ui/button';
	import ThemeToggle from '$lib/components/ThemeToggle.svelte';
	import ApiKeyPrompt from '$lib/components/ApiKeyPrompt.svelte';
	import logo from '$lib/assets/logo.png';
	import { deleteScansForRoot, hasAnthropicKey, listScanGroups, type ScanGroup } from '$lib/ipc';
	import { onMount } from 'svelte';

	interface Props {
		onOpenFresh: (path: string) => void;
		onOpenPast: (group: ScanGroup) => void;
	}
	let { onOpenFresh, onOpenPast }: Props = $props();

	let keyConfigured = $state(false);

	let groups = $state<ScanGroup[]>([]);
	let loadingGroups = $state(true);

	onMount(async () => {
		keyConfigured = await hasAnthropicKey();
		await reloadGroups();
	});

	async function reloadGroups() {
		loadingGroups = true;
		try {
			groups = await listScanGroups(20);
		} catch (e) {
			console.error('listScanGroups failed', e);
		} finally {
			loadingGroups = false;
		}
	}

	async function pickNew() {
		const picked = await open({
			directory: true,
			multiple: false,
			title: 'Choose a folder to scan'
		});
		if (typeof picked === 'string') onOpenFresh(picked);
	}

	function handleKeydown(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'o') {
			const target = e.target as HTMLElement | null;
			if (target && /^(input|textarea)$/i.test(target.tagName)) return;
			if (!keyConfigured) return;
			e.preventDefault();
			void pickNew();
		}
	}

	function basename(p: string): string {
		const idx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
		return idx >= 0 ? p.slice(idx + 1) : p;
	}

	function parentDir(p: string): string {
		const idx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
		return idx > 0 ? p.slice(0, idx) : '';
	}

	function relativeTime(ms: number): string {
		const diff = Date.now() - ms;
		const sec = Math.floor(diff / 1000);
		if (sec < 60) return `${sec}s ago`;
		const min = Math.floor(sec / 60);
		if (min < 60) return `${min}m ago`;
		const hr = Math.floor(min / 60);
		if (hr < 24) return `${hr}h ago`;
		const d = Math.floor(hr / 24);
		if (d < 30) return `${d}d ago`;
		return new Date(ms).toLocaleDateString();
	}

	async function removeGroup(e: Event, g: ScanGroup) {
		e.stopPropagation();
		try {
			await deleteScansForRoot(g.root);
			await reloadGroups();
		} catch (err) {
			console.error('deleteScansForRoot failed', err);
		}
	}

	async function clearAll() {
		// Fan out the deletes in parallel — each is an independent SQLite
		// transaction so order doesn't matter, and the latency adds up on a
		// long Recents list.
		await Promise.all(
			groups.map((g) =>
				deleteScansForRoot(g.root).catch((e) =>
					console.error('deleteScansForRoot failed', g.root, e)
				)
			)
		);
		await reloadGroups();
	}

	async function refreshKeyState() {
		keyConfigured = await hasAnthropicKey();
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="flex h-screen flex-col bg-background">
	<!-- Topbar (theme toggle only) -->
	<header class="flex items-center justify-end px-4 py-3">
		<ThemeToggle />
	</header>

	<main class="mx-auto flex w-full max-w-2xl flex-1 flex-col gap-8 px-6 pt-4 pb-12">
		<!-- Branding -->
		<div class="space-y-2">
			<div class="flex items-center gap-3">
				<img src={logo} alt="" width="40" height="40" class="rounded-lg" />
				<h1 class="text-3xl font-semibold tracking-tight">Open Security</h1>
			</div>
			<p class="text-sm text-muted-foreground">
				AI-powered security code scanner. Pick a folder to begin.
			</p>
		</div>

		{#if !keyConfigured}
			<ApiKeyPrompt variant="card" onSaved={refreshKeyState} />
		{/if}

		<!-- New project -->
		<div>
			<Button onclick={pickNew} disabled={!keyConfigured} class="w-full justify-start" size="lg">
				<span class="mr-3 inline-flex h-5 w-5 items-center justify-center">
					<svg
						xmlns="http://www.w3.org/2000/svg"
						width="16"
						height="16"
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
					>
						<path d="M12 5v14" />
						<path d="M5 12h14" />
					</svg>
				</span>
				<span class="flex-1 text-left">New project</span>
				<span class="text-xs text-primary-foreground/60">⌘O</span>
			</Button>
		</div>

		<!-- Recents -->
		<section class="flex flex-col gap-2">
			<div class="flex items-center justify-between">
				<h2 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					Recent projects
				</h2>
				{#if groups.length > 0}
					<button
						type="button"
						class="text-xs text-muted-foreground hover:text-foreground"
						onclick={clearAll}
					>
						Clear all
					</button>
				{/if}
			</div>
			{#if loadingGroups}
				<p
					class="rounded-md border border-dashed border-border px-4 py-6 text-center text-xs text-muted-foreground"
				>
					Loading…
				</p>
			{:else if groups.length === 0}
				<p
					class="rounded-md border border-dashed border-border px-4 py-6 text-center text-xs text-muted-foreground"
				>
					No projects scanned yet.
				</p>
			{:else}
				<div class="divide-y divide-border rounded-md border">
					{#each groups as g (g.root)}
						<div
							class="group flex cursor-pointer items-center gap-3 px-4 py-2.5 hover:bg-muted/50"
							role="button"
							tabindex="0"
							onclick={() => onOpenPast(g)}
							onkeydown={(e) => {
								if (e.key === 'Enter' || e.key === ' ') {
									e.preventDefault();
									onOpenPast(g);
								}
							}}
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
								class="shrink-0 text-muted-foreground"
							>
								<path
									d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
								/>
							</svg>
							<div class="flex-1 truncate">
								<div class="truncate text-sm font-medium">{basename(g.root)}</div>
								<div class="truncate font-mono text-xs text-muted-foreground">
									{parentDir(g.root)}
								</div>
							</div>
							<div class="shrink-0 text-right text-xs text-muted-foreground">
								<div>{g.latest_kept} kept</div>
								<div class="text-muted-foreground/70">
									{relativeTime(g.latest_started_at)}
								</div>
							</div>
							<button
								type="button"
								class="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground/50 opacity-0 transition-opacity group-hover:opacity-100 hover:text-destructive"
								aria-label="Remove from recents"
								title="Remove all scans for this project"
								onclick={(e) => removeGroup(e, g)}
							>
								<svg
									xmlns="http://www.w3.org/2000/svg"
									width="12"
									height="12"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="2"
									stroke-linecap="round"
									stroke-linejoin="round"
								>
									<path d="M18 6 6 18" />
									<path d="m6 6 12 12" />
								</svg>
							</button>
						</div>
					{/each}
				</div>
			{/if}
		</section>
	</main>
</div>
