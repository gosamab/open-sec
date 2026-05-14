import express from "express";
import bcrypt from "bcrypt";
import { db } from "../db/client";
import { signJwt } from "./jwt";

const router = express.Router();

router.post("/login", async (req, res) => {
  const { email, password } = req.body;
  if (typeof email !== "string" || typeof password !== "string") {
    return res.status(400).json({ error: "bad input" });
  }
  const user = await db.users.findOne({ email });
  if (!user) return res.status(401).json({ error: "no such user" });

  const ok = await bcrypt.compare(password, user.password_hash);
  if (!ok) return res.status(401).json({ error: "bad password" });

  const token = signJwt({ sub: user.id, role: user.role });
  res.cookie("session", token, { httpOnly: true, sameSite: "strict" });
  res.json({ ok: true });
});

export default router;
