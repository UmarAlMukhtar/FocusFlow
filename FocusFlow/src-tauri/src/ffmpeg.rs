use std::{path::PathBuf, process::Command};
use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

/// One resolver for both native recording and project rendering. No downloads.
pub fn command(app: &AppHandle) -> Result<Command, String> {
    // Development can opt into an installed executable; release always uses its sidecar.
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("FOCUSFLOW_FFMPEG") {
            let path = PathBuf::from(path);
            if !path.is_absolute() || !path.is_file() { return Err("FOCUSFLOW_FFMPEG must name an existing absolute executable path".into()); }
            return Ok(Command::new(path));
        }
    }
    let command: Command = app.shell().sidecar("ffmpeg").map_err(|e| format!("FFmpeg sidecar unavailable: {e}"))?.into();
    if !std::path::Path::new(command.get_program()).is_file() {
        return Err("FFmpeg sidecar is missing for this platform. For development, set FOCUSFLOW_FFMPEG to the installed executable's absolute path.".into());
    }
    Ok(command)
}
