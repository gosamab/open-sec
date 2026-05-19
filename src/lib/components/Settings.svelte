<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import {
		CONCURRENCY_BOUNDS,
		DEFAULT_SETTINGS,
		settings,
		type ScanSettings
	} from '$lib/settings.svelte';

	interface Props {
		onClose: () => void;
	}
	let { onClose }: Props = $props();

	// Working copy — only commit on Save.
	let draft = $state<ScanSettings>({ ...settings.value });

	// Known Claude models offered as presets. The "Custom" row reveals a
	// free-text input so users can paste any model ID (e.g. a future Sonnet
	// version) without us needing to ship a UI update.
	const MODEL_PRESETS = [
		{ id: 'claude-opus-4-7', label: 'Opus 4.7 — most thorough, slowest' },
		{ id: 'claude-sonnet-4-6', label: 'Sonnet 4.6 — balanced' },
		{ id: 'claude-haiku-4-5', label: 'Haiku 4.5 — fastest, cheapest' }
	] as const;
	const CUSTOM = '__custom__';
	const PRESET_IDS = MODEL_PRESETS.map((m) => m.id);

	function presetOrCustom(value: string): string {
		return PRESET_IDS.includes(value as (typeof PRESET_IDS)[number]) ? value : CUSTOM;
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
		class="relative w-full max-w-2xl space-y-6 rounded-lg border border-border bg-background p-6 shadow-xl"
		role="dialog"
		aria-modal="true"
		aria-label="Settings"
	>
		<header class="space-y-1">
			<h2 class="text-lg font-semibold tracking-tight">Settings</h2>
			<p class="text-xs text-muted-foreground">
				Per-stage model and concurrency overrides. Saved locally; applied on the next scan.
			</p>
		</header>

		<!-- Models -->
		<section class="space-y-3">
			<h3 class="text-[0.625rem] font-medium tracking-wider text-muted-foreground uppercase">
				Models
			</h3>
			<div class="grid grid-cols-[100px_1fr] items-start gap-x-3 gap-y-2 text-sm">
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
					{#each MODEL_PRESETS as preset (preset.id)}
						<option value={preset.id}>{preset.label}</option>
					{/each}
					<option value={CUSTOM}>Custom…</option>
				</select>
				{#if mode === CUSTOM}
					<Input
						value={value}
						oninput={(e) => set((e.currentTarget as HTMLInputElement).value)}
						placeholder="model id, e.g. claude-sonnet-4-6"
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

		<footer class="flex items-center justify-between border-t border-border pt-4">
			<Button variant="outline" size="sm" onclick={reset}>Reset to defaults</Button>
			<div class="flex gap-2">
				<Button variant="outline" size="sm" onclick={onClose}>Cancel</Button>
				<Button size="sm" onclick={save}>Save</Button>
			</div>
		</footer>
	</div>
</div>
