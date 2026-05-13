// Express route with classic string-concatenated SQL.
// Expected finding: vuln, high+, CWE-89, around lines 16-18.

import express from 'express';
import { Pool } from 'pg';

const app = express();
const db = new Pool();

app.get('/users/:id', async (req, res) => {
	const userId = req.params.id;
	const includeDeleted = req.query.includeDeleted === 'true';

	// Builds a SQL string by interpolating untrusted path + query params.
	const sql = `
		SELECT id, email, name FROM users
		WHERE id = ${userId}
		AND deleted = ${includeDeleted}
	`;

	try {
		const result = await db.query(sql);
		res.json(result.rows[0] ?? null);
	} catch (err) {
		res.status(500).json({ error: 'lookup failed' });
	}
});

app.listen(3000);
