<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { greet } from '$lib/ipc';

	let name = $state('open-sec');
	let response = $state<string | null>(null);
	let error = $state<string | null>(null);
	let pending = $state(false);

	async function sayHi() {
		pending = true;
		error = null;
		try {
			response = await greet(name);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			pending = false;
		}
	}
</script>

<main class="flex min-h-screen flex-col items-center justify-center gap-6 p-8">
	<div class="space-y-2 text-center">
		<h1 class="text-3xl font-semibold tracking-tight">open-sec</h1>
		<p class="text-muted-foreground text-sm">Local-first security code scanner</p>
	</div>

	<div class="flex w-full max-w-sm gap-2">
		<input
			class="border-input bg-background focus-visible:ring-ring flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-sm transition-colors focus-visible:ring-1 focus-visible:outline-none"
			bind:value={name}
			placeholder="your name"
		/>
		<Button onclick={sayHi} disabled={pending}>
			{pending ? '…' : 'Say hi'}
		</Button>
	</div>

	{#if response}
		<p class="text-foreground rounded-md border bg-card px-4 py-2 text-sm">{response}</p>
	{/if}
	{#if error}
		<p class="text-destructive text-sm">{error}</p>
	{/if}
</main>
