<script lang="ts">
	import { onMount } from 'svelte';
	import {
		getLatestScanFor,
		type Finding,
		type Patch,
		type ScanResult,
		type Severity,
		type VerifiedFinding
	} from '$lib/ipc';
	import { renderInlineMd, renderMd } from '$lib/markdown';
	import { highlightDiff } from '$lib/shiki.svelte';

	let result = $state<ScanResult | null>(null);
	let error = $state<string | null>(null);
	let diffHtmlByFindingId = $state<Map<string, string>>(new Map());

	const SEVERITY_ORDER: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

	function rank(s: Severity): number {
		return SEVERITY_ORDER.indexOf(s);
	}

	let displayed = $derived.by<VerifiedFinding[]>(() => {
		if (!result) return [];
		return result.verified
			.filter(
				(v) => v.finding.kind === 'hardening' || (v.verdict && v.verdict.is_reachable && v.verdict.concrete_exploit)
			)
			.sort((a, b) => rank(a.finding.severity) - rank(b.finding.severity));
	});

	let patchByFinding = $derived.by(() => {
		const m = new Map<string, Patch>();
		if (result) for (const p of result.patches) m.set(p.finding_id, p);
		return m;
	});

	let counts = $derived.by(() => {
		if (!result) return { kept: 0, hardening: 0, dropped: 0 };
		let kept = 0;
		let hardening = 0;
		for (const v of result.verified) {
			if (v.finding.kind === 'hardening') hardening++;
			else if (v.verdict?.is_reachable && v.verdict.concrete_exploit) kept++;
		}
		return { kept, hardening, dropped: result.verified.length - kept - hardening };
	});

	function basename(p: string): string {
		return p.split(/[\\/]/).pop() || p;
	}

	function nowDate(): string {
		const d = new Date();
		return d.toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' });
	}

	function dataFlowSteps(text: string): string[] {
		return text
			.split(/\s*(?:→|->)\s*/g)
			.map((s) => s.trim())
			.filter(Boolean);
	}

	onMount(async () => {
		const params = new URLSearchParams(window.location.search);
		const root = params.get('root');
		const auto = params.get('auto') === '1';
		if (!root) {
			error = 'missing root query param';
			return;
		}
		try {
			result = await getLatestScanFor(root);
			// Pre-highlight every patch diff in parallel so the rendered page
			// is paint-ready before we trigger print().
			const map = new Map<string, string>();
			await Promise.all(
				(result.patches ?? []).map(async (p) => {
					if (!p.diff) return;
					try {
						const html = await highlightDiff(p.diff);
						map.set(p.finding_id, html);
					} catch (e) {
						console.warn('highlightDiff failed in report', e);
					}
				})
			);
			diffHtmlByFindingId = map;
			if (auto) {
				// Wait two animation frames so Shiki's HTML is rendered before
				// the browser snapshots for the print preview.
				requestAnimationFrame(() => requestAnimationFrame(() => window.print()));
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	});

	function severityClass(s: Severity): string {
		return `sev sev-${s}`;
	}
</script>

<svelte:head>
	<title>open-sec · report</title>
	<style>
		:global(html, body) {
			background: #f8f8f8;
			color: #1a1a1a;
			margin: 0;
			padding: 0;
			font-family:
				ui-sans-serif,
				system-ui,
				-apple-system,
				'Segoe UI',
				sans-serif;
			font-feature-settings:
				'cv11',
				'ss01';
			line-height: 1.5;
			/* Force every background/border to print — browsers default to
			   "ink-saving" which drops colors and turns the whole thing into
			   plain black-on-white. */
			-webkit-print-color-adjust: exact;
			print-color-adjust: exact;
		}
		:global(*) {
			-webkit-print-color-adjust: exact;
			print-color-adjust: exact;
		}
		.sheet {
			max-width: 780px;
			margin: 2rem auto;
			background: #fff;
			padding: 2.5rem 2.75rem;
			border-radius: 4px;
			box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
			font-size: 12.5px;
		}
		h1 {
			font-size: 1.7rem;
			letter-spacing: -0.01em;
			font-weight: 600;
			margin: 0 0 0.35rem;
		}
		.subtle {
			color: #6b6b6b;
			font-size: 0.85rem;
		}
		.summary {
			margin-top: 1.5rem;
			padding: 1rem 1.1rem;
			border: 1px solid #eaeaea;
			border-radius: 6px;
			background: #fafafa;
		}
		.summary dl {
			display: grid;
			grid-template-columns: 140px 1fr;
			gap: 0.25rem 1rem;
			margin: 0;
			font-size: 12px;
		}
		.summary dt {
			color: #6b6b6b;
			text-transform: uppercase;
			letter-spacing: 0.04em;
			font-size: 10px;
			align-self: center;
		}
		.summary dd {
			margin: 0;
			font-variant-numeric: tabular-nums;
		}
		.summary code {
			font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
			font-size: 0.85em;
		}
		.finding {
			margin-top: 1.75rem;
			border: 1px solid #eaeaea;
			border-left: 4px solid #999;
			border-radius: 6px;
			padding: 1rem 1.25rem 1.1rem;
			page-break-inside: avoid;
			break-inside: avoid;
		}
		.finding.sev-critical {
			border-left-color: #dc2626;
		}
		.finding.sev-high {
			border-left-color: #ea580c;
		}
		.finding.sev-medium {
			border-left-color: #d97706;
		}
		.finding.sev-low {
			border-left-color: #2563eb;
		}
		.finding.sev-info {
			border-left-color: #6b7280;
		}
		.finding h2 {
			font-size: 1.05rem;
			margin: 0.3rem 0 0.25rem;
			font-weight: 600;
			letter-spacing: -0.005em;
		}
		.tagrow {
			display: flex;
			flex-wrap: wrap;
			gap: 0.35rem;
			align-items: center;
			font-size: 10.5px;
		}
		.tag {
			display: inline-block;
			padding: 1px 6px;
			border-radius: 3px;
			font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
			letter-spacing: 0.02em;
			background: #eef0f3;
			color: #2b2b2b;
		}
		.tag.sev {
			color: #fff;
			font-weight: 600;
		}
		.tag.sev.sev-critical {
			background: #dc2626;
		}
		.tag.sev.sev-high {
			background: #ea580c;
		}
		.tag.sev.sev-medium {
			background: #d97706;
			color: #2c1d04;
		}
		.tag.sev.sev-low {
			background: #2563eb;
		}
		.tag.sev.sev-info {
			background: #6b7280;
		}
		.tag-kind {
			background: transparent;
			border: 1px solid #d1d5db;
			color: #6b7280;
		}
		.location {
			color: #6b6b6b;
			font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
			font-size: 0.78rem;
			margin: 0.35rem 0 0.8rem;
			word-break: break-all;
		}
		.section-label {
			text-transform: uppercase;
			letter-spacing: 0.06em;
			font-size: 9.5px;
			color: #8a8a8a;
			margin: 0.9rem 0 0.3rem;
			font-weight: 600;
		}
		.dataflow {
			margin: 0;
			padding-left: 1.4rem;
			font-size: 12px;
		}
		.dataflow li {
			margin: 0.1rem 0;
		}
		.exploit {
			background: #fef9f6;
			border: 1px solid #f5e0d4;
			border-radius: 5px;
			padding: 0.55rem 0.75rem;
			font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
			font-size: 11px;
		}
		.exploit-row {
			display: grid;
			grid-template-columns: 70px 1fr;
			gap: 0.5rem;
			padding: 1px 0;
		}
		.exploit-row .k {
			color: #9a4d18;
			text-transform: uppercase;
			font-size: 9.5px;
			letter-spacing: 0.05em;
			align-self: center;
		}
		.exploit-row .v {
			word-break: break-all;
		}
		.md {
			font-size: 12.5px;
		}
		.md :global(p) {
			margin: 0.35rem 0;
		}
		.md :global(code) {
			background: #f1f3f5;
			padding: 0.05em 0.35em;
			border-radius: 3px;
			font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
			font-size: 0.85em;
		}
		/* Shiki diff blocks inherit their own colors; we override sizing only. */
		.diffwrap :global(pre.shiki) {
			margin: 0;
			padding: 0.7rem 0.85rem;
			border: 1px solid #e5e7eb;
			border-radius: 5px;
			font-size: 10.5px;
			line-height: 1.5;
			background: #fafafa !important;
			overflow: hidden; /* in print, lines wrap */
		}
		.diffwrap :global(pre.shiki code) {
			white-space: pre-wrap;
			word-break: break-word;
		}
		.no-findings {
			padding: 2rem 0;
			text-align: center;
			color: #6b6b6b;
		}
		@page {
			size: A4;
			margin: 0.45in;
		}
		@media print {
			:global(html, body) {
				background: #fff;
			}
			.sheet {
				margin: 0;
				padding: 0;
				border-radius: 0;
				box-shadow: none;
				max-width: none;
			}
			.finding {
				page-break-inside: avoid;
				break-inside: avoid;
			}
			.finding,
			.summary,
			.exploit {
				box-shadow: none;
			}
			h1,
			h2 {
				break-after: avoid;
				page-break-after: avoid;
			}
		}
	</style>
</svelte:head>

{#if error}
	<div class="sheet">
		<h1>Report unavailable</h1>
		<p class="subtle">{error}</p>
	</div>
{:else if !result}
	<div class="sheet">
		<p class="subtle">Generating report…</p>
	</div>
{:else}
	<div class="sheet">
		<header>
			<h1>{basename(result.root)} <span class="subtle" style="font-weight: 400">security report</span></h1>
			<p class="subtle">
				Generated {nowDate()} by open-sec
			</p>
		</header>

		<section class="summary">
			<dl>
				<dt>Project</dt>
				<dd><code>{result.root}</code></dd>
				<dt>Files scanned</dt>
				<dd>{result.findings_by_file.length}</dd>
				<dt>Findings</dt>
				<dd>
					{counts.kept} kept · {counts.hardening} hardening · {counts.dropped} dropped by verifier
				</dd>
				<dt>Token usage</dt>
				<dd>
					{result.usage.total.input_tokens.toLocaleString()} in /
					{result.usage.total.output_tokens.toLocaleString()} out /
					{result.usage.total.cache_read_input_tokens.toLocaleString()} cache rd
				</dd>
			</dl>
		</section>

		{#if displayed.length === 0}
			<div class="no-findings">No findings retained — clean scan.</div>
		{:else}
			{#each displayed as v (v.finding.id)}
				{@const f = v.finding}
				{@const patch = patchByFinding.get(f.id)}
				<article class="finding {severityClass(f.severity)}">
					<div class="tagrow">
						<span class={'tag ' + severityClass(f.severity)}>{f.severity}</span>
						<span class="tag tag-kind">{f.kind}</span>
						<span class="tag">{f.cwe}</span>
						{#if f.owasp}<span class="tag">OWASP {f.owasp}</span>{/if}
					</div>
					<h2>{f.title}</h2>
					<div class="location">{f.file}:{f.line_start}{f.line_end !== f.line_start ? `-${f.line_end}` : ''}</div>

					<div class="section-label">Description</div>
					<div class="md">{@html renderMd(f.description)}</div>

					<div class="section-label">Data flow</div>
					<ol class="dataflow">
						{#each dataFlowSteps(f.data_flow) as step, i (i)}
							<li>{@html renderInlineMd(step)}</li>
						{/each}
					</ol>

					{#if v.verdict}
						<div class="section-label">
							Verifier — reachable: {String(v.verdict.is_reachable)}, untrusted source: {String(v.verdict.source_is_untrusted)}
						</div>
						<div class="md">{@html renderMd(v.verdict.reasoning)}</div>

						{#if v.verdict.concrete_exploit}
							{@const ex = v.verdict.concrete_exploit}
							<div class="section-label">Exploit</div>
							<div class="exploit">
								<div class="exploit-row">
									<span class="k">Kind</span>
									<span class="v">{ex.kind}</span>
								</div>
								{#if ex.request}
									<div class="exploit-row">
										<span class="k">Request</span>
										<span class="v">{ex.request.method} {ex.request.path}</span>
									</div>
								{/if}
								<div class="exploit-row">
									<span class="k">Payload</span>
									<span class="v">{ex.payload}</span>
								</div>
								<div class="exploit-row">
									<span class="k">Effect</span>
									<span class="v">{ex.expected_effect}</span>
								</div>
							</div>
						{/if}
					{/if}

					{#if patch}
						<div class="section-label">
							Suggested patch
							{#if patch.located.kind === 'fuzzy'}
								<span class="subtle" style="text-transform: none; font-weight: 400; font-size: 10px">— fuzzy match</span>
							{:else if patch.located.kind === 'not_found'}
								<span class="subtle" style="text-transform: none; font-weight: 400; font-size: 10px">— old_block not located</span>
							{/if}
						</div>
						<div class="md">{@html renderMd(patch.proposal.explanation)}</div>
						{#if patch.diff}
							{#if diffHtmlByFindingId.get(f.id)}
								<div class="diffwrap">{@html diffHtmlByFindingId.get(f.id)}</div>
							{:else}
								<pre class="diffwrap" style="white-space: pre-wrap; word-break: break-word; font-size: 10.5px; padding: 0.7rem 0.85rem; background: #fafafa; border: 1px solid #e5e7eb; border-radius: 5px;">{patch.diff}</pre>
							{/if}
						{:else}
							<pre style="white-space: pre-wrap; word-break: break-word; font-size: 10.5px; padding: 0.7rem 0.85rem; background: #fafafa; border: 1px solid #e5e7eb; border-radius: 5px;">- {patch.proposal.old_block}
+ {patch.proposal.new_block}</pre>
						{/if}
					{/if}
				</article>
			{/each}
		{/if}
	</div>
{/if}
