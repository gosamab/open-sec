// Webhook-style endpoint that fetches an arbitrary URL given by the caller.
// Expected finding: vuln, high, CWE-918 (SSRF), around lines 12-18.

import express from 'express';

const app = express();
app.use(express.json());

app.post('/preview', async (req, res) => {
	const target = req.body.url as string;

	// No allowlist, no DNS pin, no rejection of internal/metadata addresses.
	// 169.254.169.254, http://localhost, file://, etc. are all reachable.
	const upstream = await fetch(target);
	const body = await upstream.text();

	res.set('content-type', upstream.headers.get('content-type') ?? 'text/plain');
	res.send(body);
});

app.listen(3000);
