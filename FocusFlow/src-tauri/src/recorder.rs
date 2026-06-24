use crate::audio_recorder::AudioRecorder;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{c_void, OsStr},
    fmt, fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_shell::ShellExt;
use windows_capture::{
    capture::{
        CaptureControl, CaptureControlError, Context, GraphicsCaptureApiError,
        GraphicsCaptureApiHandler,
    },
    encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    },
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window as WindowsCaptureWindow,
};

#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, TRUE},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON},
        WindowsAndMessaging::{
            EnumWindows, GetAncestor, GetCursorPos, GetShellWindow, GetWindow, GetWindowRect,
            GetWindowTextW, IsIconic, IsWindow, IsWindowVisible, WindowFromPoint, GA_ROOT,
            GW_OWNER,
        },
    },
};

const CLICKS_FILE_NAME: &str = "clicks.json";
const DRAGS_FILE_NAME: &str = "drags.json";
const EDITED_FILE_NAME: &str = "edited.mp4";
const FFMPEG_SIDECAR_NAME: &str = "ffmpeg";
const METADATA_FILE_NAME: &str = "metadata.json";
const MIC_AUDIO_FILE_NAME: &str = "mic.wav";
const OUTPUT_FILE_NAME: &str = "screen.mp4";
const OUTPUT_RECORDINGS_DIR_NAME: &str = "Recordings";
const SESSION_FILE_NAME: &str = "session.json";
const TIMELINE_FILE_NAME: &str = "timeline.json";
const CLICK_POLL_INTERVAL: Duration = Duration::from_millis(8);
const DRAG_POINT_INTERVAL: Duration = Duration::from_millis(50);
const DRAG_MIN_DISTANCE_PX: i32 = 4;
const DRAG_POINT_MIN_DISTANCE_PX: i32 = 2;
const DRAG_FORCE_SAMPLE_DISTANCE_PX: i32 = 12;
const PRE_CLICK_ZOOM_SECONDS: f64 = 0.3;
const IDLE_BEFORE_ZOOM_OUT_SECONDS: f64 = 1.0;
const ZOOM_OUT_SECONDS: f64 = 0.35;
const MIN_TIMELINE_SEGMENT_SECONDS: f64 = 0.001;
/// Two adjacent camera sequences are merged into one continuous sequence when
/// the gap between them is shorter than this value.  This avoids a zoom-out
/// followed immediately by a zoom-in when the user clicks or drags in quick
/// succession.
const TIMELINE_MERGE_WINDOW_SECONDS: f64 = 0.3;
const TARGET_FPS: u32 = 30;
const STOP_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_CAPTURED_LOG_BYTES: usize = 96 * 1024;

pub type RecorderResult<T> = Result<T, RecorderError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub phase: RecordingPhase,
    pub output_path: String,
    pub pid: Option<u32>,
    pub monitor: Option<MonitorInfo>,
    pub elapsed_ms: u64,
    pub file_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingPhase {
    Idle,
    Recording,
    Finalizing,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub name: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordableWindow {
    pub id: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RecordingSource {
    Screen,
    Window {
        hwnd: String,
        title: String,
    },
    Region {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

impl Default for RecordingSource {
    fn default() -> RecordingSource {
        RecordingSource::Screen
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct CapturePlan {
    source: RecordingSource,
    monitor: MonitorInfo,
    bounds: CaptureBounds,
    backend: CaptureBackend,
    ffmpeg_args: Vec<String>,
    window_capture: Option<WindowCapturePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CaptureBackend {
    Ffmpeg,
    WindowsCapture,
}

#[derive(Debug, Clone)]
struct WindowCapturePlan {
    hwnd: usize,
    title: String,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionMetadata {
    version: u32,
    source: RecordingSource,
    capture_bounds: CaptureBounds,
    capture_backend: CaptureBackend,
    /// Whether `mic.wav` was successfully recorded in this session.
    has_mic_audio: bool,
    /// Name of the microphone device used, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    mic_device_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionSummary {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    #[serde(alias = "createdAt")]
    pub created_at: String,
    #[serde(alias = "durationSeconds")]
    pub duration_seconds: f64,
    #[serde(alias = "recordingSource")]
    pub recording_source: String,
    pub exported: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentSession {
    pub session_id: String,
    pub created_at: String,
    pub duration_seconds: f64,
    pub recording_source: String,
    pub exported: bool,
    pub session_path: String,
    pub edited_video_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl RecorderError {
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        recoverable: bool,
    ) -> RecorderError {
        RecorderError {
            code: code.into(),
            message: message.into(),
            recoverable,
        }
    }
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RecorderError {}

#[derive(Default)]
pub struct RecorderState {
    runtime: Mutex<RecorderRuntime>,
}

impl RecorderState {
    pub fn new() -> RecorderState {
        RecorderState::default()
    }

    fn lock_runtime(&self) -> RecorderResult<MutexGuard<'_, RecorderRuntime>> {
        self.runtime.lock().map_err(|_| {
            RecorderError::new(
                "recorder_state_poisoned",
                "Recorder state lock was poisoned by a previous panic",
                true,
            )
        })
    }

    fn clear_finalizing(&self) -> RecorderResult<()> {
        let mut runtime = self.lock_runtime()?;
        runtime.finalizing = false;
        runtime.finalizing_output_path = None;
        Ok(())
    }
}

#[derive(Default)]
struct RecorderRuntime {
    active: Option<ActiveRecording>,
    finalizing: bool,
    finalizing_output_path: Option<PathBuf>,
    last_completed: Option<CompletedRecording>,
}

struct ActiveRecording {
    capture: ActiveCapture,
    /// Running microphone recorder, or `None` when mic was not enabled.
    audio_recorder: Option<AudioRecorder>,
    /// Human-readable name of the mic device that was selected.
    mic_device_name: Option<String>,
    click_tracker: ClickTracker,
    output_path: PathBuf,
    started_at: Instant,
    monitor: MonitorInfo,
    capture_bounds: CaptureBounds,
    source: RecordingSource,
    log: Arc<Mutex<ProcessLog>>,
}

enum ActiveCapture {
    Ffmpeg {
        child: Child,
    },
    WindowsCapture {
        capture: CaptureControl<WindowsWindowCapture, RecorderError>,
    },
}

impl ActiveCapture {
    fn pid(&self) -> Option<u32> {
        match self {
            ActiveCapture::Ffmpeg { child } => Some(child.id()),
            ActiveCapture::WindowsCapture { .. } => None,
        }
    }

    fn backend(&self) -> CaptureBackend {
        match self {
            ActiveCapture::Ffmpeg { .. } => CaptureBackend::Ffmpeg,
            ActiveCapture::WindowsCapture { .. } => CaptureBackend::WindowsCapture,
        }
    }
}

struct WindowsCaptureFlags {
    output_path: PathBuf,
    hwnd: usize,
    title: String,
    width: u32,
    height: u32,
}

struct WindowsWindowCapture {
    encoder: Option<VideoEncoder>,
    hwnd: usize,
    title: String,
    encoder_width: u32,
    encoder_height: u32,
    last_frame_size: Option<(u32, u32)>,
    frame_count: u64,
}

impl WindowsWindowCapture {
    fn finish_encoder(&mut self) -> RecorderResult<()> {
        if let Some(encoder) = self.encoder.take() {
            encoder.finish().map_err(|error| {
                RecorderError::new(
                    "windows_capture_finish_failed",
                    format!("Could not finalize window recording: {error}"),
                    true,
                )
            })?;
        }

        println!(
            "[FocusFlow recorder] windows-capture frames encoded: {}",
            self.frame_count
        );

        Ok(())
    }

    fn log_frame_size_change(&mut self, frame: &Frame) {
        let frame_size = (frame.width(), frame.height());

        if self.last_frame_size == Some(frame_size) {
            return;
        }

        if let Some(previous_size) = self.last_frame_size {
            println!(
                "[FocusFlow recorder] Warning: window capture frame size changed from {}x{} to {}x{} for HWND 0x{:X} ({})",
                previous_size.0,
                previous_size.1,
                frame_size.0,
                frame_size.1,
                self.hwnd,
                self.title
            );
            println!(
                "[FocusFlow recorder] Encoder remains locked to {}x{}; resize support requires segmented recording.",
                self.encoder_width, self.encoder_height
            );
            // TODO: Restart capture into a new segment on resize and merge segments after stop.
        } else {
            println!(
                "[FocusFlow recorder] First window capture frame size: {}x{}; encoder dimensions: {}x{}",
                frame_size.0, frame_size.1, self.encoder_width, self.encoder_height
            );
        }

        self.last_frame_size = Some(frame_size);
    }
}

impl GraphicsCaptureApiHandler for WindowsWindowCapture {
    type Flags = WindowsCaptureFlags;
    type Error = RecorderError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        println!(
            "[FocusFlow recorder] Starting windows-capture encoder for HWND 0x{:X} ({}) at {}x{}",
            ctx.flags.hwnd, ctx.flags.title, ctx.flags.width, ctx.flags.height
        );

        let video_settings = VideoSettingsBuilder::new(ctx.flags.width, ctx.flags.height)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(TARGET_FPS)
            .bitrate(12_000_000);

        let encoder = VideoEncoder::new(
            video_settings,
            AudioSettingsBuilder::default().disabled(true),
            ContainerSettingsBuilder::default(),
            &ctx.flags.output_path,
        )
        .map_err(|error| {
            RecorderError::new(
                "windows_capture_encoder_failed",
                format!("Could not initialize window recording encoder: {error}"),
                true,
            )
        })?;

        Ok(Self {
            encoder: Some(encoder),
            hwnd: ctx.flags.hwnd,
            title: ctx.flags.title,
            encoder_width: ctx.flags.width,
            encoder_height: ctx.flags.height,
            last_frame_size: None,
            frame_count: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        self.log_frame_size_change(frame);

        if let Some(encoder) = self.encoder.as_mut() {
            encoder.send_frame(frame).map_err(|error| {
                RecorderError::new(
                    "windows_capture_frame_failed",
                    format!("Could not encode window recording frame: {error}"),
                    true,
                )
            })?;
            self.frame_count = self.frame_count.saturating_add(1);
        }

        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.finish_encoder()?;
        Err(RecorderError::new(
            "window_capture_closed",
            "Selected window closed during recording",
            true,
        ))
    }
}

#[derive(Debug, Clone)]
struct CompletedRecording {
    output_path: PathBuf,
    monitor: MonitorInfo,
    duration_ms: u64,
    file_size_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct ProcessLog {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProcessLog {
    fn push_stdout(&mut self, bytes: Vec<u8>) {
        push_bounded(&mut self.stdout, bytes);
    }

    fn push_stderr(&mut self, bytes: Vec<u8>) {
        push_bounded(&mut self.stderr, bytes);
    }

    fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_string()
    }
}

#[derive(Debug, Clone)]
struct ProcessExit {
    code: Option<i32>,
    event_error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ClickEvent {
    timestamp: f64,
    x: i32,
    y: i32,
    button: ClickButton,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DragInteraction {
    start: f64,
    end: f64,
    button: ClickButton,
    points: Vec<DragPoint>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct DragPoint {
    timestamp: f64,
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ClickButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Serialize)]
struct TimelineSegment {
    start: f64,
    end: f64,
    x: i32,
    y: i32,
    scale: f64,
}

struct ClickTracker {
    stop_requested: Arc<AtomicBool>,
    clicks: Arc<Mutex<Vec<ClickEvent>>>,
    drags: Arc<Mutex<Vec<DragInteraction>>>,
    join_handle: Option<JoinHandle<()>>,
}

struct InteractionEvents {
    clicks: Vec<ClickEvent>,
    drags: Vec<DragInteraction>,
}

struct PendingDrag {
    button: ClickButton,
    points: Vec<DragPoint>,
    is_drag: bool,
}

/// Determines which clicks the tracker records and how coordinates are stored.
///
/// - `All`: every click is accepted; raw screen coordinates are stored and
///   converted to capture-relative after the recording stops (Screen / Region).
/// - `Window`: only clicks whose root HWND matches the recorded window are
///   accepted; coordinates are converted to window-relative at click time so
///   the after-the-fact normalization step can be skipped.
#[derive(Clone, Copy)]
enum ClickFilter {
    All,
    Window {
        hwnd: usize,
        encoder_width: u32,
        encoder_height: u32,
    },
}

impl ClickTracker {
    fn start(started_at: Instant, filter: ClickFilter) -> ClickTracker {
        let stop_requested = Arc::new(AtomicBool::new(false));
        let clicks = Arc::new(Mutex::new(Vec::new()));
        let drags = Arc::new(Mutex::new(Vec::new()));
        let worker_stop_requested = Arc::clone(&stop_requested);
        let worker_clicks = Arc::clone(&clicks);
        let worker_drags = Arc::clone(&drags);

        let join_handle = thread::spawn(move || {
            let mut left_was_down = left_mouse_button_down();
            let mut right_was_down = right_mouse_button_down();
            let mut left_drag: Option<PendingDrag> = None;
            let mut right_drag: Option<PendingDrag> = None;

            while !worker_stop_requested.load(Ordering::Relaxed) {
                let left_is_down = left_mouse_button_down();
                let right_is_down = right_mouse_button_down();

                if left_is_down && !left_was_down {
                    left_drag = push_click(&worker_clicks, started_at, ClickButton::Left, &filter)
                        .map(|point| PendingDrag::new(ClickButton::Left, point));
                } else if left_is_down {
                    update_pending_drag(&mut left_drag, started_at);
                } else if left_was_down {
                    finish_pending_drag(&mut left_drag, &worker_drags, started_at);
                }

                if right_is_down && !right_was_down {
                    right_drag = push_click(&worker_clicks, started_at, ClickButton::Right, &filter)
                        .map(|point| PendingDrag::new(ClickButton::Right, point));
                } else if right_is_down {
                    update_pending_drag(&mut right_drag, started_at);
                } else if right_was_down {
                    finish_pending_drag(&mut right_drag, &worker_drags, started_at);
                }

                left_was_down = left_is_down;
                right_was_down = right_is_down;
                thread::sleep(CLICK_POLL_INTERVAL);
            }

            finish_pending_drag(&mut left_drag, &worker_drags, started_at);
            finish_pending_drag(&mut right_drag, &worker_drags, started_at);
        });

        ClickTracker {
            stop_requested,
            clicks,
            drags,
            join_handle: Some(join_handle),
        }
    }

    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Relaxed);
    }

    fn finish(mut self) -> RecorderResult<InteractionEvents> {
        self.request_stop();

        if let Some(join_handle) = self.join_handle.take() {
            join_handle.join().map_err(|_| {
                RecorderError::new(
                    "click_tracker_failed",
                    "Click tracker thread panicked",
                    true,
                )
            })?;
        }

        let clicks = self
            .clicks
            .lock()
            .map(|clicks| clicks.clone())
            .map_err(|_| {
                RecorderError::new(
                    "click_tracker_state_poisoned",
                    "Click tracker state lock was poisoned by a previous panic",
                    true,
                )
            })?;
        let drags = self.drags.lock().map(|drags| drags.clone()).map_err(|_| {
            RecorderError::new(
                "drag_tracker_state_poisoned",
                "Drag tracker state lock was poisoned by a previous panic",
                true,
            )
        })?;

        Ok(InteractionEvents { clicks, drags })
    }
}

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, RecorderState>,
    source: Option<RecordingSource>,
    // Microphone device ID (WASAPI device name).  Pass `None` or omit to
    // record without microphone audio.
    audio_device_id: Option<String>,
) -> RecorderResult<RecordingStatus> {
    start_recording_with_source(&app, state.inner(), source.unwrap_or_default(), audio_device_id).await
}

#[tauri::command]
pub async fn stop_recording(state: State<'_, RecorderState>) -> RecorderResult<RecordingStatus> {
    stop_active_recording(state.inner()).await
}

#[tauri::command]
pub fn recording_status(
    app: AppHandle,
    state: State<'_, RecorderState>,
) -> RecorderResult<RecordingStatus> {
    current_recording_status(&app, state.inner())
}

#[tauri::command]
pub fn list_recordable_windows() -> RecorderResult<Vec<RecordableWindow>> {
    ensure_windows()?;
    enumerate_recordable_windows()
}

#[tauri::command]
pub fn list_recent_sessions(app: AppHandle) -> RecorderResult<Vec<RecentSession>> {
    let recordings_dir = recordings_root_dir(&app)?;
    let mut sessions = Vec::new();

    for entry in fs::read_dir(&recordings_dir).map_err(|error| {
        RecorderError::new(
            "read_recordings_dir_failed",
            format!("Could not read recordings directory: {error}"),
            true,
        )
    })? {
        let entry = entry.map_err(|error| {
            RecorderError::new(
                "read_recording_entry_failed",
                format!("Could not read recording directory entry: {error}"),
                true,
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            RecorderError::new(
                "read_recording_entry_type_failed",
                format!("Could not read recording entry type: {error}"),
                true,
            )
        })?;

        if !file_type.is_dir() {
            continue;
        }

        if let Some(session) = recent_session_from_dir(&entry.path())? {
            sessions.push(session);
        }
    }

    sessions.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(sessions)
}

#[tauri::command]
pub fn delete_recording_session(app: AppHandle, session_id: String) -> RecorderResult<()> {
    let recordings_dir = recordings_root_dir(&app)?;
    let session_dir = session_dir_from_id(&recordings_dir, &session_id)?;

    fs::remove_dir_all(&session_dir).map_err(|error| {
        RecorderError::new(
            "delete_session_failed",
            format!("Could not delete recording session: {error}"),
            true,
        )
    })
}

async fn start_recording_with_source(
    app: &AppHandle,
    state: &RecorderState,
    source: RecordingSource,
    audio_device_id: Option<String>,
) -> RecorderResult<RecordingStatus> {
    ensure_windows()?;

    {
        let runtime = state.lock_runtime()?;
        if runtime.finalizing {
            return Err(RecorderError::new(
                "recorder_finalizing",
                "A recording is still finalizing",
                true,
            ));
        }

        if runtime.active.is_some() {
            return Err(RecorderError::new(
                "recorder_already_running",
                "A screen recording is already running",
                true,
            ));
        }
    }

    let output_path = create_session_screen_output_path(app)?;
    let output_dir = output_path.parent().ok_or_else(|| {
        RecorderError::new(
            "invalid_output_path",
            "Could not resolve a parent directory for screen.mp4",
            true,
        )
    })?;
    fs::create_dir_all(output_dir).map_err(|error| {
        RecorderError::new(
            "create_output_dir_failed",
            format!("Could not create recording output directory: {error}"),
            true,
        )
    })?;

    let capture_plan = capture_plan_for_source(app, source, &output_path)?;

    // ── Microphone recording ───────────────────────────────────────────────
    //
    // Start audio *before* writing metadata so that a failure here prevents
    // an orphaned session folder with no video.
    let mic_device_id_ref = audio_device_id
        .as_deref()
        .filter(|s| !s.is_empty());

    let (audio_recorder, mic_device_name) = match mic_device_id_ref {
        Some(device_name) => {
            let mic_path = output_dir.join(MIC_AUDIO_FILE_NAME);
            println!(
                "[FocusFlow audio] Starting microphone recording: {device_name}"
            );
            println!(
                "[FocusFlow audio] Writing mic.wav: {}",
                mic_path.display()
            );

            match AudioRecorder::start(Some(device_name), &mic_path) {
                Ok(recorder) => (Some(recorder), Some(device_name.to_string())),
                Err(audio_error) => {
                    // Audio start failure is fatal: return a clear error so
                    // the user knows mic recording did not work before video
                    // starts (avoids a recording with no audio despite asking).
                    if let Err(cleanup_error) = fs::remove_dir_all(output_dir) {
                        eprintln!(
                            "[FocusFlow audio] Could not clean up session dir after audio start failure: {cleanup_error}"
                        );
                    }
                    return Err(RecorderError::new(
                        "mic_start_failed",
                        format!(
                            "Could not start microphone recording ({}): {}",
                            audio_error.code, audio_error.message
                        ),
                        true,
                    ));
                }
            }
        }
        None => (None, None),
    };
    // ── End microphone recording setup ────────────────────────────────────

    write_session_metadata(
        &output_path,
        &capture_plan.source,
        capture_plan.bounds,
        capture_plan.backend,
        false, // has_mic_audio is set to true on successful stop
        mic_device_name.clone(),
    )?;
    write_session_json(&output_path, &capture_plan.source, 0.0, false)?;
    let monitor = capture_plan.monitor.clone();
    let capture_bounds = capture_plan.bounds;
    let log = Arc::new(Mutex::new(ProcessLog::default()));
    let capture = match start_capture_backend(
        app,
        output_dir,
        &output_path,
        &capture_plan,
        Arc::clone(&log),
    ) {
        Ok(capture) => capture,
        Err(error) => {
            // Video capture failed after audio was already started.
            // Stop the audio recorder (best-effort) before returning the error.
            if let Some(recorder) = audio_recorder {
                if let Err(audio_error) = recorder.stop() {
                    eprintln!(
                        "[FocusFlow audio] Failed to stop mic recorder after video start failure: {audio_error}"
                    );
                }
            }
            if let Err(cleanup_error) = fs::remove_dir_all(output_dir) {
                eprintln!(
                    "Could not remove failed recording session directory {}: {cleanup_error}",
                    output_dir.display()
                );
            }

            return Err(error);
        }
    };
    let pid = capture.pid();

    let started_at = Instant::now();
    let click_filter = click_filter_for_source(&capture_plan);
    let click_tracker = ClickTracker::start(started_at, click_filter);

    let active = ActiveRecording {
        capture,
        audio_recorder,
        mic_device_name,
        click_tracker,
        output_path: output_path.clone(),
        started_at,
        monitor: monitor.clone(),
        capture_bounds,
        source: capture_plan.source.clone(),
        log,
    };

    let mut runtime = state.lock_runtime()?;
    runtime.last_completed = None;
    runtime.active = Some(active);

    Ok(RecordingStatus {
        phase: RecordingPhase::Recording,
        output_path: path_to_string(&output_path)?,
        pid,
        monitor: Some(monitor),
        elapsed_ms: 0,
        file_size_bytes: None,
    })
}

fn start_capture_backend(
    app: &AppHandle,
    output_dir: &Path,
    output_path: &Path,
    capture_plan: &CapturePlan,
    log: Arc<Mutex<ProcessLog>>,
) -> RecorderResult<ActiveCapture> {
    println!(
        "[FocusFlow recorder] Capture backend: {:?}",
        capture_plan.backend
    );
    println!(
        "[FocusFlow recorder] Output path: {}",
        output_path.display()
    );

    match capture_plan.backend {
        CaptureBackend::Ffmpeg => {
            start_ffmpeg_capture(app, output_dir, &capture_plan.ffmpeg_args, Arc::clone(&log))
        }
        CaptureBackend::WindowsCapture => {
            let window_capture = capture_plan.window_capture.as_ref().ok_or_else(|| {
                RecorderError::new(
                    "window_capture_plan_missing",
                    "Window capture plan was not prepared",
                    true,
                )
            })?;

            start_windows_capture(window_capture, output_path)
        }
    }
}

fn start_ffmpeg_capture(
    app: &AppHandle,
    output_dir: &Path,
    args: &[String],
    log: Arc<Mutex<ProcessLog>>,
) -> RecorderResult<ActiveCapture> {
    let mut command = ffmpeg_sidecar_command(app)?;
    let ffmpeg_path = command.get_program().to_os_string();
    log_recording_diagnostics(output_dir, &ffmpeg_path);
    log_ffmpeg_command(&ffmpeg_path, args);
    command
        .args(args)
        .current_dir(output_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        RecorderError::new(
            "ffmpeg_spawn_failed",
            format!("Could not start bundled FFmpeg recorder: {error}"),
            true,
        )
    })?;

    if let Some(stdout) = child.stdout.take() {
        spawn_output_reader(stdout, Arc::clone(&log), ProcessStream::Stdout);
    }

    if let Some(stderr) = child.stderr.take() {
        spawn_output_reader(stderr, Arc::clone(&log), ProcessStream::Stderr);
    }

    Ok(ActiveCapture::Ffmpeg { child })
}

fn start_windows_capture(
    plan: &WindowCapturePlan,
    output_path: &Path,
) -> RecorderResult<ActiveCapture> {
    println!("[FocusFlow recorder] Selected HWND: 0x{:X}", plan.hwnd);
    println!("[FocusFlow recorder] Window title: {}", plan.title);

    let window = WindowsCaptureWindow::from_raw_hwnd(plan.hwnd as *mut c_void);
    if !window.is_valid() {
        return Err(RecorderError::new(
            "window_capture_unavailable",
            "Selected window is not available to Windows Graphics Capture",
            true,
        ));
    }

    let settings = Settings::new(
        window,
        CursorCaptureSettings::Default,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        WindowsCaptureFlags {
            output_path: output_path.to_path_buf(),
            hwnd: plan.hwnd,
            title: plan.title.clone(),
            width: plan.width,
            height: plan.height,
        },
    );

    let capture = WindowsWindowCapture::start_free_threaded(settings).map_err(|error| {
        RecorderError::new(
            "window_capture_start_failed",
            format!("Could not start window recording: {error}"),
            true,
        )
    })?;

    Ok(ActiveCapture::WindowsCapture { capture })
}

async fn stop_active_recording(state: &RecorderState) -> RecorderResult<RecordingStatus> {
    let active = {
        let mut runtime = state.lock_runtime()?;

        if runtime.finalizing {
            return Err(RecorderError::new(
                "recorder_finalizing",
                "A recording is already finalizing",
                true,
            ));
        }

        let Some(active) = runtime.active.take() else {
            return Err(RecorderError::new(
                "recorder_not_running",
                "No active screen recording is running",
                true,
            ));
        };

        runtime.finalizing = true;
        runtime.finalizing_output_path = Some(active.output_path.clone());
        active
    };

    let ActiveRecording {
        capture,
        audio_recorder,
        mic_device_name,
        click_tracker,
        output_path,
        started_at,
        monitor,
        capture_bounds,
        source,
        log,
        ..
    } = active;

    click_tracker.request_stop();

    let backend = capture.backend();
    let capture_stop = stop_capture_backend(capture, &log).await;
    let interactions = click_tracker.finish();

    // ── Stop microphone recording ─────────────────────────────────────────
    //
    // Stop audio after video so we don't hold the WASAPI stream open while
    // the video finalizes.  Audio stop failure is non-fatal: the video
    // session is still saved and `has_mic_audio` is set to false in metadata.
    let has_mic_audio = stop_audio_recorder(audio_recorder, &output_path);
    // ── End microphone stop ───────────────────────────────────────────────

    if let Err(error) = capture_stop {
        state.clear_finalizing()?;
        return Err(error);
    }

    let interactions = match interactions {
        Ok(interactions) => interactions,
        Err(error) => {
            state.clear_finalizing()?;
            return Err(error);
        }
    };
    // For Window recordings, coordinates are already window-relative and bounds-checked
    // at click time, so the after-the-fact normalization must be skipped to avoid
    // double-subtracting the window origin.
    let interactions = match &source {
        RecordingSource::Window { .. } => interactions,
        _ => normalize_interactions_to_capture(interactions, capture_bounds),
    };

    let metadata = match fs::metadata(&output_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            state.clear_finalizing()?;
            return Err(RecorderError::new(
                "recording_output_missing",
                format!("Recording finished, but screen.mp4 was not readable: {error}"),
                true,
            ));
        }
    };

    if metadata.len() == 0 {
        state.clear_finalizing()?;
        return Err(RecorderError::new(
            "recording_output_empty",
            "Recording finished, but screen.mp4 is empty",
            true,
        ));
    }

    if let Err(error) = write_clicks_json(&output_path, &interactions.clicks) {
        state.clear_finalizing()?;
        return Err(error);
    }

    if let Err(error) = write_drags_json(&output_path, &interactions.drags) {
        state.clear_finalizing()?;
        return Err(error);
    }

    if let Err(error) = write_timeline_json(&output_path) {
        state.clear_finalizing()?;
        return Err(error);
    }

    if let Err(error) = write_session_json(
        &output_path,
        &source,
        elapsed_seconds_from_millis(elapsed_ms(started_at)),
        false,
    ) {
        state.clear_finalizing()?;
        return Err(error);
    }

    if let Err(error) = write_session_metadata(
        &output_path,
        &source,
        capture_bounds,
        backend,
        has_mic_audio,
        mic_device_name.clone(),
    ) {
        state.clear_finalizing()?;
        return Err(error);
    }

    let completed = CompletedRecording {
        output_path: output_path.clone(),
        monitor: monitor.clone(),
        duration_ms: elapsed_ms(started_at),
        file_size_bytes: metadata.len(),
    };

    {
        let mut runtime = state.lock_runtime()?;
        runtime.finalizing = false;
        runtime.finalizing_output_path = None;
        runtime.last_completed = Some(completed.clone());
    }

    Ok(RecordingStatus {
        phase: RecordingPhase::Completed,
        output_path: path_to_string(&completed.output_path)?,
        pid: None,
        monitor: Some(completed.monitor),
        elapsed_ms: completed.duration_ms,
        file_size_bytes: Some(completed.file_size_bytes),
    })
}

fn stop_audio_recorder(audio_recorder: Option<AudioRecorder>, output_path: &Path) -> bool {
    let Some(recorder) = audio_recorder else {
        return false;
    };

    println!("[FocusFlow audio] Stopped microphone recording");

    // Stop the audio recorder and finalize the WAV file.
    let stop_result = recorder.stop();

    // Check if the WAV file exists and is non-empty.
    let mic_path = match output_path.parent() {
        Some(parent) => parent.join(MIC_AUDIO_FILE_NAME),
        None => {
            eprintln!("[FocusFlow audio] Could not resolve session folder to check mic.wav");
            return false;
        }
    };

    if let Err(error) = stop_result {
        eprintln!("[FocusFlow audio] Failed to stop/finalize mic recording: {}", error);
        return false;
    }

    match fs::metadata(&mic_path) {
        Ok(metadata) => {
            let size = metadata.len();
            if size > 0 {
                println!("[FocusFlow audio] mic.wav size: {}", size);
                true
            } else {
                eprintln!("[FocusFlow audio] mic.wav is empty");
                false
            }
        }
        Err(error) => {
            eprintln!("[FocusFlow audio] mic.wav does not exist or is unreadable: {}", error);
            false
        }
    }
}

async fn stop_capture_backend(
    capture: ActiveCapture,
    log: &Arc<Mutex<ProcessLog>>,
) -> RecorderResult<()> {
    match capture {
        ActiveCapture::Ffmpeg { child } => stop_ffmpeg_capture(child, log).await,
        ActiveCapture::WindowsCapture { capture } => stop_windows_capture(capture).await,
    }
}

async fn stop_ffmpeg_capture(mut child: Child, log: &Arc<Mutex<ProcessLog>>) -> RecorderResult<()> {
    let stop_signal_error = child
        .stdin
        .as_mut()
        .ok_or_else(|| "FFmpeg stdin was not piped".to_string())
        .and_then(|stdin| {
            stdin
                .write_all(b"q\n")
                .and_then(|_| stdin.flush())
                .map_err(|error| error.to_string())
        })
        .err();
    let exit = wait_for_ffmpeg_exit(child).await;

    match exit {
        Ok(process_exit) if process_exit.code == Some(0) => Ok(()),
        Ok(process_exit) => Err(ffmpeg_exit_error(
            process_exit,
            stop_signal_error,
            &read_log_text(log),
        )),
        Err(wait_error) => Err(stop_timeout_error(wait_error, None, &read_log_text(log))),
    }
}

async fn stop_windows_capture(
    capture: CaptureControl<WindowsWindowCapture, RecorderError>,
) -> RecorderResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let callback = capture.callback();
        let stop_result = capture.stop().map_err(windows_capture_stop_error);
        let finish_result = callback.lock().finish_encoder();

        match (stop_result, finish_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(stop_error), Err(finish_error)) => Err(RecorderError::new(
                stop_error.code,
                format!(
                    "{}; additionally failed to finalize window recording: {}",
                    stop_error.message, finish_error.message
                ),
                true,
            )),
        }
    })
    .await
    .map_err(|error| {
        RecorderError::new(
            "window_capture_wait_failed",
            format!("Could not join window recording stop task: {error}"),
            true,
        )
    })?
}

fn windows_capture_stop_error(error: CaptureControlError<RecorderError>) -> RecorderError {
    match error {
        CaptureControlError::StoppedHandlerError(error)
        | CaptureControlError::GraphicsCaptureApiError(
            GraphicsCaptureApiError::FrameHandlerError(error),
        )
        | CaptureControlError::GraphicsCaptureApiError(GraphicsCaptureApiError::NewHandlerError(
            error,
        )) => error,
        other => RecorderError::new(
            "window_capture_stop_failed",
            format!("Could not stop window recording: {other}"),
            true,
        ),
    }
}

fn current_recording_status(
    _app: &AppHandle,
    state: &RecorderState,
) -> RecorderResult<RecordingStatus> {
    let runtime = state.lock_runtime()?;

    if runtime.finalizing {
        return Ok(RecordingStatus {
            phase: RecordingPhase::Finalizing,
            output_path: runtime
                .finalizing_output_path
                .as_deref()
                .map(path_to_string)
                .transpose()?
                .unwrap_or_default(),
            pid: None,
            monitor: None,
            elapsed_ms: 0,
            file_size_bytes: None,
        });
    }

    if let Some(active) = &runtime.active {
        return Ok(RecordingStatus {
            phase: RecordingPhase::Recording,
            output_path: path_to_string(&active.output_path)?,
            pid: active.capture.pid(),
            monitor: Some(active.monitor.clone()),
            elapsed_ms: elapsed_ms(active.started_at),
            file_size_bytes: fs::metadata(&active.output_path)
                .ok()
                .map(|metadata| metadata.len()),
        });
    }

    if let Some(completed) = &runtime.last_completed {
        return Ok(RecordingStatus {
            phase: RecordingPhase::Completed,
            output_path: path_to_string(&completed.output_path)?,
            pid: None,
            monitor: Some(completed.monitor.clone()),
            elapsed_ms: completed.duration_ms,
            file_size_bytes: Some(completed.file_size_bytes),
        });
    }

    Ok(RecordingStatus {
        phase: RecordingPhase::Idle,
        output_path: String::new(),
        pid: None,
        monitor: None,
        elapsed_ms: 0,
        file_size_bytes: None,
    })
}

async fn wait_for_ffmpeg_exit(mut child: Child) -> RecorderResult<ProcessExit> {
    tauri::async_runtime::spawn_blocking(move || {
        let started_waiting = Instant::now();

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Ok(ProcessExit {
                        code: status.code(),
                        event_error: None,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    return Ok(ProcessExit {
                        code: None,
                        event_error: Some(format!("Could not query FFmpeg status: {error}")),
                    });
                }
            }

            if started_waiting.elapsed() >= STOP_TIMEOUT {
                let kill_error = child.kill().err().map(|error| error.to_string());
                let wait_error = child.wait().err().map(|error| error.to_string());
                let mut message = format!(
                    "FFmpeg did not finish within {} seconds after stop",
                    STOP_TIMEOUT.as_secs()
                );

                if let Some(error) = kill_error {
                    message.push_str("; additionally failed to kill FFmpeg: ");
                    message.push_str(&error);
                }

                if let Some(error) = wait_error {
                    message.push_str("; additionally failed to wait after kill: ");
                    message.push_str(&error);
                }

                return Err(RecorderError::new("ffmpeg_stop_timeout", message, true));
            }

            thread::sleep(Duration::from_millis(100));
        }
    })
    .await
    .map_err(|error| {
        RecorderError::new(
            "ffmpeg_wait_failed",
            format!("Could not join FFmpeg wait task: {error}"),
            true,
        )
    })?
}

#[derive(Debug, Clone, Copy)]
enum ProcessStream {
    Stdout,
    Stderr,
}

fn spawn_output_reader<R>(mut reader: R, log: Arc<Mutex<ProcessLog>>, stream: ProcessStream)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    let bytes = buffer[..bytes_read].to_vec();
                    match stream {
                        ProcessStream::Stdout => append_stdout(&log, bytes),
                        ProcessStream::Stderr => append_stderr(&log, bytes),
                    }
                }
                Err(error) => {
                    append_stderr(
                        &log,
                        format!("Failed to read FFmpeg {stream:?}: {error}").into_bytes(),
                    );
                    break;
                }
            }
        }
    });
}

impl PendingDrag {
    fn new(button: ClickButton, first_point: DragPoint) -> PendingDrag {
        PendingDrag {
            button,
            points: vec![first_point],
            is_drag: false,
        }
    }

    fn update(&mut self, point: DragPoint) {
        let Some(first_point) = self.points.first() else {
            self.points.push(point);
            return;
        };

        if distance_squared(first_point, &point) >= squared_distance_threshold(DRAG_MIN_DISTANCE_PX)
        {
            self.is_drag = true;
        }

        let Some(last_point) = self.points.last() else {
            self.points.push(point);
            return;
        };
        let point_distance = distance_squared(last_point, &point);
        let elapsed = point.timestamp - last_point.timestamp;

        if point_distance >= squared_distance_threshold(DRAG_POINT_MIN_DISTANCE_PX)
            && (elapsed >= DRAG_POINT_INTERVAL.as_secs_f64()
                || point_distance >= squared_distance_threshold(DRAG_FORCE_SAMPLE_DISTANCE_PX))
        {
            self.points.push(point);
        }
    }

    fn finish(mut self, release_point: DragPoint) -> Option<DragInteraction> {
        self.update(release_point.clone());

        if !self.is_drag {
            return None;
        }

        if let Some(last_point) = self.points.last() {
            if release_point.timestamp > last_point.timestamp
                && (release_point.x != last_point.x || release_point.y != last_point.y)
            {
                self.points.push(release_point);
            } else if release_point.timestamp > last_point.timestamp {
                let last_index = self.points.len() - 1;
                self.points[last_index].timestamp = release_point.timestamp;
            }
        }

        if self.points.len() < 2 {
            return None;
        }

        let start = self.points.first()?.timestamp;
        let end = self.points.last()?.timestamp;

        if end <= start {
            return None;
        }

        Some(DragInteraction {
            start,
            end,
            button: self.button,
            points: self.points,
        })
    }
}

fn push_click(
    clicks: &Arc<Mutex<Vec<ClickEvent>>>,
    started_at: Instant,
    button: ClickButton,
    filter: &ClickFilter,
) -> Option<DragPoint> {
    // Sample raw screen coordinates first.
    let (screen_x, screen_y) = cursor_position()?;
    let timestamp = elapsed_seconds(started_at);

    // Apply the filter: returns the coordinates to store (possibly converted to
    // window-relative) or None if the click should be discarded.
    let (stored_x, stored_y) = apply_click_filter(filter, screen_x, screen_y)?;

    if let Ok(mut clicks) = clicks.lock() {
        clicks.push(ClickEvent {
            timestamp,
            x: stored_x,
            y: stored_y,
            button,
        });
    }

    // The DragPoint must carry the same stored coordinates so that drag tracking
    // is consistent with click tracking.
    Some(DragPoint {
        timestamp,
        x: stored_x,
        y: stored_y,
    })
}

fn update_pending_drag(drag: &mut Option<PendingDrag>, started_at: Instant) {
    let Some(drag) = drag.as_mut() else {
        return;
    };

    if let Some(point) = current_drag_point(started_at) {
        drag.update(point);
    }
}

fn finish_pending_drag(
    drag: &mut Option<PendingDrag>,
    drags: &Arc<Mutex<Vec<DragInteraction>>>,
    started_at: Instant,
) {
    let Some(drag) = drag.take() else {
        return;
    };

    let Some(release_point) = current_drag_point(started_at) else {
        return;
    };

    let Some(drag) = drag.finish(release_point) else {
        return;
    };

    if let Ok(mut drags) = drags.lock() {
        drags.push(drag);
    }
}

fn current_drag_point(started_at: Instant) -> Option<DragPoint> {
    let (x, y) = cursor_position()?;

    Some(DragPoint {
        timestamp: elapsed_seconds(started_at),
        x,
        y,
    })
}

fn distance_squared(left: &DragPoint, right: &DragPoint) -> i64 {
    let x = i64::from(left.x) - i64::from(right.x);
    let y = i64::from(left.y) - i64::from(right.y);

    x * x + y * y
}

fn squared_distance_threshold(pixels: i32) -> i64 {
    let pixels = i64::from(pixels);

    pixels * pixels
}

#[cfg(target_os = "windows")]
fn left_mouse_button_down() -> bool {
    unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0 }
}

#[cfg(not(target_os = "windows"))]
fn left_mouse_button_down() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn right_mouse_button_down() -> bool {
    unsafe { GetAsyncKeyState(i32::from(VK_RBUTTON.0)) < 0 }
}

#[cfg(not(target_os = "windows"))]
fn right_mouse_button_down() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point).ok()?;
    }
    Some((point.x, point.y))
}

#[cfg(not(target_os = "windows"))]
fn cursor_position() -> Option<(i32, i32)> {
    None
}

/// Returns the root (top-level) HWND currently under the cursor as a raw
/// pointer value, or `None` if the cursor position cannot be read.
#[cfg(target_os = "windows")]
fn hwnd_under_cursor() -> Option<usize> {
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point).ok()?;
        let hwnd = WindowFromPoint(point);
        if hwnd.0.is_null() {
            return None;
        }
        // Walk up to the root (owner-less ancestor) so that clicking a child
        // control inside the target window is still accepted.
        let root = GetAncestor(hwnd, GA_ROOT);
        if root.0.is_null() {
            return None;
        }
        Some(root.0 as usize)
    }
}

#[cfg(not(target_os = "windows"))]
fn hwnd_under_cursor() -> Option<usize> {
    None
}

/// Returns the current top-left screen position `(left, top)` of the window
/// identified by `hwnd`, or `None` if the rect cannot be read.
#[cfg(target_os = "windows")]
fn window_rect_for_hwnd(hwnd: usize) -> Option<(i32, i32)> {
    let mut rect = RECT::default();
    unsafe {
        GetWindowRect(HWND(hwnd as *mut _), &mut rect).ok()?;
    }
    Some((rect.left, rect.top))
}

#[cfg(not(target_os = "windows"))]
fn window_rect_for_hwnd(_hwnd: usize) -> Option<(i32, i32)> {
    None
}

/// Evaluates `filter` against the click at `(screen_x, screen_y)` and returns
/// the coordinates to store, or `None` if the click should be discarded.
///
/// - [`ClickFilter::All`]: returns `(screen_x, screen_y)` unchanged. The
///   caller is responsible for converting to capture-relative coordinates after
///   the recording stops (via `normalize_interactions_to_capture`).
/// - [`ClickFilter::Window`]: checks that the root HWND under the cursor
///   matches the recorded window, then converts screen coordinates to
///   window-relative using the window's *current* position (live `GetWindowRect`
///   call). Out-of-bounds clicks (e.g. after a window resize) are discarded.
fn apply_click_filter(
    filter: &ClickFilter,
    screen_x: i32,
    screen_y: i32,
) -> Option<(i32, i32)> {
    match filter {
        ClickFilter::All => Some((screen_x, screen_y)),
        ClickFilter::Window {
            hwnd: target_hwnd,
            encoder_width,
            encoder_height,
        } => {
            let root_hwnd = hwnd_under_cursor()?;
            if root_hwnd != *target_hwnd {
                println!(
                    "[FocusFlow click] Ignored  click x={screen_x} y={screen_y} hwnd=0x{root_hwnd:X}"
                );
                return None;
            }
            // Convert screen -> window-relative using the *live* window origin
            // so that movement during recording is handled correctly.
            let (win_left, win_top) = window_rect_for_hwnd(*target_hwnd)?;
            let rel_x = screen_x - win_left;
            let rel_y = screen_y - win_top;
            // Discard clicks that fall outside the encoder dimensions (can
            // happen if the window is resized after recording starts).
            if rel_x < 0
                || rel_y < 0
                || rel_x >= *encoder_width as i32
                || rel_y >= *encoder_height as i32
            {
                println!(
                    "[FocusFlow click] Ignored  click x={screen_x} y={screen_y} hwnd=0x{root_hwnd:X} (out of encoder bounds)"
                );
                return None;
            }
            println!(
                "[FocusFlow click] Accepted click x={screen_x} y={screen_y} hwnd=0x{root_hwnd:X} -> rel=({rel_x},{rel_y})"
            );
            Some((rel_x, rel_y))
        }
    }
}

fn normalize_interactions_to_capture(
    interactions: InteractionEvents,
    bounds: CaptureBounds,
) -> InteractionEvents {
    InteractionEvents {
        clicks: interactions
            .clicks
            .into_iter()
            .filter_map(|click| normalize_click_to_capture(click, bounds))
            .collect(),
        drags: interactions
            .drags
            .into_iter()
            .filter_map(|drag| normalize_drag_to_capture(drag, bounds))
            .collect(),
    }
}

fn normalize_click_to_capture(mut click: ClickEvent, bounds: CaptureBounds) -> Option<ClickEvent> {
    let (x, y) = normalize_point_to_capture(click.x, click.y, bounds)?;
    click.x = x;
    click.y = y;
    Some(click)
}

fn normalize_drag_to_capture(
    mut drag: DragInteraction,
    bounds: CaptureBounds,
) -> Option<DragInteraction> {
    let points = drag
        .points
        .into_iter()
        .filter_map(|point| normalize_drag_point_to_capture(point, bounds))
        .collect::<Vec<_>>();

    if points.len() < 2 {
        return None;
    }

    drag.start = points
        .first()
        .map(|point| point.timestamp)
        .unwrap_or(drag.start);
    drag.end = points
        .last()
        .map(|point| point.timestamp)
        .unwrap_or(drag.end);
    drag.points = points;
    Some(drag)
}

fn normalize_drag_point_to_capture(
    mut point: DragPoint,
    bounds: CaptureBounds,
) -> Option<DragPoint> {
    let (x, y) = normalize_point_to_capture(point.x, point.y, bounds)?;
    point.x = x;
    point.y = y;
    Some(point)
}

fn normalize_point_to_capture(x: i32, y: i32, bounds: CaptureBounds) -> Option<(i32, i32)> {
    let relative_x = i64::from(x) - i64::from(bounds.x);
    let relative_y = i64::from(y) - i64::from(bounds.y);

    if relative_x < 0
        || relative_y < 0
        || relative_x >= i64::from(bounds.width)
        || relative_y >= i64::from(bounds.height)
    {
        return None;
    }

    Some((relative_x as i32, relative_y as i32))
}

fn write_clicks_json(output_path: &Path, clicks: &[ClickEvent]) -> RecorderResult<()> {
    let clicks_path = clicks_output_path(output_path)?;
    let json = serde_json::to_vec_pretty(clicks).map_err(|error| {
        RecorderError::new(
            "serialize_clicks_failed",
            format!("Could not serialize click events: {error}"),
            true,
        )
    })?;

    fs::write(&clicks_path, json).map_err(|error| {
        RecorderError::new(
            "write_clicks_failed",
            format!("Could not write clicks.json: {error}"),
            true,
        )
    })
}

fn write_drags_json(output_path: &Path, drags: &[DragInteraction]) -> RecorderResult<()> {
    let drags_path = drags_output_path(output_path)?;
    let json = serde_json::to_vec_pretty(drags).map_err(|error| {
        RecorderError::new(
            "serialize_drags_failed",
            format!("Could not serialize drag interactions: {error}"),
            true,
        )
    })?;

    fs::write(&drags_path, json).map_err(|error| {
        RecorderError::new(
            "write_drags_failed",
            format!("Could not write drags.json: {error}"),
            true,
        )
    })
}

fn write_timeline_json(output_path: &Path) -> RecorderResult<()> {
    let clicks_path = clicks_output_path(output_path)?;
    let clicks_json = fs::read(&clicks_path).map_err(|error| {
        RecorderError::new(
            "read_clicks_failed",
            format!("Could not read clicks.json for timeline generation: {error}"),
            true,
        )
    })?;
    let clicks: Vec<ClickEvent> = serde_json::from_slice(&clicks_json).map_err(|error| {
        RecorderError::new(
            "parse_clicks_failed",
            format!("Could not parse clicks.json for timeline generation: {error}"),
            true,
        )
    })?;
    let drags_path = drags_output_path(output_path)?;
    let drags_json = fs::read(&drags_path).map_err(|error| {
        RecorderError::new(
            "read_drags_failed",
            format!("Could not read drags.json for timeline generation: {error}"),
            true,
        )
    })?;
    let drags: Vec<DragInteraction> = serde_json::from_slice(&drags_json).map_err(|error| {
        RecorderError::new(
            "parse_drags_failed",
            format!("Could not parse drags.json for timeline generation: {error}"),
            true,
        )
    })?;
    let timeline = build_timeline_segments(clicks, drags);
    let timeline_json = serde_json::to_vec_pretty(&timeline).map_err(|error| {
        RecorderError::new(
            "serialize_timeline_failed",
            format!("Could not serialize timeline segments: {error}"),
            true,
        )
    })?;

    fs::write(timeline_output_path(output_path)?, timeline_json).map_err(|error| {
        RecorderError::new(
            "write_timeline_failed",
            format!("Could not write timeline.json: {error}"),
            true,
        )
    })
}

fn build_timeline_segments(
    clicks: Vec<ClickEvent>,
    drags: Vec<DragInteraction>,
) -> Vec<TimelineSegment> {
    println!(
        "[FocusFlow timeline] raw click count: {}",
        clicks.len()
    );
    println!(
        "[FocusFlow timeline] raw drag count: {}",
        drags.len()
    );

    let mut timeline = build_click_timeline_segments(clicks);
    timeline.extend(build_drag_timeline_segments(drags));

    println!(
        "[FocusFlow timeline] segment count before cleanup: {}",
        timeline.len()
    );

    let timeline = cleanup_timeline_segments(timeline);

    println!(
        "[FocusFlow timeline] segment count after cleanup: {}",
        timeline.len()
    );

    timeline
}

/// Converts a raw, potentially overlapping list of [`TimelineSegment`]s
/// (produced by independently merging click and drag segments) into a clean,
/// non-overlapping, sorted list suitable for the export pipeline.
///
/// Steps (all in a single pass after sorting):
///
/// 1. **Sort** by `start` ascending.
/// 2. **Deduplicate**: when two segments share the same `start` time, keep
///    the one with the later `end` (wider coverage).
/// 3. **Merge nearby sequences**: if the gap between the end of the current
///    accumulator segment and the start of the next segment is within
///    [`TIMELINE_MERGE_WINDOW_SECONDS`], extend the accumulator instead of
///    starting a new zoom-out/zoom-in cycle.  The new segment is appended to
///    the accumulator's chain so the camera pans smoothly.
/// 4. **Clip overlaps**: if a following segment still starts before the
///    previous segment ends, trim the previous segment's `end` to the
///    following segment's `start`.
/// 5. **Drop invalid segments**: remove any segment where `end <= start`.
fn cleanup_timeline_segments(mut raw: Vec<TimelineSegment>) -> Vec<TimelineSegment> {
    if raw.is_empty() {
        return raw;
    }

    // Step 1 — sort by start time, break ties by preferring later end (wider).
    raw.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.end
                    .partial_cmp(&a.end)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    // Step 2 — deduplicate same-start segments (keep the one with the later end).
    raw.dedup_by(|later, earlier| {
        if (later.start - earlier.start).abs() <= MIN_TIMELINE_SEGMENT_SECONDS {
            // `dedup_by` removes `later` when returning true.  We want to keep
            // the one with the later `end`, so copy its fields into `earlier`
            // (which survives) if `later` is wider.
            if later.end > earlier.end {
                earlier.end = later.end;
                earlier.x = later.x;
                earlier.y = later.y;
            }
            true
        } else {
            false
        }
    });

    // Steps 3 & 4 — single forward pass: merge nearby sequences, clip overlaps.
    let mut out: Vec<TimelineSegment> = Vec::with_capacity(raw.len());

    for seg in raw {
        if let Some(prev) = out.last_mut() {
            let gap = seg.start - prev.end;

            if gap <= TIMELINE_MERGE_WINDOW_SECONDS {
                // The two sequences are close enough to merge: extend `prev`
                // to cover `seg` without zooming out in between.  We update
                // the camera target to `seg`'s position and widen the window.
                // (The export pipeline's `timeline_sequences` groups contiguous
                // segments into one sequence, so no artificial boundary is
                // needed here.)
                if seg.end > prev.end {
                    // Push a new segment starting where prev ends (or at
                    // seg.start if that is later) so the export pipeline sees
                    // a smooth pan rather than a jump.
                    let pan_start = prev.end.max(seg.start);
                    if pan_start < seg.end - MIN_TIMELINE_SEGMENT_SECONDS {
                        out.push(TimelineSegment {
                            start: pan_start,
                            end: seg.end,
                            x: seg.x,
                            y: seg.y,
                            scale: seg.scale,
                        });
                    } else {
                        // The window is too narrow to add a segment; just
                        // extend the previous one.
                        prev.end = seg.end;
                        prev.x = seg.x;
                        prev.y = seg.y;
                    }
                }
                // If seg is entirely contained within prev, discard it.
                continue;
            }

            // Gap is wide enough for a separate sequence.  Before appending,
            // clip any leftover overlap (shouldn't happen after dedup but
            // defends against floating-point edge cases).
            if seg.start < prev.end {
                prev.end = seg.start;
            }
        }

        // Only push segments that still have positive duration.
        if seg.end > seg.start + MIN_TIMELINE_SEGMENT_SECONDS {
            out.push(seg);
        }
    }

    // Step 5 — final pass: drop any zero-width or inverted segments that
    // slipped through (defensive).
    out.retain(|seg| seg.end > seg.start + MIN_TIMELINE_SEGMENT_SECONDS);

    out
}

fn build_click_timeline_segments(mut clicks: Vec<ClickEvent>) -> Vec<TimelineSegment> {
    clicks.sort_by(|left, right| {
        left.timestamp
            .partial_cmp(&right.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut timeline = Vec::new();
    let mut sequence_complete_at = 0.0;

    for click in clicks
        .into_iter()
        .filter(|click| click.button == ClickButton::Left)
    {
        let segment_end = click.timestamp + IDLE_BEFORE_ZOOM_OUT_SECONDS;

        if timeline.is_empty() {
            timeline.push(TimelineSegment {
                start: (click.timestamp - PRE_CLICK_ZOOM_SECONDS).max(0.0),
                end: segment_end,
                x: click.x,
                y: click.y,
                scale: 2.0,
            });
            sequence_complete_at = segment_end + ZOOM_OUT_SECONDS;
            continue;
        }

        if click.timestamp <= sequence_complete_at {
            if let Some(last_segment) = timeline.last_mut() {
                if click.timestamp > last_segment.start + MIN_TIMELINE_SEGMENT_SECONDS {
                    last_segment.end = click.timestamp;
                    timeline.push(TimelineSegment {
                        start: click.timestamp,
                        end: segment_end,
                        x: click.x,
                        y: click.y,
                        scale: 2.0,
                    });
                } else {
                    last_segment.end = segment_end;
                    last_segment.x = click.x;
                    last_segment.y = click.y;
                }
            }
        } else {
            let segment_start = (click.timestamp - PRE_CLICK_ZOOM_SECONDS)
                .max(0.0)
                .max(sequence_complete_at + MIN_TIMELINE_SEGMENT_SECONDS)
                .min(click.timestamp);

            timeline.push(TimelineSegment {
                start: segment_start,
                end: segment_end,
                x: click.x,
                y: click.y,
                scale: 2.0,
            });
        }

        sequence_complete_at = segment_end + ZOOM_OUT_SECONDS;
    }

    timeline
}

fn build_drag_timeline_segments(mut drags: Vec<DragInteraction>) -> Vec<TimelineSegment> {
    drags.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut timeline = Vec::new();

    for drag in drags
        .into_iter()
        .filter(|drag| drag.button == ClickButton::Left)
    {
        let points = normalized_drag_points(drag.points);

        if points.len() < 2 {
            continue;
        }

        let drag_start = drag.start.min(points[0].timestamp);
        let drag_end = drag.end.max(
            points
                .last()
                .map(|point| point.timestamp)
                .unwrap_or(drag.end),
        );
        let sequence_start = (drag_start - PRE_CLICK_ZOOM_SECONDS).max(0.0);
        let sequence_end = drag_end + IDLE_BEFORE_ZOOM_OUT_SECONDS;
        let mut previous_segment_start = sequence_start;

        for index in 0..points.len() {
            let point = &points[index];
            let segment_start = if index == 0 {
                sequence_start
            } else {
                point
                    .timestamp
                    .max(previous_segment_start + MIN_TIMELINE_SEGMENT_SECONDS)
            };
            let segment_end = points
                .get(index + 1)
                .map(|next_point| next_point.timestamp)
                .unwrap_or(sequence_end)
                .max(segment_start + MIN_TIMELINE_SEGMENT_SECONDS);

            timeline.push(TimelineSegment {
                start: segment_start,
                end: segment_end,
                x: point.x,
                y: point.y,
                scale: 2.0,
            });
            previous_segment_start = segment_start;
        }
    }

    timeline
}

fn normalized_drag_points(mut points: Vec<DragPoint>) -> Vec<DragPoint> {
    points.retain(|point| point.timestamp.is_finite() && point.timestamp >= 0.0);
    points.sort_by(|left, right| {
        left.timestamp
            .partial_cmp(&right.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    points.dedup_by(|right, left| {
        right.timestamp <= left.timestamp + f64::EPSILON && right.x == left.x && right.y == left.y
    });

    points
}

fn clicks_output_path(output_path: &Path) -> RecorderResult<PathBuf> {
    Ok(session_output_dir(output_path)?.join(CLICKS_FILE_NAME))
}

fn drags_output_path(output_path: &Path) -> RecorderResult<PathBuf> {
    Ok(session_output_dir(output_path)?.join(DRAGS_FILE_NAME))
}

fn timeline_output_path(output_path: &Path) -> RecorderResult<PathBuf> {
    Ok(session_output_dir(output_path)?.join(TIMELINE_FILE_NAME))
}

fn metadata_output_path(output_path: &Path) -> RecorderResult<PathBuf> {
    Ok(session_output_dir(output_path)?.join(METADATA_FILE_NAME))
}

fn session_json_output_path(output_path: &Path) -> RecorderResult<PathBuf> {
    Ok(session_output_dir(output_path)?.join(SESSION_FILE_NAME))
}

fn session_output_dir(output_path: &Path) -> RecorderResult<&Path> {
    output_path.parent().ok_or_else(|| {
        RecorderError::new(
            "invalid_session_output_path",
            "Could not resolve a parent directory for recording session assets",
            true,
        )
    })
}

/// Builds the [`ClickFilter`] appropriate for the given capture plan.
///
/// Screen and Region recordings use [`ClickFilter::All`] so that the existing
/// post-recording normalization converts screen coordinates to capture-relative.
/// Window recordings use [`ClickFilter::Window`] so that only clicks inside the
/// target window are accepted and coordinates are converted at click time.
fn click_filter_for_source(plan: &CapturePlan) -> ClickFilter {
    if let RecordingSource::Window { ref hwnd, .. } = plan.source {
        if let Ok(h) = parse_window_handle(hwnd) {
            if let Some(ref wcp) = plan.window_capture {
                return ClickFilter::Window {
                    hwnd: h.0 as usize,
                    encoder_width: wcp.width,
                    encoder_height: wcp.height,
                };
            }
        }
    }
    ClickFilter::All
}

fn capture_plan_for_source(
    app: &AppHandle,
    source: RecordingSource,
    output_path: &Path,
) -> RecorderResult<CapturePlan> {
    match source {
        RecordingSource::Screen => {
            let monitor = primary_monitor_info(app)?;
            let bounds = capture_bounds_from_monitor(&monitor);
            let args = ffmpeg_desktop_region_args(bounds, output_path)?;

            Ok(CapturePlan {
                source: RecordingSource::Screen,
                monitor,
                bounds,
                backend: CaptureBackend::Ffmpeg,
                ffmpeg_args: args,
                window_capture: None,
            })
        }
        RecordingSource::Region {
            x,
            y,
            width,
            height,
        } => {
            let bounds = validated_capture_bounds(x, y, width, height)?;
            let monitor = MonitorInfo {
                name: Some("Selected region".to_string()),
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
                scale_factor: 1.0,
            };
            let args = ffmpeg_desktop_region_args(bounds, output_path)?;

            Ok(CapturePlan {
                source: RecordingSource::Region {
                    x: bounds.x,
                    y: bounds.y,
                    width: bounds.width,
                    height: bounds.height,
                },
                monitor,
                bounds,
                backend: CaptureBackend::Ffmpeg,
                ffmpeg_args: args,
                window_capture: None,
            })
        }
        RecordingSource::Window { hwnd, title } => {
            let hwnd = parse_window_handle(&hwnd)?;
            let (window_capture, original_bounds) = window_capture_plan_from_hwnd(hwnd, &title)?;
            let source_monitor = monitor_info_for_capture_bounds(app, original_bounds)?;
            let bounds = validated_capture_bounds(
                original_bounds.x,
                original_bounds.y,
                window_capture.width,
                window_capture.height,
            )?;
            log_window_capture_bounds("Original window bounds", original_bounds);
            println!(
                "[FocusFlow recorder] Window capture backend uses Windows Graphics Capture; FFmpeg gdigrab HWND capture is bypassed."
            );
            let monitor = MonitorInfo {
                name: Some(window_capture.title.clone()),
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
                scale_factor: source_monitor.scale_factor,
            };

            Ok(CapturePlan {
                source: RecordingSource::Window {
                    hwnd: window_id(hwnd),
                    title: window_capture.title.clone(),
                },
                monitor,
                bounds,
                backend: CaptureBackend::WindowsCapture,
                ffmpeg_args: Vec::new(),
                window_capture: Some(window_capture),
            })
        }
    }
}

fn capture_bounds_from_monitor(monitor: &MonitorInfo) -> CaptureBounds {
    CaptureBounds {
        x: monitor.x,
        y: monitor.y,
        width: monitor.width,
        height: monitor.height,
    }
}

fn validated_capture_bounds(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> RecorderResult<CaptureBounds> {
    if width < 16 || height < 16 {
        return Err(RecorderError::new(
            "invalid_capture_region",
            format!("Capture region must be at least 16x16, got {width}x{height}"),
            true,
        ));
    }

    Ok(CaptureBounds {
        x,
        y,
        width,
        height,
    })
}

fn monitor_info_for_capture_bounds(
    app: &AppHandle,
    bounds: CaptureBounds,
) -> RecorderResult<MonitorInfo> {
    let monitors = app.available_monitors().map_err(|error| {
        RecorderError::new(
            "monitor_query_failed",
            format!("Could not query available monitors: {error}"),
            true,
        )
    })?;

    let best_monitor = monitors
        .into_iter()
        .map(|monitor| {
            let size = monitor.size();
            let position = monitor.position();
            let info = MonitorInfo {
                name: monitor.name().cloned(),
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                scale_factor: monitor.scale_factor(),
            };
            let intersection_area =
                capture_bounds_intersection_area(bounds, capture_bounds_from_monitor(&info));

            (intersection_area, info)
        })
        .max_by_key(|(intersection_area, _)| *intersection_area);

    match best_monitor {
        Some((0, _)) | None => primary_monitor_info(app),
        Some((_, monitor)) => Ok(monitor),
    }
}

fn capture_bounds_intersection_area(left: CaptureBounds, right: CaptureBounds) -> u64 {
    let left_right = i64::from(left.x) + i64::from(left.width);
    let left_bottom = i64::from(left.y) + i64::from(left.height);
    let right_right = i64::from(right.x) + i64::from(right.width);
    let right_bottom = i64::from(right.y) + i64::from(right.height);
    let intersection_width = left_right
        .min(right_right)
        .saturating_sub(i64::from(left.x).max(i64::from(right.x)));
    let intersection_height = left_bottom
        .min(right_bottom)
        .saturating_sub(i64::from(left.y).max(i64::from(right.y)));

    if intersection_width <= 0 || intersection_height <= 0 {
        return 0;
    }

    (intersection_width as u64) * (intersection_height as u64)
}

fn ffmpeg_desktop_region_args(
    bounds: CaptureBounds,
    output_path: &Path,
) -> RecorderResult<Vec<String>> {
    let output_path = path_to_string(output_path)?;
    let video_size = format!("{}x{}", bounds.width, bounds.height);

    Ok(vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "info".to_string(),
        "-f".to_string(),
        "gdigrab".to_string(),
        "-framerate".to_string(),
        TARGET_FPS.to_string(),
        "-offset_x".to_string(),
        bounds.x.to_string(),
        "-offset_y".to_string(),
        bounds.y.to_string(),
        "-video_size".to_string(),
        video_size,
        "-draw_mouse".to_string(),
        "1".to_string(),
        "-i".to_string(),
        "desktop".to_string(),
        "-an".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        "veryfast".to_string(),
        "-crf".to_string(),
        "23".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-y".to_string(),
        output_path,
    ])
}

fn primary_monitor_info(app: &AppHandle) -> RecorderResult<MonitorInfo> {
    let monitor = app
        .primary_monitor()
        .map_err(|error| {
            RecorderError::new(
                "primary_monitor_query_failed",
                format!("Could not query the primary monitor: {error}"),
                true,
            )
        })?
        .ok_or_else(|| {
            RecorderError::new(
                "primary_monitor_unavailable",
                "No primary monitor was reported by the operating system",
                true,
            )
        })?;

    let size = monitor.size();
    let position = monitor.position();

    if size.width == 0 || size.height == 0 {
        return Err(RecorderError::new(
            "invalid_primary_monitor_size",
            format!(
                "Primary monitor has invalid size {}x{}",
                size.width, size.height
            ),
            true,
        ));
    }

    Ok(MonitorInfo {
        name: monitor.name().cloned(),
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        scale_factor: monitor.scale_factor(),
    })
}

fn create_session_screen_output_path(app: &AppHandle) -> RecorderResult<PathBuf> {
    let recordings_dir = recordings_root_dir(app)?;
    let base_name = timestamp_folder_name()?;

    for attempt in 0..1000 {
        let folder_name = if attempt == 0 {
            base_name.clone()
        } else {
            format!("{base_name}_{attempt:03}")
        };
        let session_dir = recordings_dir.join(folder_name);

        match fs::create_dir(&session_dir) {
            Ok(()) => return Ok(session_dir.join(OUTPUT_FILE_NAME)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(RecorderError::new(
                    "create_session_dir_failed",
                    format!("Could not create recording session directory: {error}"),
                    true,
                ));
            }
        }
    }

    Err(RecorderError::new(
        "create_session_dir_failed",
        "Could not create a unique recording session directory",
        true,
    ))
}

fn write_session_metadata(
    output_path: &Path,
    source: &RecordingSource,
    capture_bounds: CaptureBounds,
    capture_backend: CaptureBackend,
    has_mic_audio: bool,
    mic_device_name: Option<String>,
) -> RecorderResult<()> {
    let metadata = SessionMetadata {
        version: 1,
        source: source.clone(),
        capture_bounds,
        capture_backend,
        has_mic_audio,
        mic_device_name,
    };
    let json = serde_json::to_vec_pretty(&metadata).map_err(|error| {
        RecorderError::new(
            "serialize_metadata_failed",
            format!("Could not serialize recording metadata: {error}"),
            true,
        )
    })?;

    fs::write(metadata_output_path(output_path)?, json).map_err(|error| {
        RecorderError::new(
            "write_metadata_failed",
            format!("Could not write metadata.json: {error}"),
            true,
        )
    })
}

fn write_session_json(
    output_path: &Path,
    source: &RecordingSource,
    duration_seconds: f64,
    exported: bool,
) -> RecorderResult<()> {
    let session_dir = session_output_dir(output_path)?;
    let session_id = session_id_from_dir(session_dir)?;
    let session = SessionSummary {
        created_at: created_at_from_session_id(&session_id),
        session_id,
        duration_seconds,
        recording_source: recording_source_label(source).to_string(),
        exported,
    };
    let json = serde_json::to_vec_pretty(&session).map_err(|error| {
        RecorderError::new(
            "serialize_session_failed",
            format!("Could not serialize session.json: {error}"),
            true,
        )
    })?;

    fs::write(session_json_output_path(output_path)?, json).map_err(|error| {
        RecorderError::new(
            "write_session_failed",
            format!("Could not write session.json: {error}"),
            true,
        )
    })
}

pub(crate) fn mark_session_exported(session_dir: &Path) -> RecorderResult<()> {
    let mut summary = match read_session_summary(session_dir) {
        Ok(Some(summary)) => summary,
        Ok(None) => fallback_session_summary(session_dir),
        Err(error) => {
            eprintln!("Could not read existing session metadata before export update: {error}");
            fallback_session_summary(session_dir)
        }
    };
    summary.exported = true;
    write_session_summary(session_dir, &summary)
}

fn read_session_summary(session_dir: &Path) -> RecorderResult<Option<SessionSummary>> {
    let path = session_dir.join(SESSION_FILE_NAME);

    if !path.exists() {
        return Ok(None);
    }

    let bytes = fs::read(&path).map_err(|error| {
        RecorderError::new(
            "read_session_failed",
            format!("Could not read session.json: {error}"),
            true,
        )
    })?;
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        RecorderError::new(
            "parse_session_failed",
            format!("Could not parse session.json: {error}"),
            true,
        )
    })
}

fn write_session_summary(session_dir: &Path, summary: &SessionSummary) -> RecorderResult<()> {
    let json = serde_json::to_vec_pretty(summary).map_err(|error| {
        RecorderError::new(
            "serialize_session_failed",
            format!("Could not serialize session.json: {error}"),
            true,
        )
    })?;

    fs::write(session_dir.join(SESSION_FILE_NAME), json).map_err(|error| {
        RecorderError::new(
            "write_session_failed",
            format!("Could not write session.json: {error}"),
            true,
        )
    })
}

fn recent_session_from_dir(session_dir: &Path) -> RecorderResult<Option<RecentSession>> {
    if !session_dir.join(OUTPUT_FILE_NAME).exists() {
        return Ok(None);
    }

    let session_id = session_id_from_dir(session_dir)?;
    let fallback = SessionSummary {
        session_id: session_id.clone(),
        created_at: created_at_from_session_id(&session_id),
        duration_seconds: 0.0,
        recording_source: "unknown".to_string(),
        exported: session_dir.join(EDITED_FILE_NAME).exists(),
    };
    let summary = match read_session_summary(session_dir) {
        Ok(Some(summary)) => summary,
        Ok(None) => fallback,
        Err(error) => {
            eprintln!("Could not read session metadata for browser list: {error}");
            fallback
        }
    };
    let edited_video_path = session_dir.join(EDITED_FILE_NAME);
    let exported = summary.exported || edited_video_path.exists();

    Ok(Some(RecentSession {
        session_id: summary.session_id,
        created_at: summary.created_at,
        duration_seconds: summary.duration_seconds,
        recording_source: summary.recording_source,
        exported,
        session_path: path_to_string(session_dir)?,
        edited_video_path: if edited_video_path.exists() {
            Some(path_to_string(&edited_video_path)?)
        } else {
            None
        },
    }))
}

fn fallback_session_summary(session_dir: &Path) -> SessionSummary {
    let session_id = session_id_from_dir(session_dir).unwrap_or_else(|_| "unknown".to_string());

    SessionSummary {
        created_at: created_at_from_session_id(&session_id),
        session_id,
        duration_seconds: 0.0,
        recording_source: "unknown".to_string(),
        exported: session_dir.join(EDITED_FILE_NAME).exists(),
    }
}

fn session_dir_from_id(recordings_dir: &Path, session_id: &str) -> RecorderResult<PathBuf> {
    if session_id.trim().is_empty()
        || session_id.contains('\\')
        || session_id.contains('/')
        || session_id.contains("..")
    {
        return Err(RecorderError::new(
            "invalid_session_id",
            "Session id is invalid",
            true,
        ));
    }

    let recordings_dir = recordings_dir.canonicalize().map_err(|error| {
        RecorderError::new(
            "resolve_recordings_dir_failed",
            format!("Could not resolve recordings directory: {error}"),
            true,
        )
    })?;
    let session_dir = recordings_dir.join(session_id);
    let canonical_session_dir = session_dir.canonicalize().map_err(|error| {
        RecorderError::new(
            "resolve_session_dir_failed",
            format!("Could not resolve session directory: {error}"),
            true,
        )
    })?;

    if !canonical_session_dir.starts_with(&recordings_dir) || !canonical_session_dir.is_dir() {
        return Err(RecorderError::new(
            "invalid_session_id",
            "Session id is outside the recordings directory",
            true,
        ));
    }

    Ok(canonical_session_dir)
}

fn session_id_from_dir(session_dir: &Path) -> RecorderResult<String> {
    session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            RecorderError::new(
                "invalid_session_dir",
                format!("Could not derive session id from {}", session_dir.display()),
                true,
            )
        })
}

fn created_at_from_session_id(session_id: &str) -> String {
    let parts = session_id.split('_').collect::<Vec<_>>();

    if parts.len() < 2 || parts[0].len() != 8 || parts[1].len() != 6 {
        return session_id.to_string();
    }

    let date = parts[0];
    let time = parts[1];

    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &date[0..4],
        &date[4..6],
        &date[6..8],
        &time[0..2],
        &time[2..4],
        &time[4..6]
    )
}

fn recording_source_label(source: &RecordingSource) -> &'static str {
    match source {
        RecordingSource::Screen => "screen",
        RecordingSource::Window { .. } => "window",
        RecordingSource::Region { .. } => "region",
    }
}

fn elapsed_seconds_from_millis(milliseconds: u64) -> f64 {
    milliseconds as f64 / 1000.0
}

pub(crate) fn recordings_root_dir(app: &AppHandle) -> RecorderResult<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|error| {
            RecorderError::new(
                "app_data_dir_unavailable",
                format!("Could not resolve AppData directory: {error}"),
                true,
            )
        })?
        .join(OUTPUT_RECORDINGS_DIR_NAME);

    if !dir.is_absolute() {
        return Err(RecorderError::new(
            "relative_output_dir",
            format!(
                "Recording output directory must be absolute, but resolved to: {}",
                dir.display()
            ),
            true,
        ));
    }

    fs::create_dir_all(&dir).map_err(|error| {
        RecorderError::new(
            "create_output_dir_failed",
            format!("Could not create recording output directory: {error}"),
            true,
        )
    })?;

    Ok(dir)
}

#[cfg(target_os = "windows")]
fn enumerate_recordable_windows() -> RecorderResult<Vec<RecordableWindow>> {
    let mut windows: Vec<RecordableWindow> = Vec::new();

    unsafe {
        EnumWindows(
            Some(enum_recordable_window),
            LPARAM((&mut windows as *mut Vec<RecordableWindow>) as isize),
        )
        .map_err(|error| {
            RecorderError::new(
                "window_enumeration_failed",
                format!("Could not enumerate windows: {error}"),
                true,
            )
        })?;
    }

    windows.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then(left.id.cmp(&right.id))
    });

    Ok(windows)
}

#[cfg(not(target_os = "windows"))]
fn enumerate_recordable_windows() -> RecorderResult<Vec<RecordableWindow>> {
    Ok(Vec::new())
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn enum_recordable_window(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows::core::BOOL {
    if lparam.0 == 0 {
        return TRUE;
    }

    let windows = &mut *(lparam.0 as *mut Vec<RecordableWindow>);

    if let Some(window) = recordable_window_info(hwnd) {
        windows.push(window);
    }

    TRUE
}

#[cfg(target_os = "windows")]
fn recordable_window_info(hwnd: HWND) -> Option<RecordableWindow> {
    if hwnd.0.is_null() {
        return None;
    }

    let shell_window = unsafe { GetShellWindow() };

    if hwnd.0 == shell_window.0 {
        return None;
    }

    if !unsafe { IsWindowVisible(hwnd).as_bool() } || unsafe { IsIconic(hwnd).as_bool() } {
        return None;
    }

    if unsafe { GetWindow(hwnd, GW_OWNER).is_ok() } {
        return None;
    }

    let title = window_title(hwnd)?;

    if is_ignored_window_title(&title) {
        return None;
    }

    let bounds = window_capture_bounds(hwnd)?;

    Some(RecordableWindow {
        id: window_id(hwnd),
        title,
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
    })
}

#[cfg(target_os = "windows")]
fn window_title(hwnd: HWND) -> Option<String> {
    let mut buffer = [0_u16; 512];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };

    if length <= 0 {
        return None;
    }

    let title = String::from_utf16_lossy(&buffer[..length as usize])
        .trim()
        .to_string();

    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

#[cfg(target_os = "windows")]
fn is_ignored_window_title(title: &str) -> bool {
    matches!(
        title,
        "Program Manager" | "Windows Input Experience" | "Default IME"
    )
}

#[cfg(target_os = "windows")]
fn window_capture_bounds(hwnd: HWND) -> Option<CaptureBounds> {
    let mut rect = RECT::default();

    unsafe {
        GetWindowRect(hwnd, &mut rect).ok()?;
    }

    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;

    if width < 16 || height < 16 {
        return None;
    }

    Some(CaptureBounds {
        x: rect.left,
        y: rect.top,
        width: width as u32,
        height: height as u32,
    })
}

#[cfg(target_os = "windows")]
fn window_capture_plan_from_hwnd(
    hwnd: HWND,
    selected_title: &str,
) -> RecorderResult<(WindowCapturePlan, CaptureBounds)> {
    if hwnd.0.is_null() || !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return Err(RecorderError::new(
            "window_source_closed",
            "Selected window is closed or no longer exists",
            true,
        ));
    }

    if unsafe { IsIconic(hwnd).as_bool() } {
        return Err(RecorderError::new(
            "window_source_minimized",
            "Selected window is minimized. Restore it before recording.",
            true,
        ));
    }

    if !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return Err(RecorderError::new(
            "window_source_hidden",
            "Selected window is hidden and cannot be recorded",
            true,
        ));
    }

    let title = window_title(hwnd)
        .or_else(|| {
            let title = selected_title.trim();
            (!title.is_empty()).then(|| title.to_string())
        })
        .ok_or_else(|| {
            RecorderError::new(
                "window_source_missing_title",
                "Selected window does not have a readable title",
                true,
            )
        })?;

    let bounds = window_capture_bounds(hwnd).ok_or_else(|| {
        RecorderError::new(
            "window_source_invalid_bounds",
            "Selected window does not have a valid recording size",
            true,
        )
    })?;

    let capture_window = WindowsCaptureWindow::from_raw_hwnd(hwnd.0 as *mut c_void);
    if !capture_window.is_valid() {
        return Err(RecorderError::new(
            "window_capture_unavailable",
            "Selected window is not available to Windows Graphics Capture",
            true,
        ));
    }

    let width = capture_window.width().map_err(|error| {
        RecorderError::new(
            "window_capture_size_failed",
            format!("Could not read selected window width: {error}"),
            true,
        )
    })?;
    let height = capture_window.height().map_err(|error| {
        RecorderError::new(
            "window_capture_size_failed",
            format!("Could not read selected window height: {error}"),
            true,
        )
    })?;
    let width = u32::try_from(width).map_err(|_| {
        RecorderError::new(
            "window_capture_invalid_size",
            format!("Selected window width is invalid: {width}"),
            true,
        )
    })?;
    let height = u32::try_from(height).map_err(|_| {
        RecorderError::new(
            "window_capture_invalid_size",
            format!("Selected window height is invalid: {height}"),
            true,
        )
    })?;

    if width < 16 || height < 16 {
        return Err(RecorderError::new(
            "window_capture_invalid_size",
            format!("Selected window is too small to record: {width}x{height}"),
            true,
        ));
    }

    Ok((
        WindowCapturePlan {
            hwnd: hwnd.0 as usize,
            title,
            width,
            height,
        },
        bounds,
    ))
}

#[cfg(target_os = "windows")]
fn parse_window_handle(value: &str) -> RecorderResult<HWND> {
    let trimmed = value.trim();
    let parsed = if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        usize::from_str_radix(hex, 16)
    } else {
        trimmed.parse::<usize>()
    }
    .map_err(|error| {
        RecorderError::new(
            "invalid_window_source",
            format!("Selected window id is invalid: {error}"),
            true,
        )
    })?;

    if parsed == 0 {
        return Err(RecorderError::new(
            "invalid_window_source",
            "Selected window id is empty",
            true,
        ));
    }

    Ok(HWND(parsed as *mut _))
}

#[cfg(target_os = "windows")]
fn window_id(hwnd: HWND) -> String {
    format!("0x{:X}", hwnd.0 as usize)
}

fn ffmpeg_sidecar_command(app: &AppHandle) -> RecorderResult<Command> {
    let command = app.shell().sidecar(FFMPEG_SIDECAR_NAME).map_err(|error| {
        RecorderError::new(
            "ffmpeg_sidecar_unavailable",
            format!("Could not resolve bundled FFmpeg sidecar: {error}"),
            true,
        )
    })?;

    Ok(command.into())
}

fn log_recording_diagnostics(session_dir: &Path, ffmpeg_path: &OsStr) {
    let current_dir = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|error| format!("<unavailable: {error}>"));
    let output_dir = session_dir.parent().unwrap_or(session_dir);

    println!("[FocusFlow recorder] Current working directory: {current_dir}");
    println!(
        "[FocusFlow recorder] FFmpeg executable path: {}",
        ffmpeg_path.to_string_lossy()
    );
    println!(
        "[FocusFlow recorder] Output directory path: {}",
        output_dir.display()
    );
    println!(
        "[FocusFlow recorder] Recording session directory path: {}",
        session_dir.display()
    );
}

fn log_window_capture_bounds(label: &str, bounds: CaptureBounds) {
    println!("[FocusFlow recorder] {label}:");
    println!("[FocusFlow recorder] x={}", bounds.x);
    println!("[FocusFlow recorder] y={}", bounds.y);
    println!("[FocusFlow recorder] width={}", bounds.width);
    println!("[FocusFlow recorder] height={}", bounds.height);
}

fn log_ffmpeg_command(ffmpeg_path: &OsStr, args: &[String]) {
    println!(
        "[FocusFlow recorder] FFmpeg command: {}",
        command_string(ffmpeg_path, args)
    );
}

fn command_string(program: &OsStr, args: &[String]) -> String {
    std::iter::once(program.to_string_lossy().to_string())
        .chain(args.iter().map(|arg| quote_command_arg(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_command_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }

    if !arg
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '"' | '\''))
    {
        return arg.to_string();
    }

    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');

    for character in arg.chars() {
        if character == '"' {
            quoted.push('\\');
        }
        quoted.push(character);
    }

    quoted.push('"');
    quoted
}

fn timestamp_folder_name() -> RecorderResult<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            RecorderError::new(
                "system_time_invalid",
                format!("System time is before the Unix epoch: {error}"),
                true,
            )
        })?;
    let (year, month, day, hour, minute, second) =
        unix_seconds_to_utc_components(now.as_secs() as i64);

    Ok(format!(
        "{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}_{millis:03}",
        millis = now.subsec_millis()
    ))
}

fn unix_seconds_to_utc_components(seconds: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;
    let second = (seconds_of_day % 60) as u32;

    (year, month, day, hour, minute, second)
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_phase = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_phase + 2) / 5 + 1;
    let month = month_phase + if month_phase < 10 { 3 } else { -9 };
    let year = year + if month <= 2 { 1 } else { 0 };

    (year as i32, month as u32, day as u32)
}

fn ensure_windows() -> RecorderResult<()> {
    if cfg!(target_os = "windows") {
        Ok(())
    } else {
        Err(RecorderError::new(
            "unsupported_platform",
            "This recorder implementation is Windows-only",
            false,
        ))
    }
}

fn append_stdout(log: &Arc<Mutex<ProcessLog>>, bytes: Vec<u8>) {
    if let Ok(mut log) = log.lock() {
        log.push_stdout(bytes);
    }
}

fn append_stderr(log: &Arc<Mutex<ProcessLog>>, bytes: Vec<u8>) {
    if let Ok(mut log) = log.lock() {
        log.push_stderr(bytes);
    }
}

fn read_log_text(log: &Arc<Mutex<ProcessLog>>) -> String {
    log.lock().map(|log| log.stderr_text()).unwrap_or_else(|_| {
        "Could not read FFmpeg log because the log lock was poisoned".to_string()
    })
}

fn push_bounded(target: &mut Vec<u8>, bytes: Vec<u8>) {
    if bytes.len() >= MAX_CAPTURED_LOG_BYTES {
        target.clear();
        target.extend_from_slice(&bytes[bytes.len() - MAX_CAPTURED_LOG_BYTES..]);
        return;
    }

    let overflow = target
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_CAPTURED_LOG_BYTES);

    if overflow > 0 {
        target.drain(..overflow);
    }

    target.extend_from_slice(&bytes);
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn elapsed_seconds(started_at: Instant) -> f64 {
    started_at.elapsed().as_millis() as f64 / 1000.0
}

fn path_to_string(path: &Path) -> RecorderResult<String> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        RecorderError::new(
            "non_utf8_path",
            format!("Path is not valid UTF-8: {}", path.display()),
            false,
        )
    })
}

fn ffmpeg_exit_error(
    exit: ProcessExit,
    stop_signal_error: Option<String>,
    stderr: &str,
) -> RecorderError {
    let mut message = match exit.code {
        Some(code) => format!("FFmpeg exited with non-zero code {code}"),
        None => "FFmpeg exited without an exit code".to_string(),
    };

    if let Some(error) = exit.event_error {
        message.push_str("; process event error: ");
        message.push_str(&error);
    }

    if let Some(error) = stop_signal_error {
        message.push_str("; stop signal error: ");
        message.push_str(&error);
    }

    if !stderr.is_empty() {
        message.push_str("; stderr: ");
        message.push_str(stderr);
    }

    RecorderError::new("ffmpeg_recording_failed", message, true)
}

fn stop_timeout_error(
    wait_error: RecorderError,
    kill_error: Option<String>,
    stderr: &str,
) -> RecorderError {
    let mut message = wait_error.message;

    if let Some(error) = kill_error {
        message.push_str("; additionally failed to kill FFmpeg: ");
        message.push_str(&error);
    }

    if !stderr.is_empty() {
        message.push_str("; stderr: ");
        message.push_str(stderr);
    }

    RecorderError::new(wait_error.code, message, true)
}
