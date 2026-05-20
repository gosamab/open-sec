<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import {
		CONCURRENCY_BOUNDS,
		DEFAULT_SETTINGS,
		MODEL_OPTIONS,
		settings,
		type ScanSettings
	} from '$lib/settings.svelte';
	import {
		hasAnthropicKey,
		hasOpenAiKey,
		setAnthropicKey,
		setOpenAiKey
	} from '$lib/ipc';

	interface Props {
		onClose: () => void;
	}
	let { onClose }: Props = $props();

	// Working copy — only commit on Save.
	let draft = $state<ScanSettings>({ ...settings.value });

	// Known model presets grouped by provider. The "Custom" row reveals a
	// free-text input so users can paste any model ID (e.g. a future Sonnet
	// version) without us needing to ship a UI update.
	const CUSTOM = '__custom__';
	const ALL_PRESET_IDS = [
		...MODEL_OPTIONS.Anthropic.map((m) => m.id),
		...MODEL_OPTIONS.OpenAI.map((m) => m.id)
	];

	function presetOrCustom(value: string): string {
		return ALL_PRESET_IDS.includes(value) ? value : CUSTOM;
	}

	// --- API keys ---
	let anthropicKeyStatus = $state<boolean | null>(null);
	let openaiKeyStatus = $state<boolean | null>(null);
	let anthropicKeyInput = $state('');
	let openaiKeyInput = $state('');
	let anthropicSaving = $state(false);
	let openaiSaving = $state(false);
	let anthropicError = $state<string | null>(null);
	let openaiError = $state<string | null>(null);

	$effect(() => {
		void (async () => {
			anthropicKeyStatus = await hasAnthropicKey().catch(() => false);
			openaiKeyStatus = await hasOpenAiKey().catch(() => false);
		})();
	});

	async function saveAnthropicKey() {
		const trimmed = anthropicKeyInput.trim();
		if (!trimmed) {
			anthropicError = 'Paste your API key first.';
			return;
		}
		if (!trimmed.startsWith('sk-ant-')) {
			anthropicError = "Anthropic keys start with 'sk-ant-'.";
			return;
		}
		anthropicSaving = true;
		anthropicError = null;
		try {
			await setAnthropicKey(trimmed);
			anthropicKeyInput = '';
			anthropicKeyStatus = true;
		} catch (e) {
			anthropicError = e instanceof Error ? e.message : String(e);
		} finally {
			anthropicSaving = false;
		}
	}

	async function saveOpenAiKey() {
		const trimmed = openaiKeyInput.trim();
		if (!trimmed) {
			openaiError = 'Paste your API key first.';
			return;
		}
		if (!trimmed.startsWith('sk-')) {
			openaiError = "OpenAI keys start with 'sk-'.";
			return;
		}
		openaiSaving = true;
		openaiError = null;
		try {
			await setOpenAiKey(trimmed);
			openaiKeyInput = '';
			openaiKeyStatus = true;
		} catch (e) {
			openaiError = e instanceof Error ? e.message : String(e);
		} finally {
			openaiSaving = false;
		}
	}

	function save() {
		settings.update(draft);
		onClose();
	}

	function reset() {
		draft = { ...DEFAULT_SETTINGS };
	}

	// Container we use both as the focus-trap root and for "outside click" hit-testing.
	let dialog = $state<HTMLDivElement | null>(null);
	// Element that had focus before we opened, so we can restore on close.
	const opener =
		typeof document !== 'undefined' ? (document.activeElement as HTMLElement | null) : null;

	$effect(() => {
		if (!dialog) return;
		// Focus the first interactive element on mount so keyboard users land
		// inside the dialog without a stray Tab press.
		const first = dialog.querySelector<HTMLElement>(
			'input, button, [tabindex]:not([tabindex="-1"])'
		);
		first?.focus();
		return () => {
			// Return focus to whoever opened us. If they're gone (rare), the
			// browser falls back to <body>.
			opener?.focus?.();
		};
	});

	function onWindowKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			onClose();
			return;
		}
		if (e.key !== 'Tab' || !dialog) return;
		// Roll our own focus trap: collect the focusable descendants and wrap
		// at both ends. Keeps the user from tabbing out into the workspace
		// behind the modal.
		const focusables = Array.from(
			dialog.querySelectorAll<HTMLElement>(
				'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'
			)
		).filter((el) => !el.hasAttribute('aria-hidden') && el.offsetParent !== null);
		if (focusables.length === 0) return;
		const first = focusables[0];
		const last = focusables[focusables.length - 1];
		const active = document.activeElement as HTMLElement | null;
		if (e.shiftKey && active === first) {
			e.preventDefault();
			last.focus();
		} else if (!e.shiftKey && active === last) {
			e.preventDefault();
			first.focus();
		}
	}
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="fixed inset-0 z-50 flex items-center justify-center p-6">
	<!-- Backdrop: a real button so click-to-close is keyboard-accessible too. -->
	<button
		type="button"
		class="absolute inset-0 bg-background/80 backdrop-blur-sm"
		aria-label="Close settings"
		tabindex="-1"
		onclick={onClose}
	></button>
	<div
		bind:this={dialog}
		class="relative flex max-h-[85vh] w-full max-w-2xl flex-col rounded-lg border border-border bg-background shadow-xl"
		role="dialog"
		aria-modal="true"
		aria-label="Settings"
	>
		<header class="space-y-1 border-b border-border px-6 py-4">
			<h2 class="text-lg font-semibold tracking-tight">Settings</h2>
			<p class="text-xs text-muted-foreground">
				Per-stage model and concurrency overrides. Saved locally; applied on the next scan.
			</p>
		</header>

		<div class="flex-1 space-y-6 overflow-y-auto px-6 py-5">
			<!-- API keys -->
			<section class="space-y-3">
				<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
					API Keys
				</h3>
				<div class="grid grid-cols-[120px_1fr] items-start gap-x-3 gap-y-3 text-sm">
					{@render keyRow(
						'Anthropic',
						'k-anthropic',
						'sk-ant-…',
						anthropicKeyStatus,
						() => anthropicKeyInput,
						(v) => (anthropicKeyInput = v),
						() => anthropicSaving,
						() => anthropicError,
						saveAnthropicKey
					)}
					{@render keyRow(
						'OpenAI',
						'k-openai',
						'sk-…',
						openaiKeyStatus,
						() => openaiKeyInput,
						(v) => (openaiKeyInput = v),
						() => openaiSaving,
						() => openaiError,
						saveOpenAiKey
					)}
				</div>
			</section>

			{#snippet keyRow(
				label: string,
				id: string,
				placeholder: string,
				status: boolean | null,
				getValue: () => string,
				setValue: (v: string) => void,
				getSaving: () => boolean,
				getError: () => string | null,
				onSave: () => void
			)}
				<div class="flex flex-wrap items-center gap-2 pt-2">
					<span class="text-muted-foreground">{label}</span>
					{#if status === true}
						<span
							class="rounded-full bg-emerald-500/10 px-1.5 py-0.5 font-mono text-[10px] text-emerald-500"
						>
							set
						</span>
					{:else if status === false}
						<span
							class="rounded-full bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground"
						>
							not set
						</span>
					{/if}
				</div>
				<div class="space-y-1">
					<div class="flex items-center gap-2">
						<Input
							{id}
							type="password"
							value={getValue()}
							oninput={(e) => setValue((e.currentTarget as HTMLInputElement).value)}
							{placeholder}
							autocomplete="off"
							spellcheck="false"
							class="h-8 font-mono text-xs"
						/>
						<Button size="sm" onclick={onSave} disabled={getSaving() || !getValue().trim()}>
							{getSaving() ? 'Saving…' : 'Save'}
						</Button>
					</div>
					{#if getError()}
						<p class="text-xs text-destructive" role="alert">{getError()}</p>
					{/if}
				</div>
			{/snippet}

		<!-- Models -->
		<section class="space-y-3">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Models
			</h3>
			<div class="grid grid-cols-[120px_1fr] items-start gap-x-3 gap-y-2 text-sm">
				{@render modelRow('Triage', 'm-triage', () => draft.triage_model, (v) => (draft.triage_model = v))}
				{@render modelRow('Detect', 'm-detect', () => draft.detect_model, (v) => (draft.detect_model = v))}
				{@render modelRow('Verify', 'm-verify', () => draft.verify_model, (v) => (draft.verify_model = v))}
				{@render modelRow('Patch', 'm-patch', () => draft.patch_model, (v) => (draft.patch_model = v))}
			</div>
		</section>

		{#snippet modelRow(label: string, id: string, get: () => string, set: (v: string) => void)}
			{@const value = get()}
			{@const mode = presetOrCustom(value)}
			<label for={id} class="pt-2 text-muted-foreground">{label}</label>
			<div class="space-y-1">
				<select
					{id}
					value={mode}
					onchange={(e) => {
						const next = (e.currentTarget as HTMLSelectElement).value;
						if (next !== CUSTOM) set(next);
					}}
					class="h-8 w-full rounded-md border border-input bg-background px-2 text-xs"
				>
					{#each Object.entries(MODEL_OPTIONS) as [provider, presets] (provider)}
						<optgroup label={provider}>
							{#each presets as preset (preset.id)}
								<option value={preset.id}>{preset.label}</option>
							{/each}
						</optgroup>
					{/each}
					<option value={CUSTOM}>Custom…</option>
				</select>
				{#if mode === CUSTOM}
					<Input
						value={value}
						oninput={(e) => set((e.currentTarget as HTMLInputElement).value)}
						placeholder="model id, e.g. claude-sonnet-4-6 or gpt-5-mini"
						class="h-8 font-mono text-xs"
					/>
				{/if}
			</div>
		{/snippet}

		<!-- Concurrency -->
		<section class="space-y-3">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Concurrency
			</h3>
			<div class="grid grid-cols-4 gap-3 text-sm">
				<div class="space-y-1">
					<label for="c-triage" class="text-xs text-muted-foreground">Triage</label>
					<Input
						id="c-triage"
						type="number"
						min={CONCURRENCY_BOUNDS.triage_concurrency.min}
						max={CONCURRENCY_BOUNDS.triage_concurrency.max}
						bind:value={draft.triage_concurrency}
						class="h-8 text-xs"
					/>
				</div>
				<div class="space-y-1">
					<label for="c-detect" class="text-xs text-muted-foreground">Detect</label>
					<Input
						id="c-detect"
						type="number"
						min={CONCURRENCY_BOUNDS.detect_concurrency.min}
						max={CONCURRENCY_BOUNDS.detect_concurrency.max}
						bind:value={draft.detect_concurrency}
						class="h-8 text-xs"
					/>
				</div>
				<div class="space-y-1">
					<label for="c-verify" class="text-xs text-muted-foreground">Verify</label>
					<Input
						id="c-verify"
						type="number"
						min={CONCURRENCY_BOUNDS.verify_concurrency.min}
						max={CONCURRENCY_BOUNDS.verify_concurrency.max}
						bind:value={draft.verify_concurrency}
						class="h-8 text-xs"
					/>
				</div>
				<div class="space-y-1">
					<label for="c-patch" class="text-xs text-muted-foreground">Patch</label>
					<Input
						id="c-patch"
						type="number"
						min={CONCURRENCY_BOUNDS.patch_concurrency.min}
						max={CONCURRENCY_BOUNDS.patch_concurrency.max}
						bind:value={draft.patch_concurrency}
						class="h-8 text-xs"
					/>
				</div>
			</div>
		</section>

		<!-- Budget -->
		<section class="space-y-3">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Budget cap
			</h3>
			<div class="grid grid-cols-[160px_1fr] items-center gap-3 text-sm">
				<label for="b-tokens" class="text-muted-foreground">Max tokens (in+out)</label>
				<div class="flex items-center gap-2">
					<Input
						id="b-tokens"
						type="number"
						min="0"
						step="1000"
						bind:value={draft.budget_total_tokens}
						class="h-8 text-xs"
					/>
					<span class="text-xs text-muted-foreground">0 = unlimited</span>
				</div>
			</div>
			<p class="text-xs leading-relaxed text-muted-foreground">
				When the running total of input + output tokens crosses this cap, the scan is cancelled at
				the next stage boundary. Partial findings are preserved and the scan is saved with status <span
					class="font-mono">cancelled</span
				>.
			</p>
		</section>

		<!-- Currency -->
		<section class="space-y-3">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Currency
			</h3>
			<div class="grid grid-cols-[160px_1fr] items-center gap-3 text-sm">
				<label for="c-sar" class="text-muted-foreground">SAR per USD</label>
				<div class="flex items-center gap-2">
					<Input
						id="c-sar"
						type="number"
						min="0"
						step="0.01"
						bind:value={draft.sar_per_usd}
						class="h-8 text-xs"
					/>
					<span class="text-xs text-muted-foreground">default 3.75 (SAMA peg)</span>
				</div>
			</div>
		</section>
		</div>

		<footer class="flex items-center justify-between border-t border-border px-6 py-4">
			<Button variant="outline" size="sm" onclick={reset}>Reset to defaults</Button>
			<div class="flex gap-2">
				<Button variant="outline" size="sm" onclick={onClose}>Cancel</Button>
				<Button size="sm" onclick={save}>Save</Button>
			</div>
		</footer>
	</div>
</div>
