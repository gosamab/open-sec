// HTTP handler that serves a file from a fixed docs root, joined with an
// untrusted query parameter. `PathBuf::join` does not strip `..`, so
// `?name=../../etc/passwd` escapes the root.
// Vulnerable: CWE-22 (Path Traversal).

use std::path::PathBuf;

use axum::{extract::Query, response::IntoResponse, routing::get, Router};
use serde::Deserialize;
use tokio::fs;

const DOCS_ROOT: &str = "/var/app/docs";

#[derive(Deserialize)]
struct DownloadParams {
    name: String,
}

async fn download(Query(params): Query<DownloadParams>) -> impl IntoResponse {
    // PathBuf::join discards the base if `name` is absolute, and does not
    // resolve `..` — both are exploit vectors here.
    let path = PathBuf::from(DOCS_ROOT).join(&params.name);
    match fs::read(&path).await {
        Ok(bytes) => (axum::http::StatusCode::OK, bytes),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
    }
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/download", get(download));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
