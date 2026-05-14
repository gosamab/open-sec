// HTTP handler that builds a SQL query string from a path parameter via
// `format!`. The `rusqlite` driver will happily execute it.
// Vulnerable: CWE-89 (SQL Injection).

use axum::{extract::Path, response::Json, routing::get, Router};
use rusqlite::Connection;
use serde::Serialize;

#[derive(Serialize)]
struct User {
    id: i64,
    email: String,
}

async fn get_user(Path(id): Path<String>) -> Json<Option<User>> {
    let conn = Connection::open("app.db").expect("open db");

    // Builds a SQL string by interpolating an untrusted path segment.
    // `id` is attacker-controlled; payload `1 OR 1=1--` returns the first row
    // regardless of id, or worse via subqueries / UNION.
    let sql = format!("SELECT id, email FROM users WHERE id = {}", id);

    let user = conn
        .query_row(&sql, [], |row| {
            Ok(User {
                id: row.get(0)?,
                email: row.get(1)?,
            })
        })
        .ok();
    Json(user)
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/users/:id", get(get_user));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
