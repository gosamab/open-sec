mod commands;
pub mod config;
pub mod error;
pub mod providers;
pub mod scanner;
pub mod tools;

use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Load .env if present (no-op otherwise); used as a fallback API-key source.
    let _ = dotenvy::dotenv();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,open_sec_lib=debug")),
        )
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::has_anthropic_key,
            commands::set_anthropic_key,
            commands::scan_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
