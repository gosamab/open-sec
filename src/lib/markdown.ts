import DOMPurify from 'isomorphic-dompurify';
import { Marked } from 'marked';
import { openUrl } from '$lib/ipc';

// Inline-only mode for short fragments (badge labels, single sentences with
// backticks) — strips the wrapping <p>.
const inlineParser = new Marked({ async: false, gfm: true, breaks: false });
const blockParser = new Marked({ async: false, gfm: true, breaks: true });

const SAFE_TAGS = [
	'p',
	'strong',
	'em',
	'code',
	'pre',
	'ul',
	'ol',
	'li',
	'a',
	'br',
	'blockquote',
	'h3',
	'h4'
];

// Anchor-hardening hook: anything DOMPurify lets through as <a> gets forced
// into "external link" shape. The href is also re-validated here against an
// explicit scheme allowlist because the renderer's input is LLM-generated.
let hookInstalled = false;
function ensureAnchorHook() {
	if (hookInstalled) return;
	DOMPurify.addHook('afterSanitizeAttributes', (node) => {
		if (!(node instanceof Element) || node.tagName !== 'A') return;
		const href = node.getAttribute('href') ?? '';
		const lower = href.trim().toLowerCase();
		const ok =
			lower.startsWith('https://') || lower.startsWith('http://') || lower.startsWith('mailto:');
		if (!ok) {
			// Drop the link entirely — leave the inner text in place.
			node.removeAttribute('href');
			node.removeAttribute('target');
			node.removeAttribute('rel');
			node.setAttribute('data-md-link-removed', '1');
			return;
		}
		node.setAttribute('target', '_blank');
		node.setAttribute('rel', 'noopener noreferrer');
		// Tag the node so a single delegated click listener can intercept all
		// markdown-anchor clicks (see installExternalLinkHandler).
		node.setAttribute('data-md-link', 'external');
	});
	hookInstalled = true;
}

function sanitize(html: string): string {
	ensureAnchorHook();
	return DOMPurify.sanitize(html, {
		ALLOWED_TAGS: SAFE_TAGS,
		ALLOWED_ATTR: ['href', 'title', 'target', 'rel', 'data-md-link', 'data-md-link-removed'],
		ALLOWED_URI_REGEXP: /^(?:https?|mailto):/i
	});
}

/** Render markdown that's expected to span multiple paragraphs / lists. */
export function renderMd(text: string | null | undefined): string {
	if (!text) return '';
	const html = blockParser.parse(text) as string;
	return sanitize(html);
}

/** Render markdown for a single inline fragment (no paragraph wrapper). */
export function renderInlineMd(text: string | null | undefined): string {
	if (!text) return '';
	const html = inlineParser.parseInline(text) as string;
	return sanitize(html);
}

let clickHandlerInstalled = false;

/** Install a single document-level click listener that routes markdown-anchor
 *  clicks through the Tauri shell (so the webview never navigates). Idempotent:
 *  safe to call multiple times. Call once from the root layout. */
export function installExternalLinkHandler(): void {
	if (clickHandlerInstalled || typeof document === 'undefined') return;
	clickHandlerInstalled = true;
	document.addEventListener('click', (e) => {
		const target = e.target;
		if (!(target instanceof Element)) return;
		const anchor = target.closest('a[data-md-link="external"]') as HTMLAnchorElement | null;
		if (!anchor) return;
		const href = anchor.getAttribute('href');
		if (!href) return;
		e.preventDefault();
		void openUrl(href).catch((err: unknown) => {
			console.warn('openUrl failed', err);
		});
	});
}
