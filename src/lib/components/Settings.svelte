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
			<div class="grid grid-cols-[100px_1fr] items-center gap-3 text-sm">
				<label for="m-triage" class="text-muted-foreground">Triage</label>
				<Input id="m-triage" bind:value={draft.triage_model} class="h-8 font-mono text-xs" />
				<label for="m-detect" class="text-muted-foreground">Detect</label>
				<Input id="m-detect" bind:value={draft.detect_model} class="h-8 font-mono text-xs" />
				<label for="m-verify" class="text-muted-foreground">Verify</label>
				<Input id="m-verify" bind:value={draft.verify_model} class="h-8 font-mono text-xs" />
				<label for="m-patch" class="text-muted-foreground">Patch</label>
				<Input id="m-patch" bind:value={draft.patch_model} class="h-8 font-mono text-xs" />
			</div>
		</section>

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

		<footer class="flex items-center justify-between border-t border-border pt-4">
			<Button variant="outline" size="sm" onclick={reset}>Reset to defaults</Button>
			<div class="flex gap-2">
				<Button variant="outline" size="sm" onclick={onClose}>Cancel</Button>
				<Button size="sm" onclick={save}>Save</Button>
			</div>
		</footer>
	</div>
</div>
