<script lang="ts">
	import { open } from '@tauri-apps/plugin-dialog';
	import { onMount } from 'svelte';

	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import {
		hasAnthropicKey,
		scanFile,
		setAnthropicKey,
		type Finding,
		type Severity
	} from '$lib/ipc';

	let filePath = $state('');
	let scopeRoot = $state('');
	let scanning = $state(false);
	let findings = $state<Finding[]>([]);
	let error = $state<string | null>(null);

	let keyConfigured = $state(false);
	let keyInput = $state('');
	let savingKey = $state(false);

	onMount(async () => {
		keyConfigured = await hasAnthropicKey();
	});

	function parentDir(p: string): string {
		const idx = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
		return idx > 0 ? p.slice(0, idx) : p;
	}

	async function pickFile() {
		const picked = await open({
			multiple: false,
			directory: false,
			title: 'Choose a source file to scan'
		});
		if (typeof picked === 'string') {
			filePath = picked;
			// Default scope to the file's parent dir if the user hasn't set one.
			if (!scopeRoot) scopeRoot = parentDir(picked);
		}
	}

	async function pickScope() {
		const picked = await open({
			multiple: false,
			directory: true,
			title: 'Choose scope root for tool calls'
		});
		if (typeof picked === 'string') scopeRoot = picked;
	}

	async function runScan() {
		if (!filePath) return;
		scanning = true;
		error = null;
		findings = [];
		try {
			const root = scopeRoot || parentDir(filePath);
			findings = await scanFile(filePath, root);
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			scanning = false;
		}
	}

	async function saveKey() {
		if (!keyInput.trim()) return;
		savingKey = true;
		try {
			await setAnthropicKey(keyInput.trim());
			keyConfigured = await hasAnthropicKey();
			keyInput = '';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			savingKey = false;
		}
	}

	const SEVERITY_ORDER: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

	function severityRank(s: Severity): number {
		return SEVERITY_ORDER.indexOf(s);
	}

	function severityClass(s: Severity): string {
		switch (s) {
			case 'critical':
				return 'bg-red-600 text-white hover:bg-red-600';
			case 'high':
				return 'bg-orange-500 text-white hover:bg-orange-500';
			case 'medium':
				return 'bg-amber-400 text-amber-950 hover:bg-amber-400';
			case 'low':
				return 'bg-blue-500 text-white hover:bg-blue-500';
			case 'info':
				return 'bg-zinc-400 text-zinc-50 hover:bg-zinc-400';
		}
	}

	let sortedFindings = $derived(
		[...findings].sort((a, b) => severityRank(a.severity) - severityRank(b.severity))
	);
</script>

<main class="mx-auto flex min-h-screen max-w-4xl flex-col gap-6 p-8">
	<header class="space-y-1">
		<h1 class="text-3xl font-semibold tracking-tight">open-sec</h1>
		<p class="text-muted-foreground text-sm">
			Local-first security code scanner. Single file + tool-use agent — Step 4.
		</p>
	</header>

	{#if !keyConfigured}
		<Card.Root class="border-amber-300/40 bg-amber-50/40 dark:bg-amber-950/20">
			<Card.Header class="space-y-1">
				<Card.Title class="text-base">Anthropic API key</Card.Title>
				<Card.Description>
					Stored in the OS keychain. Or set <code>ANTHROPIC_API_KEY</code> in a
					<code>.env</code> next to <code>vite.config.ts</code>.
				</Card.Description>
			</Card.Header>
			<Card.Content class="flex gap-2">
				<Input
					type="password"
					bind:value={keyInput}
					placeholder="sk-ant-…"
					autocomplete="off"
				/>
				<Button onclick={saveKey} disabled={savingKey || !keyInput.trim()}>
					{savingKey ? 'Saving…' : 'Save'}
				</Button>
			</Card.Content>
		</Card.Root>
	{/if}

	<div class="flex flex-col gap-3">
		<div class="flex flex-col gap-1">
			<label for="path" class="text-sm font-medium">File to scan</label>
			<div class="flex gap-2">
				<Input
					id="path"
					bind:value={filePath}
					placeholder="/path/to/file.ts"
					spellcheck={false}
				/>
				<Button variant="outline" onclick={pickFile}>Pick…</Button>
			</div>
		</div>

		<div class="flex flex-col gap-1">
			<label for="scope" class="text-sm font-medium">
				Scope (tools can only read files inside this folder)
			</label>
			<div class="flex gap-2">
				<Input
					id="scope"
					bind:value={scopeRoot}
					placeholder="defaults to the file's parent directory"
					spellcheck={false}
				/>
				<Button variant="outline" onclick={pickScope}>Pick…</Button>
			</div>
		</div>

		<Button onclick={runScan} disabled={scanning || !filePath} class="self-start">
			{scanning ? 'Scanning…' : 'Scan'}
		</Button>
	</div>

	{#if error}
		<Card.Root class="border-destructive/40">
			<Card.Header>
				<Card.Title class="text-destructive text-sm">Scan failed</Card.Title>
			</Card.Header>
			<Card.Content class="text-sm whitespace-pre-wrap">{error}</Card.Content>
		</Card.Root>
	{/if}

	{#if scanning}
		<p class="text-muted-foreground text-sm">Calling Sonnet… first response can take 10–30 s.</p>
	{:else if findings.length === 0 && filePath && !error}
		<p class="text-muted-foreground text-sm">No findings yet. Hit Scan.</p>
	{/if}

	{#if sortedFindings.length > 0}
		<section class="flex flex-col gap-3">
			<p class="text-muted-foreground text-xs uppercase tracking-wide">
				{sortedFindings.length} finding{sortedFindings.length === 1 ? '' : 's'}
			</p>
			{#each sortedFindings as f (f.id)}
				<Card.Root>
					<Card.Header class="flex flex-row items-start justify-between gap-3 space-y-0">
						<div class="space-y-1">
							<Card.Title class="text-base leading-tight">{f.title}</Card.Title>
							<Card.Description class="font-mono text-xs">
								{f.file}:{f.line_start}{f.line_end !== f.line_start ? `-${f.line_end}` : ''}
								<span class="ml-2">{f.cwe}</span>
								{#if f.owasp}<span class="ml-2">· {f.owasp}</span>{/if}
							</Card.Description>
						</div>
						<div class="flex shrink-0 gap-1.5">
							<Badge class={severityClass(f.severity)}>{f.severity}</Badge>
							<Badge variant="outline">{f.kind}</Badge>
						</div>
					</Card.Header>
					<Card.Content class="space-y-2 text-sm">
						<p>{f.description}</p>
						<p class="text-muted-foreground">
							<span class="font-medium">Data flow:</span>
							{f.data_flow}
						</p>
					</Card.Content>
				</Card.Root>
			{/each}
		</section>
	{/if}
</main>
