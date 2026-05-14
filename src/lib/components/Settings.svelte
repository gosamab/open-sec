<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { DEFAULT_SETTINGS, settings, type ScanSettings } from '$lib/settings.svelte';

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

	function backdropKey(e: KeyboardEvent) {
		if (e.key === 'Escape') onClose();
	}
</script>

<svelte:window onkeydown={backdropKey} />

<div
	class="bg-background/80 fixed inset-0 z-50 flex items-center justify-center p-6 backdrop-blur-sm"
	role="dialog"
	aria-modal="true"
	aria-label="Settings"
	onclick={(e) => {
		if (e.target === e.currentTarget) onClose();
	}}
	onkeydown={() => {}}
	tabindex="-1"
>
	<div class="bg-background border-border w-full max-w-2xl space-y-6 rounded-lg border p-6 shadow-xl">
		<header class="space-y-1">
			<h2 class="text-lg font-semibold tracking-tight">Settings</h2>
			<p class="text-muted-foreground text-xs">
				Per-stage model and concurrency overrides. Saved locally; applied on the next scan.
			</p>
		</header>

		<!-- Models -->
		<section class="space-y-3">
			<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
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
			<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
				Concurrency
			</h3>
			<div class="grid grid-cols-4 gap-3 text-sm">
				<div class="space-y-1">
					<label for="c-triage" class="text-muted-foreground text-xs">Triage</label>
					<Input
						id="c-triage"
						type="number"
						min="1"
						max="32"
						bind:value={draft.triage_concurrency}
						class="h-8 text-xs"
					/>
				</div>
				<div class="space-y-1">
					<label for="c-detect" class="text-muted-foreground text-xs">Detect</label>
					<Input
						id="c-detect"
						type="number"
						min="1"
						max="16"
						bind:value={draft.detect_concurrency}
						class="h-8 text-xs"
					/>
				</div>
				<div class="space-y-1">
					<label for="c-verify" class="text-muted-foreground text-xs">Verify</label>
					<Input
						id="c-verify"
						type="number"
						min="1"
						max="8"
						bind:value={draft.verify_concurrency}
						class="h-8 text-xs"
					/>
				</div>
				<div class="space-y-1">
					<label for="c-patch" class="text-muted-foreground text-xs">Patch</label>
					<Input
						id="c-patch"
						type="number"
						min="1"
						max="8"
						bind:value={draft.patch_concurrency}
						class="h-8 text-xs"
					/>
				</div>
			</div>
		</section>

		<!-- Budget -->
		<section class="space-y-3">
			<h3 class="text-muted-foreground text-[0.625rem] font-medium uppercase tracking-wider">
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
					<span class="text-muted-foreground text-xs">0 = unlimited</span>
				</div>
			</div>
			<p class="text-muted-foreground text-xs leading-relaxed">
				When the running total of input + output tokens crosses this cap, the scan is
				cancelled at the next stage boundary. Partial findings are preserved and the
				scan is saved with status <span class="font-mono">cancelled</span>.
			</p>
		</section>

		<footer class="border-border flex items-center justify-between border-t pt-4">
			<Button variant="outline" size="sm" onclick={reset}>Reset to defaults</Button>
			<div class="flex gap-2">
				<Button variant="outline" size="sm" onclick={onClose}>Cancel</Button>
				<Button size="sm" onclick={save}>Save</Button>
			</div>
		</footer>
	</div>
</div>
