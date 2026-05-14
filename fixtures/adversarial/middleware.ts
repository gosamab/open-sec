// Strict allowlist: only accept 16-char lowercase-hex ids. Any other input
// is rejected with 400 before the handler runs. This effectively makes
// `req.params.id` non-attacker-controlled by the time SQL is built.

import type { Request, Response, NextFunction } from "express";

const HEX_ID = /^[a-f0-9]{16}$/;

export function validateHexId(req: Request, res: Response, next: NextFunction) {
  const id = req.params.id;
  if (typeof id !== "string" || !HEX_ID.test(id)) {
    return res.status(400).json({ error: "invalid id" });
  }
  next();
}
