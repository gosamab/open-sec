// Same shape as sqli_express.ts but uses parameterized queries.
// Expected: 0 findings, or only "hardening" / "info" at most.

import express from 'express';
import { Pool } from 'pg';

const app = express();
const db = new Pool();

const ALLOWED_FIELDS = new Set(['id', 'email', 'name', 'created_at']);

app.get('/users/:id', async (req, res) => {
	const userId = Number(req.params.id);
	if (!Number.isInteger(userId) || userId <= 0) {
		return res.status(400).json({ error: 'invalid id' });
	}

	const requested = (req.query.fields as string | undefined) ?? 'id,email,name';
	const fields = requested
		.split(',')
		.map((f) => f.trim())
		.filter((f) => ALLOWED_FIELDS.has(f));

	if (fields.length === 0) {
		return res.status(400).json({ error: 'no valid fields' });
	}

	// Identifiers are validated against an allowlist; values use $1 placeholders.
	const sql = `SELECT ${fields.join(', ')} FROM users WHERE id = $1`;
	const result = await db.query(sql, [userId]);
	res.json(result.rows[0] ?? null);
});

app.listen(3000);
