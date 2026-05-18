<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';
	import { Button } from '$lib/components/ui/button';
	import ThemeToggle from '$lib/components/ThemeToggle.svelte';
	import {
		deleteScansForRoot,
		hasAnthropicKey,
		listScanGroups,
		setAnthropicKey,
		type ScanGroup
	} from '$lib/ipc';
	import { Input } from '$lib/components/ui/input';
	import { onMount } from 'svelte';

	interface Props {
		onOpenFresh: (path: string) => void;
		onOpenPast: (group: ScanGroup) => void;
	}
	let { onOpenFresh, onOpenPast }: Props = $props();

	let keyConfigured = $state(false);
	let keyInput = $state('');
	let savingKey = $state(false);
	let keyError = $state<string | null>(null);

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
		// Sequence individual deletes — simpler than adding a new IPC for this.
		for (const g of groups) {
			try {
				await deleteScansForRoot(g.root);
			} catch (e) {
				console.error('deleteScansForRoot failed', g.root, e);
			}
		}
		await reloadGroups();
	}

	async function saveKey() {
		if (!keyInput.trim()) return;
		savingKey = true;
		keyError = null;
		try {
			await setAnthropicKey(keyInput.trim());
			keyConfigured = await hasAnthropicKey();
			keyInput = '';
		} catch (e) {
			keyError = e instanceof Error ? e.message : String(e);
		} finally {
			savingKey = false;
		}
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="bg-background flex h-screen flex-col">
	<!-- Topbar (theme toggle only) -->
	<header class="flex items-center justify-end px-4 py-3">
		<ThemeToggle />
	</header>

	<main class="mx-auto flex w-full max-w-2xl flex-1 flex-col gap-8 px-6 pb-12 pt-4">
		<!-- Branding -->
		<div class="space-y-2">
			<div class="flex items-center gap-3">
				<img src="/logo.png" alt="" width="40" height="40" class="rounded-lg" />
				<h1 class="text-3xl font-semibold tracking-tight">Open Security</h1>
			</div>
			<p class="text-muted-foreground text-sm">
				AI-powered security code scanner. Pick a folder to begin.
			</p>
		</div>

		{#if !keyConfigured}
			<div
				class="border-amber-300/40 bg-amber-50/40 dark:border-amber-500/30 dark:bg-amber-950/20 space-y-2 rounded-md border p-4"
			>
				<div class="space-y-1">
					<p class="text-sm font-medium">Anthropic API key required</p>
					<p class="text-muted-foreground text-xs">
						Stored in the OS keychain. Or set <code class="font-mono">ANTHROPIC_API_KEY</code> in a
						<code class="font-mono">.env</code> file at the project root.
					</p>
				</div>
				<div class="flex gap-2">
					<Input
						type="password"
						bind:value={keyInput}
						placeholder="sk-ant-…"
						autocomplete="off"
						class="h-8 text-xs"
					/>
					<Button size="sm" onclick={saveKey} disabled={savingKey || !keyInput.trim()}>
						{savingKey ? 'Saving…' : 'Save'}
					</Button>
				</div>
				{#if keyError}
					<p class="text-destructive text-xs">{keyError}</p>
				{/if}
			</div>
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
				<span class="text-primary-foreground/60 text-xs">⌘O</span>
			</Button>
		</div>

		<!-- Recents -->
		<section class="flex flex-col gap-2">
			<div class="flex items-center justify-between">
				<h2 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
					Recent projects
				</h2>
				{#if groups.length > 0}
					<button
						type="button"
						class="text-muted-foreground hover:text-foreground text-xs"
						onclick={clearAll}
					>
						Clear all
					</button>
				{/if}
			</div>
			{#if loadingGroups}
				<p class="text-muted-foreground border-border rounded-md border border-dashed px-4 py-6 text-center text-xs">
					Loading…
				</p>
			{:else if groups.length === 0}
				<p class="text-muted-foreground border-border rounded-md border border-dashed px-4 py-6 text-center text-xs">
					No projects scanned yet.
				</p>
			{:else}
				<div class="divide-border divide-y rounded-md border">
					{#each groups as g (g.root)}
						<div
							class="hover:bg-muted/50 group flex cursor-pointer items-center gap-3 px-4 py-2.5"
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
								class="text-muted-foreground shrink-0"
							>
								<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
							</svg>
							<div class="flex-1 truncate">
								<div class="truncate text-sm font-medium">{basename(g.root)}</div>
								<div class="text-muted-foreground truncate font-mono text-xs">
									{parentDir(g.root)}
								</div>
							</div>
							<div class="text-muted-foreground shrink-0 text-right text-xs">
								<div>{g.latest_kept} kept</div>
								<div class="text-muted-foreground/70">
									{relativeTime(g.latest_started_at)}
								</div>
							</div>
							<button
								type="button"
								class="text-muted-foreground/50 hover:text-destructive inline-flex h-5 w-5 shrink-0 items-center justify-center rounded opacity-0 transition-opacity group-hover:opacity-100"
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
