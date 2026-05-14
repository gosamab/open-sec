import crypto from "crypto";
import express from "express";

const SECRET = process.env.WEBHOOK_SECRET ?? "";

export const webhook = express.Router();

webhook.post("/hooks/payments", express.raw({ type: "application/json" }), (req, res) => {
  const sig = req.header("x-signature") ?? "";
  const mac = crypto.createHmac("sha256", SECRET).update(req.body).digest("hex");
  if (sig !== mac) {
    return res.status(401).end();
  }
  const payload = JSON.parse(req.body.toString("utf8"));
  // ... dispatch to handlers based on payload.event ...
  res.json({ received: payload.event });
});
