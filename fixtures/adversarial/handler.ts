// Looks like classic SQLi at first glance: `req.params.id` is interpolated
// directly into a SQL string. But the route is gated by the `validateHexId`
// middleware (see ./middleware.ts), which rejects anything not matching a
// strict 16-char hex pattern. Verifier needs to read the middleware to
// realize the source is constrained.

import express from "express";
import { validateHexId } from "./middleware";
import { db } from "./db";

export const router = express.Router();

router.get("/users/:id", validateHexId, async (req, res) => {
  const sql = `SELECT id, email, role FROM users WHERE id = '${req.params.id}'`;
  const rows = await db.query(sql);
  res.json(rows);
});
