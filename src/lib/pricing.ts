/**
 * Anthropic price tables and currency conversion helpers.
 *
 * Prices are USD per million tokens, listed for input, output, prompt-cache
 * write (1-hour TTL — see CLAUDE.md) and cache read. The 1-hour cache write
 * is the base input price × 2; cache reads are × 0.10. We bake those
 * multipliers into the table rather than computing them at call sites so the
 * source of truth stays at the top of this file.
 *
 * The Saudi riyal is pegged to USD at 3.75 by SAMA, but the rate is exposed
 * as a Setting so the user can override if the peg ever moves.
 */
import type { Usage, StageUsage } from './ipc';

export interface ModelPrices {
	/** USD per million input tokens (uncached). */
	input: number;
	/** USD per million output tokens. */
	output: number;
	/** USD per million tokens written to the 1-hour prompt cache (2 × input). */
	cache_write_1h: number;
	/** USD per million tokens read from the prompt cache (0.10 × input). */
	cache_read: number;
}

/** Known Claude models and their per-MTok pricing. Keys must match the model
 *  IDs sent in `ScanConfig` (see settings.svelte.ts defaults). Unknown models
 *  fall through to {@link FALLBACK_PRICES} — a Sonnet-grade placeholder so
 *  estimates stay in the right ballpark even for misspelled or new model IDs. */
export const MODEL_PRICES: Record<string, ModelPrices> = {
	'claude-haiku-4-5': {
		input: 1,
		output: 5,
		cache_write_1h: 2,
		cache_read: 0.1
	},
	'claude-sonnet-4-6': {
		input: 3,
		output: 15,
		cache_write_1h: 6,
		cache_read: 0.3
	},
	'claude-opus-4-7': {
		input: 15,
		output: 75,
		cache_write_1h: 30,
		cache_read: 1.5
	}
};

/** Used when a model ID isn't in {@link MODEL_PRICES}. Sonnet pricing as a
 *  middle-of-the-road default — better than zero (free) or Opus (alarmist). */
export const FALLBACK_PRICES: ModelPrices = MODEL_PRICES['claude-sonnet-4-6'];

export function priceFor(model: string): ModelPrices {
	return MODEL_PRICES[model] ?? FALLBACK_PRICES;
}

/** Approximate output-token throughput per model (tokens / second). Used to
 *  estimate wall-clock time. Output dominates latency in a streamed call —
 *  the input is processed quickly, then tokens trickle out at these rates.
 *  Numbers are rough field observations; actual throughput varies with
 *  region and load. */
export const MODEL_THROUGHPUT_TOK_PER_SEC: Record<string, number> = {
	'claude-haiku-4-5': 150,
	'claude-sonnet-4-6': 85,
	'claude-opus-4-7': 55
};

export const FALLBACK_THROUGHPUT_TOK_PER_SEC = 85;

export function throughputFor(model: string): number {
	return MODEL_THROUGHPUT_TOK_PER_SEC[model] ?? FALLBACK_THROUGHPUT_TOK_PER_SEC;
}

/** USD cost for a single stage's token usage on the given model. */
export function costUSD(u: Usage, model: string): number {
	const p = priceFor(model);
	return (
		(u.input_tokens * p.input +
			u.output_tokens * p.output +
			u.cache_creation_input_tokens * p.cache_write_1h +
			u.cache_read_input_tokens * p.cache_read) /
		1_000_000
	);
}

export interface StageModels {
	triage_model: string;
	detect_model: string;
	verify_model: string;
	patch_model: string;
}

export interface StageCosts {
	triage: number;
	detect: number;
	verify: number;
	patch: number;
	total: number;
}

/** Per-stage USD cost for a full scan. Each stage is priced against its own
 *  model — the four can differ (Haiku for triage, Opus for verify, etc.). */
export function stageCostsUSD(usage: StageUsage, models: StageModels): StageCosts {
	const triage = costUSD(usage.triage, models.triage_model);
	const detect = costUSD(usage.detect, models.detect_model);
	const verify = costUSD(usage.verify, models.verify_model);
	const patch = costUSD(usage.patch, models.patch_model);
	return { triage, detect, verify, patch, total: triage + detect + verify + patch };
}

export function usdToSAR(usd: number, rate: number): number {
	return usd * rate;
}

/** "$0.05" / "$1.23" / "$12.45". Always 2 dp for $≥0.01; "<$0.01" below that
 *  so tiny estimates don't read as free. */
export function formatUSD(usd: number): string {
	if (usd > 0 && usd < 0.01) return '<$0.01';
	return `$${usd.toFixed(2)}`;
}

/** "0.19 SAR" / "4.61 SAR" / "46.7 SAR". Same epsilon convention as USD. */
export function formatSAR(sar: number): string {
	if (sar > 0 && sar < 0.01) return '<0.01 SAR';
	return `${sar.toFixed(2)} SAR`;
}
