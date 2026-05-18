<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { hasAnthropicKey, setAnthropicKey, openUrl } from '$lib/ipc';

	interface Props {
		variant?: 'card' | 'strip';
		onSaved?: () => void;
	}
	let { variant = 'card', onSaved }: Props = $props();

	let keyInput = $state('');
	let saving = $state(false);
	let errorMsg = $state<string | null>(null);
	let savedFlash = $state(false);
	let inputEl = $state<HTMLInputElement | null>(null);

	function validate(raw: string): string | null {
		const trimmed = raw.trim();
		if (!trimmed) return 'Paste your API key first.';
		if (!trimmed.startsWith('sk-ant-')) return "API keys start with 'sk-ant-'.";
		if (trimmed.length < 20) return 'That key looks too short.';
		return null;
	}

	async function save(e?: Event) {
		e?.preventDefault();
		const trimmed = keyInput.trim();
		const v = validate(trimmed);
		if (v) {
			errorMsg = v;
			return;
		}
		saving = true;
		errorMsg = null;
		try {
			await setAnthropicKey(trimmed);
			const ok = await hasAnthropicKey();
			if (!ok) {
				errorMsg = 'Saved, but the OS keychain did not return the key on read-back.';
				return;
			}
			keyInput = '';
			savedFlash = true;
			setTimeout(() => (savedFlash = false), 1600);
			onSaved?.();
		} catch (err) {
			const msg = err instanceof Error ? err.message : String(err);
			errorMsg = `Could not save: ${msg}`;
			console.error('setAnthropicKey failed', err);
		} finally {
			saving = false;
		}
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') void save(e);
	}

	function openConsole() {
		void openUrl('https://console.anthropic.com/settings/keys').catch((err) =>
			console.warn('openUrl failed', err)
		);
	}
</script>

{#if variant === 'card'}
	<div
		class="border-amber-300/50 bg-amber-50/50 dark:border-amber-500/30 dark:bg-amber-950/20 space-y-3 rounded-md border p-4"
	>
		<div class="space-y-1">
			<p class="text-sm font-medium">Connect your Anthropic API key to get started</p>
			<p class="text-muted-foreground text-xs leading-relaxed">
				Open Security uses Claude to scan your code. Your key is stored locally in the macOS keychain
				and only ever sent to Anthropic.
				<button
					type="button"
					class="text-primary underline-offset-2 hover:underline"
					onclick={openConsole}
				>
					Get a key from console.anthropic.com →
				</button>
			</p>
		</div>
		<form class="flex gap-2" onsubmit={save}>
			<Input
				bind:ref={inputEl}
				type="password"
				bind:value={keyInput}
				onkeydown={onKeydown}
				placeholder="sk-ant-…"
				autocomplete="off"
				spellcheck="false"
				aria-label="Anthropic API key"
				aria-invalid={errorMsg ? 'true' : undefined}
				class="h-8 font-mono text-xs"
			/>
			<Button type="submit" size="sm" disabled={saving || !keyInput.trim()}>
				{#if saving}
					Saving…
				{:else if savedFlash}
					Saved ✓
				{:else}
					Save key
				{/if}
			</Button>
		</form>
		{#if errorMsg}
			<p class="text-destructive text-xs" role="alert">{errorMsg}</p>
		{/if}
	</div>
{:else}
	<div class="border-border bg-amber-50/40 dark:bg-amber-950/20 border-b px-4 py-3">
		<form class="flex flex-wrap items-center gap-2" onsubmit={save}>
			<span class="text-sm font-medium">Anthropic API key required</span>
			<Input
				type="password"
				bind:value={keyInput}
				onkeydown={onKeydown}
				placeholder="sk-ant-…"
				autocomplete="off"
				spellcheck="false"
				aria-label="Anthropic API key"
				class="h-8 max-w-md font-mono text-xs"
			/>
			<Button type="submit" size="sm" disabled={saving || !keyInput.trim()}>
				{#if saving}
					Saving…
				{:else if savedFlash}
					Saved ✓
				{:else}
					Save to keychain
				{/if}
			</Button>
			<button
				type="button"
				class="text-muted-foreground hover:text-foreground text-xs underline-offset-2 hover:underline"
				onclick={openConsole}
			>
				Get a key →
			</button>
			{#if errorMsg}
				<span class="text-destructive basis-full text-xs" role="alert">{errorMsg}</span>
			{/if}
		</form>
	</div>
{/if}
