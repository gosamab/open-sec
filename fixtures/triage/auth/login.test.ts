import request from "supertest";
import app from "./test-app";
import { db } from "../db/client";

jest.mock("../db/client");

describe("POST /login", () => {
  beforeEach(() => {
    (db.users.findOne as jest.Mock).mockReset();
  });

  it("400s on missing fields", async () => {
    const res = await request(app).post("/login").send({});
    expect(res.status).toBe(400);
  });

  it("401s when the user doesn't exist", async () => {
    (db.users.findOne as jest.Mock).mockResolvedValue(null);
    const res = await request(app).post("/login").send({ email: "a@b", password: "x" });
    expect(res.status).toBe(401);
  });

  it("sets a session cookie on success", async () => {
    (db.users.findOne as jest.Mock).mockResolvedValue({
      id: "u_1",
      email: "a@b",
      password_hash: "$2b$10$fakehash",
      role: "user",
    });
    const res = await request(app).post("/login").send({ email: "a@b", password: "hunter2" });
    expect(res.status).toBe(200);
    expect(res.headers["set-cookie"]).toBeDefined();
  });
});
