// Shared API types. Pure declarations, no runtime behavior.

export interface User {
  id: string;
  email: string;
  role: "admin" | "user";
  createdAt: string;
}

export interface Session {
  userId: string;
  expiresAt: string;
}

export interface ApiError {
  code: string;
  message: string;
}

export type Result<T> = { ok: true; value: T } | { ok: false; error: ApiError };
