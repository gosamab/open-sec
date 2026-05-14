import DOMPurify from 'isomorphic-dompurify';
import { Marked } from 'marked';

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

function sanitize(html: string): string {
	return DOMPurify.sanitize(html, {
		ALLOWED_TAGS: SAFE_TAGS,
		ALLOWED_ATTR: ['href', 'title']
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
