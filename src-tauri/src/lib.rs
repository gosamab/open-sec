mod commands;
pub mod config;
pub mod error;
pub mod providers;
pub mod scanner;
pub mod store;
pub mod tools;

use tauri::Manager;
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
        .setup(|app| {
            // Open the scan-history DB at <app_data_dir>/open-sec.db. We crash
            // here on failure because nothing downstream works without it.
            let app_data = app
                .path()
                .app_data_dir()
                .expect("resolving app_data_dir");
            std::fs::create_dir_all(&app_data).ok();
            let db_path = app_data.join("open-sec.db");
            let store = store::Store::open(&db_path)
                .unwrap_or_else(|e| panic!("opening store at {}: {e:#}", db_path.display()));
            app.manage(store);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::has_anthropic_key,
            commands::set_anthropic_key,
            commands::scan_file,
            commands::run_pipeline,
            commands::list_scan_groups,
            commands::load_scan,
            commands::delete_scan,
            commands::delete_scans_for_root,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
