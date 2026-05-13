// Looks safe at a glance — calls sanitize() before the DB query.
// The real vuln only shows up if you READ sanitize.ts.
// Expected with tools: vuln CWE-89 high.
// Expected without tools: nothing, or weak hardening at best.

import express from 'express';
import { Pool } from 'pg';
import { sanitize } from './sanitize';

const app = express();
const db = new Pool();

app.get('/users/:id', async (req, res) => {
	const id = sanitize(req.params.id);
	const sql = `SELECT id, email FROM users WHERE id = ${id}`;
	const result = await db.query(sql);
	res.json(result.rows[0] ?? null);
});

app.listen(3000);
