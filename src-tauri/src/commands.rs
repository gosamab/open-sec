use tracing::info;

#[tauri::command]
pub fn greet(name: String) -> Result<String, String> {
    info!(name = %name, "greet command invoked");
    Ok(format!("Hello, {name}! open-sec is alive."))
}
