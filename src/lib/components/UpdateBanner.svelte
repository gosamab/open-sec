<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button';

	type UpdateState =
		| { kind: 'idle' }
		| { kind: 'available'; version: string; notes: string | null; update: unknown }
		| { kind: 'downloading'; progress: number }
		| { kind: 'ready' }
		| { kind: 'error'; message: string }
		| { kind: 'dismissed' };

	let state = $state<UpdateState>({ kind: 'idle' });

	onMount(async () => {
		try {
			const { check } = await import('@tauri-apps/plugin-updater');
			const upd = await check();
			if (upd) {
				state = {
					kind: 'available',
					version: upd.version,
					notes: upd.body ?? null,
					update: upd
				};
			}
		} catch (e) {
			// Network down, no manifest published yet, or running in dev — silent.
			console.warn('updater check failed', e);
		}
	});

	async function install() {
		if (state.kind !== 'available') return;
		const update = state.update as {
			downloadAndInstall: (
				cb?: (event: { event: string; data?: { chunkLength?: number; contentLength?: number } }) => void
			) => Promise<void>;
		};
		try {
			let downloaded = 0;
			let total = 0;
			state = { kind: 'downloading', progress: 0 };
			await update.downloadAndInstall((event) => {
				if (event.event === 'Started' && event.data?.contentLength) {
					total = event.data.contentLength;
				} else if (event.event === 'Progress' && event.data?.chunkLength) {
					downloaded += event.data.chunkLength;
					if (total > 0) {
						state = { kind: 'downloading', progress: Math.min(100, (downloaded / total) * 100) };
					}
				} else if (event.event === 'Finished') {
					state = { kind: 'ready' };
				}
			});
			state = { kind: 'ready' };
		} catch (e) {
			state = { kind: 'error', message: e instanceof Error ? e.message : String(e) };
		}
	}

	async function restart() {
		const { relaunch } = await import('@tauri-apps/plugin-process');
		await relaunch();
	}

	function dismiss() {
		state = { kind: 'dismissed' };
	}
</script>

{#if state.kind === 'available'}
	<div
		class="border-border bg-popover text-popover-foreground fixed bottom-4 right-4 z-50 w-80 space-y-2 rounded-md border p-3 shadow-lg"
	>
		<div class="flex items-start justify-between gap-2">
			<div class="space-y-0.5">
				<div class="text-sm font-medium">Update available</div>
				<div class="text-muted-foreground font-mono text-xs">v{state.version}</div>
			</div>
			<button
				type="button"
				onclick={dismiss}
				class="text-muted-foreground hover:text-foreground inline-flex h-5 w-5 items-center justify-center rounded"
				aria-label="Dismiss"
				title="Dismiss"
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
		{#if state.notes}
			<p class="text-muted-foreground line-clamp-4 text-xs leading-relaxed">{state.notes}</p>
		{/if}
		<div class="flex gap-2 pt-1">
			<Button size="sm" onclick={install}>Install &amp; restart</Button>
			<Button size="sm" variant="outline" onclick={dismiss}>Later</Button>
		</div>
	</div>
{:else if state.kind === 'downloading'}
	<div
		class="border-border bg-popover text-popover-foreground fixed bottom-4 right-4 z-50 w-80 space-y-2 rounded-md border p-3 shadow-lg"
	>
		<div class="text-sm font-medium">Downloading update…</div>
		<div class="bg-muted h-1.5 w-full overflow-hidden rounded-full">
			<div
				class="bg-foreground h-full transition-all"
				style="width: {state.progress.toFixed(0)}%"
			></div>
		</div>
		<div class="text-muted-foreground font-mono text-xs">{state.progress.toFixed(0)}%</div>
	</div>
{:else if state.kind === 'ready'}
	<div
		class="border-border bg-popover text-popover-foreground fixed bottom-4 right-4 z-50 w-80 space-y-2 rounded-md border p-3 shadow-lg"
	>
		<div class="text-sm font-medium">Update ready</div>
		<p class="text-muted-foreground text-xs">Restart now to apply.</p>
		<div class="flex gap-2 pt-1">
			<Button size="sm" onclick={restart}>Restart</Button>
			<Button size="sm" variant="outline" onclick={dismiss}>Later</Button>
		</div>
	</div>
{:else if state.kind === 'error'}
	<div
		class="border-destructive/40 bg-destructive/5 fixed bottom-4 right-4 z-50 w-80 space-y-1 rounded-md border p-3 shadow-lg"
	>
		<div class="text-destructive text-sm font-medium">Update failed</div>
		<p class="text-muted-foreground line-clamp-3 text-xs">{state.message}</p>
		<div class="pt-1">
			<Button size="sm" variant="outline" onclick={dismiss}>Dismiss</Button>
		</div>
	</div>
{/if}
