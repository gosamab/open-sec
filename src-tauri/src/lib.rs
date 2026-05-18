mod commands;
pub mod config;
pub mod error;
pub mod export;
pub mod providers;
pub mod scanner;
pub mod store;
pub mod tools;

use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tracing_subscriber::EnvFilter;

fn fatal_dialog(app: &tauri::AppHandle, message: String) -> ! {
    tracing::error!("{message}");
    app.dialog()
        .message(message)
        .title("Open Security")
        .kind(MessageDialogKind::Error)
        .blocking_show();
    std::process::exit(1);
}

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
        .setup(|app| {
            let handle = app.handle().clone();
            let app_data = match app.path().app_data_dir() {
                Ok(p) => p,
                Err(e) => fatal_dialog(
                    &handle,
                    format!("Could not resolve the app data directory.\n\n{e:#}"),
                ),
            };
            std::fs::create_dir_all(&app_data).ok();
            config::init_key_path(&app_data);
            let db_path = app_data.join("open-sec.db");
            let store = match store::Store::open(&db_path) {
                Ok(s) => s,
                Err(e) => fatal_dialog(
                    &handle,
                    format!(
                        "Could not open the scan-history database at:\n{}\n\n{e:#}",
                        db_path.display()
                    ),
                ),
            };
            app.manage(store);
            app.manage(commands::CancelHandle::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::has_anthropic_key,
            commands::set_anthropic_key,
            commands::scan_file,
            commands::run_pipeline,
            commands::cancel_scan,
            commands::get_excerpt,
            commands::apply_patch,
            commands::regenerate_patch,
            commands::get_applied_for_root,
            commands::export_markdown,
            commands::export_sarif,
            commands::save_text_file,
            commands::list_scan_groups,
            commands::load_scan,
            commands::delete_scans_for_root,
            commands::set_triage,
            commands::clear_triage,
            commands::get_triage_for_root,
            commands::open_url,
            commands::open_in_editor,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
