/**
 * Shiki integration. The `shiki` package is dynamically imported the first
 * time a highlight is requested — Vite splits it into its own lazy chunk so
 * cold-start doesn't pay for the grammar/theme JSON. We initialise a single
 * highlighter starting with `diff` for the patch viewer, then load
 * additional languages on demand. Both light and dark themes load up front
 * so we can flip via the `.dark` class on `<html>` without re-highlighting.
 */

import DOMPurify from 'isomorphic-dompurify';
// Type-only import — erased at compile time, doesn't pull `shiki` into the
// importing chunk. The runtime import lives in `getHighlighter()` below.
import type { Highlighter } from 'shiki';

const LIGHT_THEME = 'github-light';
const DARK_THEME = 'github-dark';

let highlighterPromise: Promise<Highlighter> | null = null;
const loadedLangs = new Set<string>(['diff']);
const langLoadInflight = new Map<string, Promise<void>>();

function getHighlighter(): Promise<Highlighter> {
	if (!highlighterPromise) {
		highlighterPromise = import('shiki').then((shiki) =>
			shiki.createHighlighter({
				themes: [LIGHT_THEME, DARK_THEME],
				langs: ['diff']
			})
		);
	}
	return highlighterPromise;
}

async function ensureLang(lang: string): Promise<boolean> {
	if (loadedLangs.has(lang)) return true;
	const inflight = langLoadInflight.get(lang);
	if (inflight) {
		await inflight;
		return loadedLangs.has(lang);
	}
	const promise = (async () => {
		const h = await getHighlighter();
		try {
			await h.loadLanguage(lang as never);
			loadedLangs.add(lang);
		} catch (e) {
			// Unknown language — record so we don't keep retrying.
			console.warn(`shiki: loadLanguage failed for '${lang}'`, e);
		} finally {
			langLoadInflight.delete(lang);
		}
	})();
	langLoadInflight.set(lang, promise);
	await promise;
	return loadedLangs.has(lang);
}

/** Render a unified-diff string as themed HTML. Returns a wrapper `<pre>` /
 *  `<code>` block from Shiki. The output is sanitised but Shiki produces
 *  well-formed HTML so the allowlist is permissive. */
export async function highlightDiff(diff: string): Promise<string> {
	const h = await getHighlighter();
	const html = h.codeToHtml(diff, {
		lang: 'diff',
		themes: { light: LIGHT_THEME, dark: DARK_THEME },
		// `defaultColor: 'light'` means the inline style applies the light
		// theme; CSS below flips to the dark vars under `.dark`.
		defaultColor: 'light'
	});
	return DOMPurify.sanitize(html, {
		// Shiki outputs <pre><code><span class style>... — allow those.
		ALLOWED_TAGS: ['pre', 'code', 'span', 'div', 'br'],
		ALLOWED_ATTR: ['class', 'style']
	});
}

/** Highlight source code in `lang`. Loads the grammar on first use; falls
 *  back to a plain pre/code escaping when the language is unknown. */
export async function highlightCode(code: string, lang: string | null): Promise<string> {
	const ok = lang ? await ensureLang(lang) : false;
	const h = await getHighlighter();
	const useLang = ok && lang ? lang : 'text';
	const html = h.codeToHtml(code, {
		lang: useLang,
		themes: { light: LIGHT_THEME, dark: DARK_THEME },
		defaultColor: 'light'
	});
	return DOMPurify.sanitize(html, {
		ALLOWED_TAGS: ['pre', 'code', 'span', 'div', 'br'],
		ALLOWED_ATTR: ['class', 'style']
	});
}
