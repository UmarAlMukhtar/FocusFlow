// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod audio_recorder;
mod capture_guard;
pub fn dispatch_capture_guard(args: impl Iterator<Item = std::ffi::OsString>) -> Option<i32> {
    let args: Vec<_> = args.collect();
    #[cfg(windows)]
    if let Some(code)=recorder::dispatch_windows_capture(args.clone().into_iter()) {return Some(code);}
    capture_guard::dispatch(args.into_iter())
}
mod export;
mod recorder;
mod ffmpeg;
use tauri_plugin_opener::OpenerExt;

use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, LogicalSize, Manager, Size};
#[cfg(windows)]
use tauri::Emitter;
#[cfg(windows)]
use std::thread;
static GLOBAL_HOTKEYS_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Default)]
struct AppShutdown {
    started: std::sync::atomic::AtomicBool,
    complete: std::sync::atomic::AtomicBool,
}
#[derive(Debug, PartialEq, Eq)]
enum ExitAction { BeginCleanup, Wait, Allow }
impl AppShutdown {
    fn request(&self) -> ExitAction {
        use std::sync::atomic::Ordering;
        if self.complete.load(Ordering::Acquire) { ExitAction::Allow }
        else if !self.started.swap(true, Ordering::AcqRel) { ExitAction::BeginCleanup }
        else { ExitAction::Wait }
    }
    fn finish(&self) { self.complete.store(true, std::sync::atomic::Ordering::Release); }
}

#[cfg(target_os = "windows")]
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_R, VK_S,
    },
    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
};

const MIN_WINDOW_WIDTH: f64 = 1000.0;
const MIN_WINDOW_HEIGHT: f64 = 700.0;
#[cfg(windows)]
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
    open_path_in_explorer(&app, &recordings_dir)
}

#[tauri::command]
fn open_recording_session_folder(app: AppHandle, session_id: String) -> Result<(), String> {
    let state = app.state::<export::DesktopProjects>();
    let session = focusflow_core::paths::project_dir(&state.root, &session_id).map_err(|error| error.to_string())?;
    open_path_in_explorer(&app, &session)
}

#[tauri::command]
fn open_edited_video_folder(app: AppHandle, session_id: String, job_id: Option<String>) -> Result<(), String> {
    let state = app.state::<export::DesktopProjects>();
    let session = focusflow_core::paths::project_dir(&state.root, &session_id).map_err(|error| error.to_string())?;
    let output = match job_id {
        Some(id) => {
            let job = state.jobs.status(&session_id, &id).map_err(|error| error.to_string())?;
            export::completed_output(&session, &job).map_err(|error| error.to_string())?
        }
        None => focusflow_core::paths::asset_path(&session, "edited.mp4").map_err(|error| error.to_string())?,
    };
    let folder = output.parent().ok_or_else(|| "Edited output has no folder".to_string())?;
    open_path_in_explorer(&app, folder)
}

fn recordings_root_dir(app: &AppHandle) -> Result<PathBuf, String> {
    crate::recorder::recordings_root_dir(app).map_err(|error| error.to_string())
}

fn open_path_in_explorer(app: &AppHandle, path: &Path) -> Result<(), String> {
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| format!("Could not open folder: {e}"))
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

    GLOBAL_HOTKEYS_READY.store(start_registered && stop_registered, std::sync::atomic::Ordering::Relaxed);
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
            recorder::recording_status(app.clone(), state).await
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

        match recorder::start_recording(app.clone(), state, None, None, None).await {
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

#[cfg(windows)]
fn emit_recording_status_changed(app: &AppHandle) {
    if let Err(error) = app.emit(RECORDING_STATUS_CHANGED_EVENT, ()) {
        eprintln!("Could not emit recording status change event: {error}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .manage(recorder::RecorderState::new())
        .manage(AppShutdown::default())
        .setup(|app| {
            let root = recorder::recordings_root_dir(app.handle())?;
            fs::create_dir_all(&root)?;
            let root = root.canonicalize()?;
            let ffmpeg = export::ffmpeg_sidecar_command(app.handle())?;
            app.manage(export::DesktopProjects::new(&root, Path::new(ffmpeg.get_program()))?);
            enforce_main_window_min_size(app)?;
            register_global_hotkeys(&app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            export::open_project,
            export::get_project,
            export::apply_project_edits,
            export::restore_project_edits,
            export::render_project_preview,
            export::get_project_job,
            export::cancel_project_job,
            export::get_project_media,
            export::export_edited_mp4,
            export::get_export_job,
            export::cancel_export_job,
            open_recordings_folder,
            open_recording_session_folder,
            open_edited_video_folder,
            recorder::get_capture_capabilities,
            recorder::start_recording,
            recorder::stop_recording,
            recorder::recording_status,
            recorder::list_recordable_windows,
            recorder::list_recent_sessions,
            recorder::delete_recording_session,
            audio_recorder::list_audio_input_devices
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                match app.state::<AppShutdown>().request() {
                    ExitAction::Allow => {},
                    ExitAction::Wait => api.prevent_exit(),
                    ExitAction::BeginCleanup => {
                        api.prevent_exit();
                        if let Err(error) = app.state::<recorder::RecorderState>().request_shutdown() {
                            eprintln!("Recording shutdown: {error}");
                        }
                        let app = app.clone();
                        // Keep the event loop alive for startup's dispatched monitor/window queries.
                        tauri::async_runtime::spawn(async move {
                            recorder::shutdown(&app.state::<recorder::RecorderState>()).await;
                            let jobs_app = app.clone();
                            let jobs = tauri::async_runtime::spawn_blocking(move || {
                                jobs_app.state::<export::DesktopProjects>().jobs.shutdown()
                            }).await;
                            if let Err(error) = jobs { eprintln!("Export shutdown task failed: {error}"); }
                            else if let Ok(Err(error)) = jobs { eprintln!("Export shutdown failed: {error}"); }
                            app.state::<AppShutdown>().finish();
                            app.exit(code.unwrap_or(0));
                        });
                    }
                }
            }
        });
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    #[test]
    fn exit_is_deferred_once_until_cleanup_finishes() {
        let shutdown = AppShutdown::default();
        assert_eq!(shutdown.request(), ExitAction::BeginCleanup);
        assert_eq!(shutdown.request(), ExitAction::Wait);
        assert_eq!(shutdown.request(), ExitAction::Wait);
        shutdown.finish();
        assert_eq!(shutdown.request(), ExitAction::Allow);
        assert_eq!(shutdown.request(), ExitAction::Allow);
    }
}

// The native qualification target compiles this exact crate root and dependency graph.
#[cfg(all(test, windows))]
fn main() {
    if let Some(code) = dispatch_capture_guard(std::env::args_os().skip(1)) { std::process::exit(code); }
    if let Err(error) = recorder::run_windows_qualification() { eprintln!("{error}"); std::process::exit(1); }
}
#[cfg(all(test, not(windows)))]
fn main() { println!("Windows owned-product qualification: not applicable on this OS"); }
