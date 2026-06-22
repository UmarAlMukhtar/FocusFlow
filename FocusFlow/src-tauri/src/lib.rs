// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod export;
mod recorder;

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Size};

#[cfg(target_os = "windows")]
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_R, VK_S,
    },
    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
};

const SESSION_SCREEN_FILE_NAME: &str = "screen.mp4";
const MIN_WINDOW_WIDTH: f64 = 800.0;
const MIN_WINDOW_HEIGHT: f64 = 600.0;
const RECORDING_STATUS_CHANGED_EVENT: &str = "recording-status-changed";

#[cfg(target_os = "windows")]
const HOTKEY_START_RECORDING_ID: i32 = 0x4652;
#[cfg(target_os = "windows")]
const HOTKEY_STOP_RECORDING_ID: i32 = 0x4653;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn open_recordings_folder(app: AppHandle) -> Result<(), String> {
    let recordings_dir = recordings_root_dir(&app)?;
    open_path_in_explorer(&recordings_dir)
}

#[tauri::command]
fn open_recording_session_folder(
    app: AppHandle,
    folder_path: Option<String>,
) -> Result<(), String> {
    let session_dir = match folder_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        Some(path) => folder_inside_recordings_root(&app, path)?,
        None => latest_recording_session_dir(&app)?,
    };

    open_path_in_explorer(&session_dir)
}

#[tauri::command]
fn open_edited_video_folder(app: AppHandle, folder_path: String) -> Result<(), String> {
    let folder_dir = folder_inside_recordings_root(&app, &folder_path)?;

    open_path_in_explorer(&folder_dir)
}

fn recordings_root_dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::recorder::recordings_root_dir(app)
        .map_err(|error| format!("Could not resolve recordings directory: {error}"))
}

fn folder_inside_recordings_root(app: &AppHandle, folder_path: &str) -> Result<PathBuf, String> {
    let recordings_dir = canonicalize_existing_dir(&recordings_root_dir(app)?)?;
    let folder_dir = canonicalize_existing_dir(Path::new(folder_path))?;

    if !folder_dir.starts_with(&recordings_dir) {
        return Err(format!(
            "Folder is outside the FocusFlow recordings directory: {}",
            folder_dir.display()
        ));
    }

    Ok(folder_dir)
}

fn latest_recording_session_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let recordings_dir = recordings_root_dir(app)?;
    let mut candidates = Vec::new();

    for entry in fs::read_dir(&recordings_dir).map_err(|error| {
        format!(
            "Could not read recordings directory {}: {error}",
            recordings_dir.display()
        )
    })? {
        let entry = entry.map_err(|error| format!("Could not read recording entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("Could not read recording entry type: {error}"))?;

        if file_type.is_dir() && path.join(SESSION_SCREEN_FILE_NAME).exists() {
            candidates.push((modified_time(&path), path));
        }
    }

    candidates
        .into_iter()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
        .ok_or_else(|| {
            format!(
                "No recording session containing {SESSION_SCREEN_FILE_NAME} was found in {}",
                recordings_dir.display()
            )
        })
}

fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf, String> {
    let canonical_path = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve folder {}: {error}", path.display()))?;

    if !canonical_path.is_dir() {
        return Err(format!(
            "Path is not a folder: {}",
            canonical_path.display()
        ));
    }

    Ok(canonical_path)
}

fn modified_time(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

fn open_path_in_explorer(path: &Path) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Err("Opening folders is only supported on Windows".to_string());
    }

    Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            format!(
                "Could not open folder in File Explorer {}: {error}",
                path.display()
            )
        })
}

fn enforce_main_window_min_size(app: &mut tauri::App) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.set_min_size(Some(Size::Logical(LogicalSize::new(
            MIN_WINDOW_WIDTH,
            MIN_WINDOW_HEIGHT,
        ))))?;
    }

    Ok(())
}

fn register_global_hotkeys(app: &AppHandle) {
    #[cfg(target_os = "windows")]
    {
        let app = app.clone();

        if let Err(error) = thread::Builder::new()
            .name("focusflow-global-hotkeys".to_string())
            .spawn(move || run_windows_hotkey_loop(app))
        {
            eprintln!("Could not start FocusFlow global hotkey thread: {error}");
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
    }
}

#[cfg(target_os = "windows")]
fn run_windows_hotkey_loop(app: AppHandle) {
    let start_registered =
        register_windows_hotkey(HOTKEY_START_RECORDING_ID, "Ctrl+Shift+R", u32::from(VK_R.0));
    let stop_registered =
        register_windows_hotkey(HOTKEY_STOP_RECORDING_ID, "Ctrl+Shift+S", u32::from(VK_S.0));

    if !start_registered && !stop_registered {
        return;
    }

    let mut message = MSG::default();

    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };

        if result.0 <= 0 {
            break;
        }

        if message.message == WM_HOTKEY {
            match message.wParam.0 as i32 {
                HOTKEY_START_RECORDING_ID if start_registered => {
                    handle_start_recording_hotkey(&app);
                }
                HOTKEY_STOP_RECORDING_ID if stop_registered => {
                    handle_stop_recording_hotkey(&app);
                }
                _ => {}
            }
        }
    }

    if start_registered {
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_START_RECORDING_ID) };
    }

    if stop_registered {
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_STOP_RECORDING_ID) };
    }
}

#[cfg(target_os = "windows")]
fn register_windows_hotkey(id: i32, label: &str, virtual_key: u32) -> bool {
    let modifiers = MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;

    match unsafe { RegisterHotKey(None, id, modifiers, virtual_key) } {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Could not register FocusFlow hotkey {label}: {error}");
            false
        }
    }
}

#[cfg(target_os = "windows")]
fn handle_start_recording_hotkey(app: &AppHandle) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let status = {
            let state = app.state::<recorder::RecorderState>();
            recorder::recording_status(app.clone(), state)
        };

        match status {
            Ok(status)
                if matches!(
                    status.phase,
                    recorder::RecordingPhase::Recording | recorder::RecordingPhase::Finalizing
                ) =>
            {
                return;
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("FocusFlow start recording hotkey status check failed: {error}");
                return;
            }
        }

        minimize_main_window(&app);
        let state = app.state::<recorder::RecorderState>();

        match recorder::start_recording(app.clone(), state, None).await {
            Ok(_) => emit_recording_status_changed(&app),
            Err(error) => eprintln!("FocusFlow start recording hotkey failed: {error}"),
        }
    });
}

#[cfg(target_os = "windows")]
fn handle_stop_recording_hotkey(app: &AppHandle) {
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        let state = app.state::<recorder::RecorderState>();

        match recorder::stop_recording(state).await {
            Ok(_) => {
                restore_main_window(&app);
                emit_recording_status_changed(&app);
            }
            Err(error) => eprintln!("FocusFlow stop recording hotkey failed: {error}"),
        }
    });
}

#[cfg(target_os = "windows")]
fn minimize_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.minimize() {
            eprintln!("Could not minimize FocusFlow window for hotkey recording: {error}");
        }
    }
}

#[cfg(target_os = "windows")]
fn restore_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window.unminimize() {
            eprintln!("Could not restore FocusFlow window after hotkey recording: {error}");
        }

        if let Err(error) = window.set_focus() {
            eprintln!("Could not focus FocusFlow window after hotkey recording: {error}");
        }
    }
}

fn emit_recording_status_changed(app: &AppHandle) {
    if let Err(error) = app.emit(RECORDING_STATUS_CHANGED_EVENT, ()) {
        eprintln!("Could not emit recording status change event: {error}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(recorder::RecorderState::new())
        .setup(|app| {
            enforce_main_window_min_size(app)?;
            register_global_hotkeys(&app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            export::export_edited_mp4,
            open_recordings_folder,
            open_recording_session_folder,
            open_edited_video_folder,
            recorder::start_recording,
            recorder::stop_recording,
            recorder::recording_status,
            recorder::list_recordable_windows,
            recorder::list_recent_sessions,
            recorder::delete_recording_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
