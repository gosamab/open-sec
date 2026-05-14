"""User repository — wraps the raw SQL access for our users table."""

from typing import Optional
import psycopg2

_conn = None


def get_conn():
    global _conn
    if _conn is None:
        _conn = psycopg2.connect(host="db", dbname="app", user="app", password="x")
    return _conn


def find_by_email(email: str) -> Optional[dict]:
    cur = get_conn().cursor()
    cur.execute("SELECT id, email, password_hash, role FROM users WHERE email = %s", (email,))
    row = cur.fetchone()
    if row is None:
        return None
    return {"id": row[0], "email": row[1], "password_hash": row[2], "role": row[3]}


def search(query: str) -> list[dict]:
    cur = get_conn().cursor()
    # NB: builds the LIKE pattern; query is a user-controlled string.
    pattern = f"%{query}%"
    cur.execute("SELECT id, email FROM users WHERE email ILIKE %s LIMIT 50", (pattern,))
    return [{"id": r[0], "email": r[1]} for r in cur.fetchall()]
