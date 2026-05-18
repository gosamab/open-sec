/**
 * Theme state for the app.
 *
 * `value` is the user's *choice* — one of four:
 *   - 'system'   : follow the OS `prefers-color-scheme` (resolves to light or midnight)
 *   - 'light'    : forced light
 *   - 'midnight' : forced midnight (softer dark)
 *   - 'dark'     : forced dark (crisp, near-black)
 *
 * `resolved` is the *applied* theme — always one of 'light' | 'midnight' | 'dark'.
 * When `value === 'system'`, resolved tracks the OS preference and flips live.
 *
 * `.dark` is added to <html> for both midnight and dark so every `dark:`
 * Tailwind utility keeps working. A second `.midnight` class layered on top
 * overrides the CSS variables to the softer palette.
 */

const STORAGE_KEY = 'open-sec:theme';

export type ThemeChoice = 'system' | 'light' | 'midnight' | 'dark';
export type AppliedTheme = 'light' | 'midnight' | 'dark';

function loadChoice(): ThemeChoice {
	if (typeof window === 'undefined') return 'system';
	const saved = window.localStorage.getItem(STORAGE_KEY);
	if (saved === 'system' || saved === 'light' || saved === 'midnight' || saved === 'dark') {
		return saved;
	}
	return 'system';
}

function systemPrefersDark(): boolean {
	if (typeof window === 'undefined' || !window.matchMedia) return false;
	return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

class ThemeStore {
	/** User's selection. Persisted to localStorage. */
	value: ThemeChoice = $state(loadChoice());

	/** Cached OS preference. Updated by `watchSystem()` when the user has
	 *  `value === 'system'` so `resolved` flips live. */
	systemDark: boolean = $state(systemPrefersDark());

	/** The theme actually applied to the document. */
	resolved: AppliedTheme = $derived.by(() => {
		if (this.value === 'system') return this.systemDark ? 'midnight' : 'light';
		return this.value;
	});

	set(choice: ThemeChoice) {
		this.value = choice;
		if (typeof window !== 'undefined') {
			window.localStorage.setItem(STORAGE_KEY, choice);
		}
	}

	/** Subscribe to OS color-scheme changes. Returns an unsubscribe fn.
	 *  Call once at mount (from the root layout). */
	watchSystem(): () => void {
		if (typeof window === 'undefined' || !window.matchMedia) return () => {};
		const mq = window.matchMedia('(prefers-color-scheme: dark)');
		const onChange = (e: MediaQueryListEvent) => {
			this.systemDark = e.matches;
		};
		mq.addEventListener('change', onChange);
		return () => mq.removeEventListener('change', onChange);
	}

	apply(doc: Document) {
		const r = this.resolved;
		const cls = doc.documentElement.classList;
		cls.toggle('dark', r !== 'light');
		cls.toggle('midnight', r === 'midnight');
	}
}

export const theme = new ThemeStore();
