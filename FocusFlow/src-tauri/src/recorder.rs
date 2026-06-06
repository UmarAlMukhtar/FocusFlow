use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
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

#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::POINT,
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON},
        WindowsAndMessaging::GetCursorPos,
    },
};

const CLICKS_FILE_NAME: &str = "clicks.json";
const DRAGS_FILE_NAME: &str = "drags.json";
const FFMPEG_SIDECAR_NAME: &str = "ffmpeg";
const OUTPUT_FILE_NAME: &str = "screen.mp4";
const OUTPUT_RECORDINGS_DIR_NAME: &str = "Recordings";
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
    pid: u32,
    child: Child,
    click_tracker: ClickTracker,
    output_path: PathBuf,
    started_at: Instant,
    monitor: MonitorInfo,
    log: Arc<Mutex<ProcessLog>>,
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

impl ClickTracker {
    fn start(started_at: Instant) -> ClickTracker {
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
                    left_drag = push_click(&worker_clicks, started_at, ClickButton::Left)
                        .map(|point| PendingDrag::new(ClickButton::Left, point));
                } else if left_is_down {
                    update_pending_drag(&mut left_drag, started_at);
                } else if left_was_down {
                    finish_pending_drag(&mut left_drag, &worker_drags, started_at);
                }

                if right_is_down && !right_was_down {
                    right_drag = push_click(&worker_clicks, started_at, ClickButton::Right)
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
) -> RecorderResult<RecordingStatus> {
    start_primary_monitor_recording(&app, &state).await
}

#[tauri::command]
pub async fn stop_recording(state: State<'_, RecorderState>) -> RecorderResult<RecordingStatus> {
    stop_active_recording(&state).await
}

#[tauri::command]
pub fn recording_status(
    app: AppHandle,
    state: State<'_, RecorderState>,
) -> RecorderResult<RecordingStatus> {
    current_recording_status(&app, &state)
}

async fn start_primary_monitor_recording(
    app: &AppHandle,
    state: &RecorderState,
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

    let monitor = primary_monitor_info(app)?;
    let args = ffmpeg_primary_monitor_args(&monitor, &output_path)?;
    let mut command = ffmpeg_sidecar_command(app)?;
    let ffmpeg_path = command.get_program().to_os_string();
    log_recording_diagnostics(output_dir, &ffmpeg_path);
    command
        .args(args)
        .current_dir(output_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| {
            RecorderError::new(
                "ffmpeg_spawn_failed",
                format!("Could not start system FFmpeg screen recorder: {error}"),
                true,
            )
        })?;

    let pid = child.id();
    let log = Arc::new(Mutex::new(ProcessLog::default()));

    if let Some(stdout) = child.stdout.take() {
        spawn_output_reader(stdout, Arc::clone(&log), ProcessStream::Stdout);
    }

    if let Some(stderr) = child.stderr.take() {
        spawn_output_reader(stderr, Arc::clone(&log), ProcessStream::Stderr);
    }

    let started_at = Instant::now();
    let click_tracker = ClickTracker::start(started_at);

    let active = ActiveRecording {
        pid,
        child,
        click_tracker,
        output_path: output_path.clone(),
        started_at,
        monitor: monitor.clone(),
        log,
    };

    let mut runtime = state.lock_runtime()?;
    runtime.last_completed = None;
    runtime.active = Some(active);

    Ok(RecordingStatus {
        phase: RecordingPhase::Recording,
        output_path: path_to_string(&output_path)?,
        pid: Some(pid),
        monitor: Some(monitor),
        elapsed_ms: 0,
        file_size_bytes: None,
    })
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
        mut child,
        click_tracker,
        output_path,
        started_at,
        monitor,
        log,
        ..
    } = active;

    click_tracker.request_stop();

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
    let interactions = click_tracker.finish();

    match exit {
        Ok(process_exit) => {
            if process_exit.code != Some(0) {
                state.clear_finalizing()?;
                return Err(ffmpeg_exit_error(
                    process_exit,
                    stop_signal_error,
                    &read_log_text(&log),
                ));
            }
        }
        Err(wait_error) => {
            state.clear_finalizing()?;
            return Err(stop_timeout_error(wait_error, None, &read_log_text(&log)));
        }
    }

    let interactions = match interactions {
        Ok(interactions) => interactions,
        Err(error) => {
            state.clear_finalizing()?;
            return Err(error);
        }
    };

    let metadata = match fs::metadata(&output_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            state.clear_finalizing()?;
            return Err(RecorderError::new(
                "recording_output_missing",
                format!("FFmpeg exited successfully, but screen.mp4 was not readable: {error}"),
                true,
            ));
        }
    };

    if metadata.len() == 0 {
        state.clear_finalizing()?;
        return Err(RecorderError::new(
            "recording_output_empty",
            "FFmpeg exited successfully, but screen.mp4 is empty",
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
            pid: Some(active.pid),
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
) -> Option<DragPoint> {
    let point = current_drag_point(started_at)?;

    if let Ok(mut clicks) = clicks.lock() {
        clicks.push(ClickEvent {
            timestamp: point.timestamp,
            x: point.x,
            y: point.y,
            button,
        });
    }

    Some(point)
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
    let mut timeline = build_click_timeline_segments(clicks);
    timeline.extend(build_drag_timeline_segments(drags));
    timeline.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    timeline
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

fn session_output_dir(output_path: &Path) -> RecorderResult<&Path> {
    output_path.parent().ok_or_else(|| {
        RecorderError::new(
            "invalid_session_output_path",
            "Could not resolve a parent directory for recording session assets",
            true,
        )
    })
}

fn ffmpeg_primary_monitor_args(
    monitor: &MonitorInfo,
    output_path: &Path,
) -> RecorderResult<Vec<String>> {
    let output_path = path_to_string(output_path)?;
    let video_size = format!("{}x{}", monitor.width, monitor.height);

    Ok(vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "info".to_string(),
        "-f".to_string(),
        "gdigrab".to_string(),
        "-framerate".to_string(),
        TARGET_FPS.to_string(),
        "-offset_x".to_string(),
        monitor.x.to_string(),
        "-offset_y".to_string(),
        monitor.y.to_string(),
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

fn ffmpeg_sidecar_command(app: &AppHandle) -> RecorderResult<Command> {
    let command = app
        .shell()
        .sidecar(FFMPEG_SIDECAR_NAME)
        .map_err(|error| {
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
