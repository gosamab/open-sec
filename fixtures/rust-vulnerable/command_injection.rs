// HTTP handler that builds a shell command from a query parameter.
// Vulnerable: CWE-78 (OS Command Injection).
//
// Equivalent to the existing Python/TS command_injection fixtures, but
// in Rust idiom — `Command::new("sh").arg("-c").arg(format!(...))`.

use std::process::Command;

use axum::{extract::Query, response::Json, routing::get, Router};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct PingParams {
    host: String,
}

#[derive(Serialize)]
struct PingResult {
    output: String,
}

async fn ping(Query(params): Query<PingParams>) -> Json<PingResult> {
    // Untrusted `host` from the HTTP query string is interpolated into a
    // shell-evaluated command. An attacker can submit `127.0.0.1; cat /etc/passwd`.
    let cmd = format!("ping -c 1 {}", params.host);
    let out = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .expect("ping failed");
    Json(PingResult {
        output: String::from_utf8_lossy(&out.stdout).into_owned(),
    })
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/ping", get(ping));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
