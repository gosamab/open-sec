<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import {
		hasAnthropicKey,
		hasOpenAiKey,
		openUrl,
		setAnthropicKey,
		setOpenAiKey
	} from '$lib/ipc';

	interface Props {
		variant?: 'card' | 'strip';
		onSaved?: () => void;
	}
	let { variant = 'card', onSaved }: Props = $props();

	type Provider = {
		id: 'anthropic' | 'openai';
		label: string;
		placeholder: string;
		consoleUrl: string;
		validate(raw: string): string | null;
		save(key: string): Promise<void>;
		check(): Promise<boolean>;
	};

	const PROVIDERS: Provider[] = [
		{
			id: 'anthropic',
			label: 'Anthropic',
			placeholder: 'sk-ant-…',
			consoleUrl: 'https://console.anthropic.com/settings/keys',
			validate: (raw) => {
				const t = raw.trim();
				if (!t) return 'Paste your API key first.';
				if (!t.startsWith('sk-ant-')) return "Anthropic keys start with 'sk-ant-'.";
				if (t.length < 20) return 'That key looks too short.';
				return null;
			},
			save: setAnthropicKey,
			check: hasAnthropicKey
		},
		{
			id: 'openai',
			label: 'OpenAI',
			placeholder: 'sk-…',
			consoleUrl: 'https://platform.openai.com/api-keys',
			validate: (raw) => {
				const t = raw.trim();
				if (!t) return 'Paste your API key first.';
				if (!t.startsWith('sk-')) return "OpenAI keys start with 'sk-'.";
				if (t.length < 20) return 'That key looks too short.';
				return null;
			},
			save: setOpenAiKey,
			check: hasOpenAiKey
		}
	];

	// Per-provider state. Keep separate so saves don't trample each other.
	let inputs = $state<Record<string, string>>({ anthropic: '', openai: '' });
	let saving = $state<Record<string, boolean>>({ anthropic: false, openai: false });
	let errors = $state<Record<string, string | null>>({ anthropic: null, openai: null });
	let savedFlash = $state<Record<string, boolean>>({ anthropic: false, openai: false });
	let configured = $state<Record<string, boolean>>({ anthropic: false, openai: false });

	$effect(() => {
		void (async () => {
			for (const p of PROVIDERS) {
				configured[p.id] = await p.check().catch(() => false);
			}
		})();
	});

	async function saveProvider(p: Provider, e?: Event) {
		e?.preventDefault();
		const trimmed = inputs[p.id].trim();
		const v = p.validate(trimmed);
		if (v) {
			errors[p.id] = v;
			return;
		}
		saving[p.id] = true;
		errors[p.id] = null;
		try {
			await p.save(trimmed);
			inputs[p.id] = '';
			configured[p.id] = true;
			savedFlash[p.id] = true;
			setTimeout(() => (savedFlash[p.id] = false), 1600);
			onSaved?.();
		} catch (err) {
			const msg = err instanceof Error ? err.message : String(err);
			errors[p.id] = msg || 'Could not save the key. Try again.';
			console.error(`set${p.label}Key failed`, err);
		} finally {
			saving[p.id] = false;
		}
	}

	function openConsole(url: string) {
		void openUrl(url).catch((err) => console.warn('openUrl failed', err));
	}
</script>

{#if variant === 'card'}
	<div class="space-y-4 rounded-md border border-border bg-muted/30 p-4">
		<div class="space-y-1">
			<p class="text-sm font-medium leading-tight">Connect an API key to get started</p>
			<p class="text-xs leading-relaxed text-muted-foreground">
				Open Security needs at least one provider key. Configure either or both — the per-stage
				model setting routes calls to the matching provider. Keys are saved locally in the app's
				data folder.
			</p>
		</div>

		{#each PROVIDERS as p (p.id)}
			<form class="space-y-1.5" onsubmit={(e) => saveProvider(p, e)}>
				<div class="flex items-baseline justify-between">
					<label for="key-{p.id}" class="text-xs font-medium">
						{p.label}
						{#if configured[p.id]}
							<span class="ml-1 text-[10px] text-muted-foreground">— set</span>
						{/if}
					</label>
					<button
						type="button"
						class="text-[10px] text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
						onclick={() => openConsole(p.consoleUrl)}
					>
						Get a key →
					</button>
				</div>
				<div class="flex gap-2">
					<Input
						id="key-{p.id}"
						type="password"
						bind:value={inputs[p.id]}
						placeholder={configured[p.id] ? 'Replace existing key…' : p.placeholder}
						autocomplete="off"
						spellcheck="false"
						aria-label="{p.label} API key"
						class="h-8 font-mono text-xs"
					/>
					<Button type="submit" disabled={saving[p.id] || !inputs[p.id].trim()}>
						{#if saving[p.id]}
							Saving…
						{:else if savedFlash[p.id]}
							Saved ✓
						{:else}
							Save
						{/if}
					</Button>
				</div>
				{#if errors[p.id]}
					<p class="text-xs leading-relaxed text-destructive" role="alert">{errors[p.id]}</p>
				{/if}
			</form>
		{/each}
	</div>
{:else}
	<div class="border-b border-border bg-muted/40 px-4 py-2.5">
		<div class="flex flex-wrap items-center gap-3">
			<span class="text-sm font-medium">API key required</span>
			{#each PROVIDERS as p (p.id)}
				<form
					class="flex flex-wrap items-center gap-1.5"
					onsubmit={(e) => saveProvider(p, e)}
				>
					<span class="text-xs text-muted-foreground">{p.label}:</span>
					<Input
						type="password"
						bind:value={inputs[p.id]}
						placeholder={configured[p.id] ? 'set' : p.placeholder}
						autocomplete="off"
						spellcheck="false"
						aria-label="{p.label} API key"
						class="h-8 w-44 font-mono text-xs"
					/>
					<Button type="submit" size="sm" disabled={saving[p.id] || !inputs[p.id].trim()}>
						{#if saving[p.id]}
							…
						{:else if savedFlash[p.id]}
							✓
						{:else}
							Save
						{/if}
					</Button>
					{#if errors[p.id]}
						<span class="basis-full text-xs text-destructive" role="alert">{errors[p.id]}</span>
					{/if}
				</form>
			{/each}
		</div>
	</div>
{/if}
