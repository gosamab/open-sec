// Tiny browser component that renders a comment.
// Expected finding: vuln, high, CWE-79 (DOM XSS), around lines 13-15.

interface Comment {
	author: string;
	body: string; // user-supplied, never sanitized
}

export function renderComment(target: HTMLElement, comment: Comment) {
	const card = document.createElement('div');
	card.className = 'comment';

	// Both fields come from untrusted users and are written as HTML.
	card.innerHTML = `
		<strong>${comment.author}</strong>
		<p>${comment.body}</p>
	`;

	target.appendChild(card);
}
