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
	budget_total_tokens: 0
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

class SettingsStore {
	value: ScanSettings = $state(load());

	update(partial: Partial<ScanSettings>) {
		this.value = { ...this.value, ...partial };
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
