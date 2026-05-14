/**
 * Theme state for the app. Resolves on first access from (in order):
 *   1. localStorage "open-sec:theme" — explicit user choice
 *   2. `prefers-color-scheme: dark` — OS preference
 *   3. "light"
 *
 * `cycle()` flips between light/dark and writes back to localStorage so the
 * choice sticks across reloads. The `.dark` class on <html> is applied via
 * an effect started in the layout.
 */

const STORAGE_KEY = 'open-sec:theme';

export type ThemeName = 'light' | 'dark';

function initial(): ThemeName {
	if (typeof window === 'undefined') return 'light';
	const saved = window.localStorage.getItem(STORAGE_KEY);
	if (saved === 'light' || saved === 'dark') return saved;
	if (window.matchMedia?.('(prefers-color-scheme: dark)').matches) return 'dark';
	return 'light';
}

class ThemeStore {
	value: ThemeName = $state(initial());

	cycle() {
		this.value = this.value === 'dark' ? 'light' : 'dark';
		if (typeof window !== 'undefined') {
			window.localStorage.setItem(STORAGE_KEY, this.value);
		}
	}

	apply(doc: Document) {
		doc.documentElement.classList.toggle('dark', this.value === 'dark');
	}
}

export const theme = new ThemeStore();
