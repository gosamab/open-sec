/**
 * User-tunable scan settings. Persisted in localStorage; passed through to
 * `run_pipeline` on each scan as an optional override of the backend's
 * defaults. Keeping it client-side avoids a separate read-on-startup IPC
 * roundtrip and matches the "settings live with the UI" model — the backend
 * always has its own defaults if no override is provided.
 */

const STORAGE_KEY = 'open-sec:settings';

export interface ScanSettings {
	triage_concurrency: number;
	detect_concurrency: number;
	verify_concurrency: number;
	patch_concurrency: number;
	triage_model: string;
	detect_model: string;
	verify_model: string;
	patch_model: string;
	/** Combined input+output token cap across the whole scan. 0 = unlimited. */
	budget_total_tokens: number;
	/** Saudi riyal per USD. Default 3.75 is the SAMA peg (in place since 1986).
	 *  Exposed in case the peg ever moves or the user wants a custom rate. */
	sar_per_usd: number;
}

export const DEFAULT_SETTINGS: ScanSettings = {
	triage_concurrency: 8,
	detect_concurrency: 4,
	verify_concurrency: 2,
	patch_concurrency: 4,
	triage_model: 'claude-haiku-4-5',
	detect_model: 'claude-sonnet-4-6',
	verify_model: 'claude-opus-4-7',
	patch_model: 'claude-sonnet-4-6',
	budget_total_tokens: 0,
	sar_per_usd: 3.75
};

function load(): ScanSettings {
	if (typeof window === 'undefined') return { ...DEFAULT_SETTINGS };
	try {
		const raw = window.localStorage.getItem(STORAGE_KEY);
		if (!raw) return { ...DEFAULT_SETTINGS };
		const parsed = JSON.parse(raw);
		return { ...DEFAULT_SETTINGS, ...parsed };
	} catch {
		return { ...DEFAULT_SETTINGS };
	}
}

function save(s: ScanSettings) {
	if (typeof window === 'undefined') return;
	try {
		window.localStorage.setItem(STORAGE_KEY, JSON.stringify(s));
	} catch {
		// Quota / disabled — silent.
	}
}

/** Concurrency bounds. Exported so the Settings UI uses the same min/max
 *  values as the clamp logic — keeps them from drifting. */
export const CONCURRENCY_BOUNDS: Record<
	'triage_concurrency' | 'detect_concurrency' | 'verify_concurrency' | 'patch_concurrency',
	{ min: number; max: number }
> = {
	triage_concurrency: { min: 1, max: 32 },
	detect_concurrency: { min: 1, max: 16 },
	verify_concurrency: { min: 1, max: 8 },
	patch_concurrency: { min: 1, max: 8 }
};

function clampInt(v: unknown, lo: number, hi: number, fallback: number): number {
	const n = typeof v === 'number' ? v : Number(v);
	if (!Number.isFinite(n)) return fallback;
	return Math.min(Math.max(Math.trunc(n), lo), hi);
}

function clampFloat(v: unknown, lo: number, hi: number, fallback: number): number {
	const n = typeof v === 'number' ? v : Number(v);
	if (!Number.isFinite(n) || n <= 0) return fallback;
	return Math.min(Math.max(n, lo), hi);
}

/** Clamp every numeric field against its bounds. Strings stay untouched. */
function sanitize(s: ScanSettings): ScanSettings {
	return {
		...s,
		triage_concurrency: clampInt(
			s.triage_concurrency,
			CONCURRENCY_BOUNDS.triage_concurrency.min,
			CONCURRENCY_BOUNDS.triage_concurrency.max,
			DEFAULT_SETTINGS.triage_concurrency
		),
		detect_concurrency: clampInt(
			s.detect_concurrency,
			CONCURRENCY_BOUNDS.detect_concurrency.min,
			CONCURRENCY_BOUNDS.detect_concurrency.max,
			DEFAULT_SETTINGS.detect_concurrency
		),
		verify_concurrency: clampInt(
			s.verify_concurrency,
			CONCURRENCY_BOUNDS.verify_concurrency.min,
			CONCURRENCY_BOUNDS.verify_concurrency.max,
			DEFAULT_SETTINGS.verify_concurrency
		),
		patch_concurrency: clampInt(
			s.patch_concurrency,
			CONCURRENCY_BOUNDS.patch_concurrency.min,
			CONCURRENCY_BOUNDS.patch_concurrency.max,
			DEFAULT_SETTINGS.patch_concurrency
		),
		budget_total_tokens: clampInt(s.budget_total_tokens, 0, Number.MAX_SAFE_INTEGER, 0),
		sar_per_usd: clampFloat(s.sar_per_usd, 0.0001, 1000, DEFAULT_SETTINGS.sar_per_usd)
	};
}

class SettingsStore {
	value: ScanSettings = $state(load());

	update(partial: Partial<ScanSettings>) {
		this.value = sanitize({ ...this.value, ...partial });
		save(this.value);
	}

	reset() {
		this.value = { ...DEFAULT_SETTINGS };
		save(this.value);
	}
}

export const settings = new SettingsStore();

/** Backend-shaped override. budget_total_tokens=0 is converted to None. */
export function asScanConfig(s: ScanSettings) {
	return {
		triage_concurrency: s.triage_concurrency,
		detect_concurrency: s.detect_concurrency,
		verify_concurrency: s.verify_concurrency,
		patch_concurrency: s.patch_concurrency,
		triage_model: s.triage_model,
		detect_model: s.detect_model,
		verify_model: s.verify_model,
		patch_model: s.patch_model,
		budget_total_tokens: s.budget_total_tokens > 0 ? s.budget_total_tokens : null
	};
}
