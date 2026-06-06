// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod export;
mod recorder;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(recorder::RecorderState::new())
        .invoke_handler(tauri::generate_handler![
            greet,
            export::export_edited_mp4,
            recorder::start_recording,
            recorder::stop_recording,
            recorder::recording_status
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
