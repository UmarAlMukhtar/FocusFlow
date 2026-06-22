use serde::{Deserialize, Serialize};
use std::{
    fmt, fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::ShellExt;

const CLICKS_FILE_NAME: &str = "clicks.json";
const CLICK_INDICATOR_ALPHA: f64 = 0.9;
const CLICK_INDICATOR_DURATION_SECONDS: f64 = 0.3;
const CLICK_INDICATOR_FONT_FILE: &str = "C\\:/Windows/Fonts/segoeui.ttf";
const CLICK_INDICATOR_FONT_SIZE: u32 = 56;
const CLICK_INDICATOR_GLYPH: &str = "\u{25CB}";
const EDITED_FILE_NAME: &str = "edited.mp4";
const EXPORT_PROGRESS_EVENT: &str = "export-progress";
const FFMPEG_SIDECAR_NAME: &str = "ffmpeg";
const FILTER_SCRIPT_FILE_NAME: &str = "edited.filter_complex.txt";
const MAX_CAPTURED_LOG_BYTES: usize = 96 * 1024;
const OUTPUT_FILE_NAME: &str = "screen.mp4";
const DEFAULT_TARGET_FPS: u32 = 30;
const DEFAULT_CRF: u8 = 25;
const TIMELINE_FILE_NAME: &str = "timeline.json";
const ZOOM_IN_MS: u64 = 180;
const ZOOM_OUT_MS: u64 = 220;
const ZOOM_HOLD_MS: u64 = 150;
const ZOOM_SCALE: f64 = 2.3;
const PAN_TRANSITION_MS: u64 = 220;
/// Brief hold added after the camera reaches a pan target before the next
/// movement starts.  Prevents the camera from feeling like it is constantly
/// rubber-banding between targets.
const CAMERA_SETTLE_MS: u64 = 120;
const EXPRESSION_EPSILON: f64 = 0.000_001;
const MIN_CAMERA_INTERVAL_SECONDS: f64 = 0.01;

pub type ExportCommandResult<T> = Result<T, ExportError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportStatus {
    pub input_path: String,
    pub timeline_path: String,
    pub output_path: String,
    pub segment_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgress {
    percentage: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    pub zoom_scale: Option<f64>,
    pub zoom_in_ms: Option<f64>,
    pub zoom_out_ms: Option<f64>,
    pub pan_transition_ms: Option<f64>,
    pub preset: Option<ExportPreset>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportPreset {
    SmallFile,
    Balanced,
    HighQuality,
}

#[derive(Debug, Clone, Copy)]
struct ExportConfig {
    zoom_scale: f64,
    zoom_in_ms: u64,
    zoom_out_ms: u64,
    zoom_hold_ms: u64,
    pan_transition_ms: u64,
    /// Hold duration added after each pan completes before the next movement.
    camera_settle_ms: u64,
    crf: u8,
    fps: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl ExportError {
    fn new(code: impl Into<String>, message: impl Into<String>, recoverable: bool) -> ExportError {
        ExportError {
            code: code.into(),
            message: message.into(),
            recoverable,
        }
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ExportError {}

impl ExportConfig {
    fn from_settings(settings: Option<ExportSettings>) -> ExportConfig {
        let Some(settings) = settings else {
            return ExportConfig::default();
        };
        let defaults = ExportConfig::default();

        ExportConfig {
            zoom_scale: positive_f64_or(settings.zoom_scale, defaults.zoom_scale),
            zoom_in_ms: positive_milliseconds_or(settings.zoom_in_ms, defaults.zoom_in_ms),
            zoom_out_ms: positive_milliseconds_or(settings.zoom_out_ms, defaults.zoom_out_ms),
            zoom_hold_ms: defaults.zoom_hold_ms,
            pan_transition_ms: positive_milliseconds_or(
                settings.pan_transition_ms,
                defaults.pan_transition_ms,
            ),
            camera_settle_ms: defaults.camera_settle_ms,
            ..ExportPreset::config(settings.preset.unwrap_or_default())
        }
    }

    fn zoom_in_seconds(self) -> f64 {
        milliseconds_to_seconds(self.zoom_in_ms)
    }

    fn zoom_out_seconds(self) -> f64 {
        milliseconds_to_seconds(self.zoom_out_ms)
    }

    fn zoom_hold_seconds(self) -> f64 {
        milliseconds_to_seconds(self.zoom_hold_ms)
    }

    fn pan_transition_seconds(self) -> f64 {
        milliseconds_to_seconds(self.pan_transition_ms)
    }

    /// Brief hold after a pan lands; prevents immediate chained movement.
    fn camera_settle_seconds(self) -> f64 {
        milliseconds_to_seconds(self.camera_settle_ms)
    }
}

impl Default for ExportConfig {
    fn default() -> ExportConfig {
        ExportConfig {
            zoom_scale: ZOOM_SCALE,
            zoom_in_ms: ZOOM_IN_MS,
            zoom_out_ms: ZOOM_OUT_MS,
            zoom_hold_ms: ZOOM_HOLD_MS,
            pan_transition_ms: PAN_TRANSITION_MS,
            camera_settle_ms: CAMERA_SETTLE_MS,
            crf: DEFAULT_CRF,
            fps: DEFAULT_TARGET_FPS,
        }
    }
}

impl Default for ExportPreset {
    fn default() -> ExportPreset {
        ExportPreset::Balanced
    }
}

impl ExportPreset {
    fn config(self) -> ExportConfig {
        let defaults = ExportConfig::default();
        let (crf, fps) = match self {
            ExportPreset::SmallFile => (28, 30),
            ExportPreset::Balanced => (25, 30),
            ExportPreset::HighQuality => (22, 60),
        };

        ExportConfig {
            crf,
            fps,
            ..defaults
        }
    }
}

fn positive_f64_or(value: Option<f64>, default: f64) -> f64 {
    value
        .filter(|value| value.is_finite() && *value > 1.0)
        .unwrap_or(default)
}

fn positive_milliseconds_or(value: Option<f64>, default: u64) -> u64 {
    value
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.round() as u64)
        .unwrap_or(default)
}

#[derive(Debug, Clone, Deserialize)]
struct TimelineSegment {
    start: f64,
    end: f64,
    x: i32,
    y: i32,
    scale: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct ClickEvent {
    timestamp: f64,
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy)]
struct VideoInfo {
    width: u32,
    height: u32,
    duration: f64,
}

#[derive(Debug, Clone)]
struct TimelineSequence {
    start: f64,
    end: f64,
    targets: Vec<TimelineSegment>,
}

#[derive(Debug, Clone, Copy)]
struct CameraKeyframe {
    time: f64,
    x: f64,
    y: f64,
    zoom: f64,
}

#[derive(Debug, Clone, Copy)]
struct CameraInterval {
    start: f64,
    end: f64,
    from: CameraKeyframe,
    to: CameraKeyframe,
}

#[tauri::command]
pub async fn export_edited_mp4(
    app: AppHandle,
    settings: Option<ExportSettings>,
) -> ExportCommandResult<ExportStatus> {
    let paths = ExportPaths::resolve(&app)?;
    let config = ExportConfig::from_settings(settings);

    tauri::async_runtime::spawn_blocking(move || export_edited_mp4_blocking(app, paths, config))
        .await
        .map_err(|error| {
            ExportError::new(
                "export_join_failed",
                format!("Could not join export task: {error}"),
                true,
            )
        })?
}

fn export_edited_mp4_blocking(
    app: AppHandle,
    paths: ExportPaths,
    config: ExportConfig,
) -> ExportCommandResult<ExportStatus> {
    ensure_windows()?;
    ensure_input_exists(&paths.input_path, "screen.mp4")?;
    ensure_input_exists(&paths.timeline_path, "timeline.json")?;
    ensure_input_exists(&paths.clicks_path, "clicks.json")?;

    let timeline = read_timeline(&paths.timeline_path)?;
    let clicks = read_clicks(&paths.clicks_path)?;
    let video_info = probe_video_info(&app, &paths.input_path)?;

    // Diagnostics: input
    let input_size_kb = fs::metadata(&paths.input_path)
        .map(|m| m.len() / 1024)
        .unwrap_or(0);
    println!(
        "[FocusFlow export] input duration: {:.3}s  size: {} KB  timeline segments: {}",
        video_info.duration,
        input_size_kb,
        timeline.len()
    );

    if paths.temp_output_path.exists() {
        remove_file(&paths.temp_output_path)?;
    }

    emit_export_progress(&app, 0.0);

    if timeline.is_empty() && clicks.is_empty() {
        run_ffmpeg_copy(&app, &paths, video_info.duration, config)?;
    } else {
        let filter = build_export_filter(&timeline, &clicks, video_info, config);
        let filter_bytes = filter.len() as u64;
        write_filter_script(&paths.filter_script_path, &filter)?;
        println!(
            "[FocusFlow export] filter script size: {} bytes  expected output duration: ≈{:.3}s",
            filter_bytes,
            video_info.duration
        );
        let export_result = run_ffmpeg_zoom_export(&app, &paths, video_info.duration, config);
        if let Err(error) = remove_filter_script(&paths) {
            eprintln!("{error}");
        }
        export_result?;
    }

    replace_output_file(&paths.temp_output_path, &paths.output_path)?;
    mark_session_exported(&paths)?;
    emit_export_progress(&app, 100.0);

    // Diagnostics: output
    let output_size_kb = fs::metadata(&paths.output_path)
        .map(|m| m.len() / 1024)
        .unwrap_or(0);
    println!(
        "[FocusFlow export] output size: {} KB  path: {}",
        output_size_kb,
        paths.output_path.display()
    );

    export_edited_mp4_ok_result(&paths, timeline.len())
}

fn export_edited_mp4_ok_result(
    paths: &ExportPaths,
    timeline_len: usize,
) -> ExportCommandResult<ExportStatus> {
    Ok(ExportStatus {
        input_path: path_to_string(&paths.input_path)?,
        timeline_path: path_to_string(&paths.timeline_path)?,
        output_path: path_to_string(&paths.output_path)?,
        segment_count: timeline_len,
    })
}

#[derive(Debug, Clone)]
struct ExportPaths {
    session_dir: PathBuf,
    input_path: PathBuf,
    timeline_path: PathBuf,
    clicks_path: PathBuf,
    output_path: PathBuf,
    temp_output_path: PathBuf,
    filter_script_path: PathBuf,
}

impl ExportPaths {
    fn resolve(app: &AppHandle) -> ExportCommandResult<ExportPaths> {
        let recordings_dir = crate::recorder::recordings_root_dir(app).map_err(|error| {
            ExportError::new(
                error.code,
                format!(
                    "Could not resolve recording storage directory: {}",
                    error.message
                ),
                error.recoverable,
            )
        })?;
        let session_dir = latest_session_dir(&recordings_dir)?;

        Ok(ExportPaths::from_session_dir(session_dir))
    }

    fn from_session_dir(session_dir: PathBuf) -> ExportPaths {
        ExportPaths {
            session_dir: session_dir.clone(),
            input_path: session_dir.join(OUTPUT_FILE_NAME),
            timeline_path: session_dir.join(TIMELINE_FILE_NAME),
            clicks_path: session_dir.join(CLICKS_FILE_NAME),
            output_path: session_dir.join(EDITED_FILE_NAME),
            temp_output_path: session_dir.join("edited.tmp.mp4"),
            filter_script_path: session_dir.join(FILTER_SCRIPT_FILE_NAME),
        }
    }
}

fn mark_session_exported(paths: &ExportPaths) -> ExportCommandResult<()> {
    crate::recorder::mark_session_exported(&paths.session_dir).map_err(|error| {
        ExportError::new(
            error.code,
            format!(
                "Could not update session export metadata: {}",
                error.message
            ),
            error.recoverable,
        )
    })
}

fn latest_session_dir(recordings_dir: &Path) -> ExportCommandResult<PathBuf> {
    let mut candidates = Vec::new();

    for entry in fs::read_dir(recordings_dir).map_err(|error| {
        ExportError::new(
            "read_recordings_dir_failed",
            format!("Could not read recordings directory: {error}"),
            true,
        )
    })? {
        let entry = entry.map_err(|error| {
            ExportError::new(
                "read_recording_entry_failed",
                format!("Could not read recording directory entry: {error}"),
                true,
            )
        })?;
        let path = entry.path();

        if !entry
            .file_type()
            .map_err(|error| {
                ExportError::new(
                    "read_recording_entry_type_failed",
                    format!("Could not read recording entry type: {error}"),
                    true,
                )
            })?
            .is_dir()
        {
            continue;
        }

        if path.join(OUTPUT_FILE_NAME).exists()
            && path.join(TIMELINE_FILE_NAME).exists()
            && path.join(CLICKS_FILE_NAME).exists()
        {
            candidates.push((modified_time(&path), path));
        }
    }

    candidates
        .into_iter()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
        .ok_or_else(|| {
            ExportError::new(
                "recording_session_missing",
                format!(
                    "No recording session containing screen.mp4, clicks.json, and timeline.json was found in {}",
                    recordings_dir.display()
                ),
                true,
            )
        })
}

fn modified_time(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

fn read_timeline(path: &Path) -> ExportCommandResult<Vec<TimelineSegment>> {
    let bytes = fs::read(path).map_err(|error| {
        ExportError::new(
            "read_timeline_failed",
            format!("Could not read timeline.json: {error}"),
            true,
        )
    })?;
    let mut timeline: Vec<TimelineSegment> = serde_json::from_slice(&bytes).map_err(|error| {
        ExportError::new(
            "parse_timeline_failed",
            format!("Could not parse timeline.json: {error}"),
            true,
        )
    })?;

    timeline.retain(|segment| {
        segment.start.is_finite()
            && segment.end.is_finite()
            && segment.scale.is_finite()
            && segment.end > segment.start
            && segment.scale > 1.0
    });
    timeline.sort_by(|left, right| {
        left.start
            .partial_cmp(&right.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(timeline)
}

fn read_clicks(path: &Path) -> ExportCommandResult<Vec<ClickEvent>> {
    let bytes = fs::read(path).map_err(|error| {
        ExportError::new(
            "read_clicks_failed",
            format!("Could not read clicks.json: {error}"),
            true,
        )
    })?;
    let mut clicks: Vec<ClickEvent> = serde_json::from_slice(&bytes).map_err(|error| {
        ExportError::new(
            "parse_clicks_failed",
            format!("Could not parse clicks.json: {error}"),
            true,
        )
    })?;

    clicks.retain(|click| click.timestamp.is_finite() && click.timestamp >= 0.0);
    clicks.sort_by(|left, right| {
        left.timestamp
            .partial_cmp(&right.timestamp)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(clicks)
}

fn probe_video_info(app: &AppHandle, input_path: &Path) -> ExportCommandResult<VideoInfo> {
    let output = ffmpeg_sidecar_output(
        app,
        [
            "-hide_banner".to_string(),
            "-i".to_string(),
            path_to_string(input_path)?,
            "-frames:v".to_string(),
            "1".to_string(),
            "-f".to_string(),
            "null".to_string(),
            "-".to_string(),
        ],
        "ffmpeg_probe_spawn_failed",
        "Could not start bundled FFmpeg metadata probe",
    )?;

    if !output.status.success() {
        return Err(ExportError::new(
            "ffmpeg_probe_failed",
            format!(
                "Bundled FFmpeg metadata probe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            true,
        ));
    }

    let output_text = String::from_utf8_lossy(&output.stderr);
    let duration = parse_ffmpeg_duration_seconds(&output_text).ok_or_else(|| {
        ExportError::new(
            "video_duration_missing",
            "Bundled FFmpeg did not return a usable video duration",
            true,
        )
    })?;
    let (width, height) = parse_ffmpeg_video_dimensions(&output_text).ok_or_else(|| {
        ExportError::new(
            "video_dimensions_missing",
            "Bundled FFmpeg did not return usable video dimensions",
            true,
        )
    })?;

    Ok(VideoInfo {
        width,
        height,
        duration,
    })
}

fn parse_ffmpeg_duration_seconds(output: &str) -> Option<f64> {
    for line in output.lines() {
        let Some((_, duration)) = line.split_once("Duration:") else {
            continue;
        };
        let duration = duration.split(',').next()?.trim();

        if let Some(seconds) = parse_ffmpeg_timestamp(duration) {
            return Some(seconds);
        }
    }

    None
}

fn parse_ffmpeg_timestamp(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;

    if parts.next().is_some() {
        return None;
    }

    let total = hours * 3600.0 + minutes * 60.0 + seconds;

    if total.is_finite() && total > 0.0 {
        Some(total)
    } else {
        None
    }
}

fn parse_ffmpeg_video_dimensions(output: &str) -> Option<(u32, u32)> {
    output
        .lines()
        .filter(|line| line.contains("Video:"))
        .find_map(parse_video_dimensions_from_line)
}

fn parse_video_dimensions_from_line(line: &str) -> Option<(u32, u32)> {
    for token in line
        .split(|character: char| character.is_whitespace() || matches!(character, ',' | '[' | ']'))
    {
        let Some((width, height)) = token.split_once('x') else {
            continue;
        };
        let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) else {
            continue;
        };

        if width >= 16 && height >= 16 {
            return Some((width, height));
        }
    }

    None
}

fn ffmpeg_sidecar_output<I>(
    app: &AppHandle,
    args: I,
    spawn_code: &'static str,
    spawn_message: &'static str,
) -> ExportCommandResult<Output>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let mut command = ffmpeg_sidecar_command(app)?;

    command
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| ExportError::new(spawn_code, format!("{spawn_message}: {error}"), true))
}

fn ffmpeg_sidecar_output_with_progress(
    app: &AppHandle,
    args: Vec<String>,
    duration_seconds: f64,
) -> ExportCommandResult<Output> {
    let mut command = ffmpeg_sidecar_command(app)?;

    command
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| {
        ExportError::new(
            "ffmpeg_spawn_failed",
            format!("Could not start bundled FFmpeg export: {error}"),
            true,
        )
    })?;

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            return Err(ExportError::new(
                "ffmpeg_stdout_unavailable",
                "Bundled FFmpeg stdout was not available for progress events",
                true,
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            return Err(ExportError::new(
                "ffmpeg_stderr_unavailable",
                "Bundled FFmpeg stderr was not available for export diagnostics",
                true,
            ));
        }
    };
    let stderr_log = Arc::new(Mutex::new(Vec::new()));
    let stderr_reader = spawn_log_reader(stderr, Arc::clone(&stderr_log));
    let progress_read_error = read_ffmpeg_progress(app, stdout, duration_seconds).err();
    let status = child.wait().map_err(|error| {
        ExportError::new(
            "ffmpeg_wait_failed",
            format!("Could not wait for bundled FFmpeg export: {error}"),
            true,
        )
    })?;
    let stderr = join_log_reader(stderr_reader, stderr_log);

    if status.success() {
        if let Some(error) = progress_read_error {
            return Err(error);
        }
    }

    Ok(Output {
        status,
        stdout: Vec::new(),
        stderr,
    })
}

fn read_ffmpeg_progress<R>(
    app: &AppHandle,
    reader: R,
    duration_seconds: f64,
) -> ExportCommandResult<()>
where
    R: Read,
{
    let mut reader = BufReader::new(reader);
    let mut parser = FfmpegProgressParser::new(duration_seconds);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).map_err(|error| {
            ExportError::new(
                "ffmpeg_progress_read_failed",
                format!("Could not read bundled FFmpeg export progress: {error}"),
                true,
            )
        })?;

        if bytes_read == 0 {
            break;
        }

        if let Some(percentage) = parser.parse_line(&line) {
            emit_export_progress(app, percentage);
        }
    }

    Ok(())
}

struct FfmpegProgressParser {
    duration_seconds: f64,
    last_emitted_percentage: Option<f64>,
}

impl FfmpegProgressParser {
    fn new(duration_seconds: f64) -> FfmpegProgressParser {
        FfmpegProgressParser {
            duration_seconds,
            last_emitted_percentage: None,
        }
    }

    fn parse_line(&mut self, line: &str) -> Option<f64> {
        let (key, value) = line.trim().split_once('=')?;
        let output_seconds = parse_ffmpeg_progress_seconds(key, value)?;
        let percentage = progress_percentage(output_seconds, self.duration_seconds)?;

        if self.should_emit(percentage) {
            self.last_emitted_percentage = Some(percentage);
            Some(percentage)
        } else {
            None
        }
    }

    fn should_emit(&self, percentage: f64) -> bool {
        match self.last_emitted_percentage {
            Some(last) => percentage > last + 0.05,
            None => true,
        }
    }
}

fn parse_ffmpeg_progress_seconds(key: &str, value: &str) -> Option<f64> {
    match key {
        "out_time_us" | "out_time_ms" => {
            let micros = value.trim().parse::<f64>().ok()?;
            let seconds = micros / 1_000_000.0;

            if seconds.is_finite() && seconds >= 0.0 {
                Some(seconds)
            } else {
                None
            }
        }
        "out_time" => parse_ffmpeg_progress_timestamp(value.trim()),
        _ => None,
    }
}

fn parse_ffmpeg_progress_timestamp(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<f64>().ok()?;
    let minutes = parts.next()?.parse::<f64>().ok()?;
    let seconds = parts.next()?.parse::<f64>().ok()?;

    if parts.next().is_some() {
        return None;
    }

    let total = hours * 3600.0 + minutes * 60.0 + seconds;

    if total.is_finite() && total >= 0.0 {
        Some(total)
    } else {
        None
    }
}

fn progress_percentage(output_seconds: f64, duration_seconds: f64) -> Option<f64> {
    if !output_seconds.is_finite() || !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return None;
    }

    Some(((output_seconds / duration_seconds) * 100.0).clamp(0.0, 99.9))
}

fn emit_export_progress(app: &AppHandle, percentage: f64) {
    let percentage = percentage.clamp(0.0, 100.0);

    if let Err(error) = app.emit(EXPORT_PROGRESS_EVENT, ExportProgress { percentage }) {
        eprintln!("Could not emit export progress event: {error}");
    }
}

fn spawn_log_reader<R>(mut reader: R, log: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => append_bounded_log(&log, &buffer[..bytes_read]),
                Err(error) => {
                    append_bounded_log(
                        &log,
                        format!("Failed to read FFmpeg stderr: {error}").as_bytes(),
                    );
                    break;
                }
            }
        }
    })
}

fn join_log_reader(reader: JoinHandle<()>, log: Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    if reader.join().is_err() {
        append_bounded_log(&log, b"FFmpeg stderr reader thread panicked");
    }

    log.lock()
        .map(|log| log.clone())
        .unwrap_or_else(|_| b"Could not read FFmpeg stderr log".to_vec())
}

fn append_bounded_log(log: &Arc<Mutex<Vec<u8>>>, bytes: &[u8]) {
    let Ok(mut log) = log.lock() else {
        return;
    };

    if bytes.len() >= MAX_CAPTURED_LOG_BYTES {
        log.clear();
        log.extend_from_slice(&bytes[bytes.len() - MAX_CAPTURED_LOG_BYTES..]);
        return;
    }

    let overflow = log
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_CAPTURED_LOG_BYTES);

    if overflow > 0 {
        log.drain(..overflow);
    }

    log.extend_from_slice(bytes);
}

fn build_export_filter(
    timeline: &[TimelineSegment],
    clicks: &[ClickEvent],
    info: VideoInfo,
    config: ExportConfig,
) -> String {
    if timeline.is_empty() {
        let click_filters = click_indicator_filters_for_interval(clicks, 0.0, info.duration);

        return if click_filters.is_empty() {
            "[0:v]setsar=1[v]".to_string()
        } else {
            format!("[0:v]{click_filters},setsar=1[v]")
        };
    }

    let keyframes = build_camera_keyframes(timeline, info, config);
    println!(
        "[FocusFlow export] camera keyframe count: {}",
        keyframes.len()
    );

    let intervals = camera_intervals(&keyframes);
    println!(
        "[FocusFlow export] camera interval count: {}",
        intervals.len()
    );

    if intervals.is_empty() {
        let click_filters = click_indicator_filters_for_interval(clicks, 0.0, info.duration);

        return if click_filters.is_empty() {
            "[0:v]setsar=1[v]".to_string()
        } else {
            format!("[0:v]{click_filters},setsar=1[v]")
        };
    }

    // Count intervals that involve camera movement (zoom or position change)
    // for diagnostics.
    let moving_intervals = intervals
        .iter()
        .filter(|iv| {
            (iv.from.zoom - iv.to.zoom).abs() > EXPRESSION_EPSILON
                || (iv.from.x - iv.to.x).abs() > EXPRESSION_EPSILON
                || (iv.from.y - iv.to.y).abs() > EXPRESSION_EPSILON
        })
        .count();
    println!(
        "[FocusFlow export] smoothing intervals (moving): {moving_intervals} / {}",
        intervals.len()
    );

    build_interval_filter(&intervals, clicks, info, config)
}

fn build_interval_filter(
    intervals: &[CameraInterval],
    clicks: &[ClickEvent],
    info: VideoInfo,
    config: ExportConfig,
) -> String {
    let mut chains = Vec::with_capacity(intervals.len() + 1);
    let mut labels = Vec::with_capacity(intervals.len());

    for (index, interval) in intervals.iter().enumerate() {
        let output_label = if intervals.len() == 1 {
            "v".to_string()
        } else {
            format!("v{index}")
        };
        let click_filters =
            click_indicator_filters_for_interval(clicks, interval.start, interval.end);
        let mut chain = format!(
            "[0:v]trim=start={start}:end={end},setpts=PTS-STARTPTS",
            start = format_seconds(interval.start),
            end = format_seconds(interval.end)
        );

        if !click_filters.is_empty() {
            chain.push(',');
            chain.push_str(&click_filters);
        }

        chain.push(',');
        chain.push_str(&zoompan_interval_filter(
            interval,
            &output_label,
            info,
            config,
        ));
        chains.push(chain);

        if intervals.len() > 1 {
            labels.push(format!("[{output_label}]"));
        }
    }

    if intervals.len() > 1 {
        chains.push(format!(
            "{inputs}concat=n={count}:v=1:a=0[v]",
            inputs = labels.join(""),
            count = intervals.len()
        ));
    }

    chains.join(";")
}

fn zoompan_interval_filter(
    interval: &CameraInterval,
    output_label: &str,
    info: VideoInfo,
    config: ExportConfig,
) -> String {
    let duration = (interval.end - interval.start).max(MIN_CAMERA_INTERVAL_SECONDS);
    let zoom = interpolate_expression(interval.from.zoom, interval.to.zoom, duration);
    let x = interpolate_expression(interval.from.x, interval.to.x, duration);
    let y = interpolate_expression(interval.from.y, interval.to.y, duration);

    // IMPORTANT: d must always be 1 for video input.
    //
    // FFmpeg zoompan with a video source generates `d` output frames *per
    // input frame*, not `d` frames in total.  Setting d=N (where N is the
    // segment frame count) therefore multiplies the output length by N,
    // producing a file that is N× too long and too large.
    //
    // With d=1 each input frame produces exactly 1 output frame.  The
    // easing curve in the z/x/y expressions uses `out_time` (the output
    // presentation timestamp in seconds), which advances frame-by-frame at
    // the natural rate, so the smooth cosine interpolation works correctly.
    // Duration is controlled entirely by trim=start=...:end=... before
    // this filter in the chain.
    let filter = format!(
        "zoompan=z='{zoom}':x='clip(({x})-ow/(2*({zoom})),0,iw-ow/({zoom}))':y='clip(({y})-oh/(2*({zoom})),0,ih-oh/({zoom}))':d=1:s={width}x{height}:fps={fps},setsar=1[{output_label}]",
        zoom = zoom,
        x = x,
        y = y,
        width = info.width,
        height = info.height,
        fps = config.fps,
        output_label = output_label
    );

    // Safety check: confirm no d value other than 1 was generated.
    // A d>1 in a video segment filter is always a bug — log a warning so
    // it shows up immediately in the console.
    if !filter.contains(":d=1:") {
        eprintln!(
            "[FocusFlow export] WARNING: zoompan filter for interval [{:.3}, {:.3}] \
             does not contain d=1 — frame multiplication may occur!",
            interval.start, interval.end
        );
    }

    filter
}

fn click_indicator_filters_for_interval(
    clicks: &[ClickEvent],
    interval_start: f64,
    interval_end: f64,
) -> String {
    clicks
        .iter()
        .filter(|click| {
            click.timestamp < interval_end
                && click.timestamp + CLICK_INDICATOR_DURATION_SECONDS > interval_start
        })
        .map(|click| click_indicator_filter(click, interval_start))
        .collect::<Vec<_>>()
        .join(",")
}

fn click_indicator_filter(click: &ClickEvent, time_offset: f64) -> String {
    let start = click.timestamp - time_offset;
    let end = start + CLICK_INDICATOR_DURATION_SECONDS;
    let fade = format!(
        "{alpha}*(0.5+0.5*cos(PI*((t-({start}))/{duration})))",
        alpha = format_seconds(CLICK_INDICATOR_ALPHA),
        start = format_seconds(start),
        duration = format_seconds(CLICK_INDICATOR_DURATION_SECONDS)
    );

    format!(
        "drawtext=fontfile='{fontfile}':text='{glyph}':fontsize={fontsize}:fontcolor=white:alpha='if(between(t,{start},{end}),{fade},0)':x='{x}-text_w/2':y='{y}-text_h/2':enable='between(t,{start},{end})'",
        fontfile = CLICK_INDICATOR_FONT_FILE,
        glyph = CLICK_INDICATOR_GLYPH,
        fontsize = CLICK_INDICATOR_FONT_SIZE,
        start = format_seconds(start),
        end = format_seconds(end),
        fade = fade,
        x = click.x,
        y = click.y
    )
}

fn build_camera_keyframes(
    timeline: &[TimelineSegment],
    info: VideoInfo,
    config: ExportConfig,
) -> Vec<CameraKeyframe> {
    let mut keyframes = Vec::new();
    push_camera_keyframe(&mut keyframes, default_keyframe(0.0, info), info.duration);

    for sequence in timeline_sequences(timeline, config) {
        push_sequence_keyframes(&mut keyframes, &sequence, info, config);
    }

    push_camera_keyframe(
        &mut keyframes,
        default_keyframe(info.duration, info),
        info.duration,
    );

    normalize_camera_keyframes(keyframes, info)
}

fn push_sequence_keyframes(
    keyframes: &mut Vec<CameraKeyframe>,
    sequence: &TimelineSequence,
    info: VideoInfo,
    config: ExportConfig,
) {
    let Some(first_target) = sequence.targets.first() else {
        return;
    };

    let start = clamp_time(sequence.start, info.duration);
    let end = clamp_time(sequence.end, info.duration);

    if end <= start + EXPRESSION_EPSILON {
        return;
    }

    let scale = sequence_scale(sequence, config);
    let first_boundary = sequence
        .targets
        .get(1)
        .map(|target| clamp_time(target.start, info.duration))
        .unwrap_or(end)
        .max(start)
        .min(end);
    let zoom_in_duration = config
        .zoom_in_seconds()
        .min((first_boundary - start).max(MIN_CAMERA_INTERVAL_SECONDS))
        .min((end - start).max(MIN_CAMERA_INTERVAL_SECONDS));
    let zoom_in_end = (start + zoom_in_duration).min(end);

    push_camera_keyframe(keyframes, default_keyframe(start, info), info.duration);
    push_camera_keyframe(
        keyframes,
        target_keyframe(zoom_in_end, first_target, scale),
        info.duration,
    );

    for index in 1..sequence.targets.len() {
        let previous_target = &sequence.targets[index - 1];
        let current_target = &sequence.targets[index];
        let transition_start = clamp_time(current_target.start, info.duration)
            .max(start)
            .min(end);
        let next_boundary = sequence
            .targets
            .get(index + 1)
            .map(|target| clamp_time(target.start, info.duration))
            .unwrap_or(end)
            .max(transition_start)
            .min(end);

        // Guarantee the pan lasts at least pan_transition_seconds so very
        // short segment gaps can't collapse to zero frames.
        let available_for_pan = (next_boundary - transition_start).max(MIN_CAMERA_INTERVAL_SECONDS);
        let pan_duration = config
            .pan_transition_seconds()
            .min(available_for_pan)
            // Never shorter than the minimum camera interval regardless of
            // how tightly-packed the targets are.
            .max(MIN_CAMERA_INTERVAL_SECONDS);
        let pan_end = (transition_start + pan_duration).min(end);

        // Keyframe where we leave the previous target position.
        push_camera_keyframe(
            keyframes,
            target_keyframe(transition_start, previous_target, scale),
            info.duration,
        );
        // Keyframe where the pan arrives at the new target.
        push_camera_keyframe(
            keyframes,
            target_keyframe(pan_end, current_target, scale),
            info.duration,
        );

        // Optional settle hold: keep the camera stationary at the new target
        // for camera_settle_seconds before the next movement.  This removes
        // the rubber-banding effect when targets are close together.
        let settle_end = (pan_end + config.camera_settle_seconds()).min(end);
        if settle_end > pan_end + EXPRESSION_EPSILON {
            push_camera_keyframe(
                keyframes,
                target_keyframe(settle_end, current_target, scale),
                info.duration,
            );
        }
    }

    if let Some(last_target) = sequence.targets.last() {
        push_camera_keyframe(
            keyframes,
            target_keyframe(end, last_target, scale),
            info.duration,
        );
    }

    let zoom_out_end = clamp_time(sequence_complete_at(sequence, config), info.duration);

    if zoom_out_end > end + EXPRESSION_EPSILON {
        push_camera_keyframe(
            keyframes,
            default_keyframe(zoom_out_end, info),
            info.duration,
        );
    }
}

fn timeline_sequences(timeline: &[TimelineSegment], config: ExportConfig) -> Vec<TimelineSequence> {
    let mut sequences: Vec<TimelineSequence> = Vec::new();

    for segment in timeline.iter().cloned() {
        if let Some(sequence) = sequences.last_mut() {
            if segment.start <= sequence_complete_at(sequence, config) + EXPRESSION_EPSILON {
                sequence.end = sequence.end.max(segment.end);
                sequence.targets.push(segment);
                continue;
            }
        }

        sequences.push(TimelineSequence {
            start: segment.start,
            end: segment.end,
            targets: vec![segment],
        });
    }

    sequences
}

fn push_camera_keyframe(
    keyframes: &mut Vec<CameraKeyframe>,
    keyframe: CameraKeyframe,
    duration: f64,
) {
    if !keyframe.time.is_finite()
        || !keyframe.x.is_finite()
        || !keyframe.y.is_finite()
        || !keyframe.zoom.is_finite()
    {
        return;
    }

    let keyframe = CameraKeyframe {
        time: clamp_time(keyframe.time, duration),
        x: keyframe.x,
        y: keyframe.y,
        zoom: keyframe.zoom.max(1.0),
    };

    if let Some(existing) = keyframes
        .iter_mut()
        .find(|existing| (existing.time - keyframe.time).abs() <= EXPRESSION_EPSILON)
    {
        *existing = keyframe;
        return;
    }

    keyframes.push(keyframe);
}

fn normalize_camera_keyframes(
    mut keyframes: Vec<CameraKeyframe>,
    info: VideoInfo,
) -> Vec<CameraKeyframe> {
    keyframes.sort_by(|left, right| {
        left.time
            .partial_cmp(&right.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut normalized: Vec<CameraKeyframe> = Vec::with_capacity(keyframes.len());

    for keyframe in keyframes {
        if let Some(last) = normalized.last_mut() {
            if keyframe.time <= last.time + EXPRESSION_EPSILON {
                *last = keyframe;
                continue;
            }
        }

        normalized.push(keyframe);
    }

    if normalized
        .first()
        .map(|keyframe| keyframe.time > EXPRESSION_EPSILON)
        .unwrap_or(true)
    {
        normalized.insert(0, default_keyframe(0.0, info));
    }

    if normalized
        .last()
        .map(|keyframe| keyframe.time < info.duration - EXPRESSION_EPSILON)
        .unwrap_or(true)
    {
        normalized.push(default_keyframe(info.duration, info));
    }

    normalized
}

fn camera_intervals(keyframes: &[CameraKeyframe]) -> Vec<CameraInterval> {
    keyframes
        .windows(2)
        .filter_map(|window| {
            let from = window[0];
            let to = window[1];

            if to.time <= from.time + EXPRESSION_EPSILON {
                return None;
            }

            Some(CameraInterval {
                start: from.time,
                end: to.time,
                from,
                to,
            })
        })
        .collect()
}

fn sequence_scale(_sequence: &TimelineSequence, config: ExportConfig) -> f64 {
    config.zoom_scale
}

fn sequence_complete_at(sequence: &TimelineSequence, config: ExportConfig) -> f64 {
    sequence.end + config.zoom_hold_seconds() + config.zoom_out_seconds()
}

fn milliseconds_to_seconds(milliseconds: u64) -> f64 {
    milliseconds as f64 / 1000.0
}

fn default_keyframe(time: f64, info: VideoInfo) -> CameraKeyframe {
    CameraKeyframe {
        time,
        x: f64::from(info.width) / 2.0,
        y: f64::from(info.height) / 2.0,
        zoom: 1.0,
    }
}

fn target_keyframe(time: f64, target: &TimelineSegment, scale: f64) -> CameraKeyframe {
    CameraKeyframe {
        time,
        x: f64::from(target.x),
        y: f64::from(target.y),
        zoom: scale,
    }
}

fn interpolate_expression(from: f64, to: f64, duration: f64) -> String {
    if (from - to).abs() <= EXPRESSION_EPSILON {
        return format_seconds(to);
    }

    format!(
        "{from}+({to}-{from})*(0.5-0.5*cos(PI*out_time/{duration}))",
        from = format_seconds(from),
        to = format_seconds(to),
        duration = format_seconds(duration.max(MIN_CAMERA_INTERVAL_SECONDS))
    )
}

fn clamp_time(value: f64, duration: f64) -> f64 {
    value.max(0.0).min(duration)
}

fn write_filter_script(path: &Path, filter: &str) -> ExportCommandResult<()> {
    fs::write(path, filter).map_err(|error| {
        ExportError::new(
            "write_filter_script_failed",
            format!(
                "Could not write FFmpeg filter script at {}: {error}",
                path.display()
            ),
            true,
        )
    })
}

fn remove_filter_script(paths: &ExportPaths) -> ExportCommandResult<()> {
    if !paths.filter_script_path.exists() {
        return Ok(());
    }

    remove_file(&paths.filter_script_path)
}

fn run_ffmpeg_zoom_export(
    app: &AppHandle,
    paths: &ExportPaths,
    duration_seconds: f64,
    config: ExportConfig,
) -> ExportCommandResult<()> {
    run_ffmpeg(
        app,
        [
            "-hide_banner".to_string(),
            "-nostats".to_string(),
            "-progress".to_string(),
            "pipe:1".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            path_to_string(&paths.input_path)?,
            "-filter_complex_script".to_string(),
            path_to_string(&paths.filter_script_path)?,
            "-map".to_string(),
            "[v]".to_string(),
            "-r".to_string(),
            config.fps.to_string(),
            "-an".to_string(),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-preset".to_string(),
            "veryfast".to_string(),
            "-crf".to_string(),
            config.crf.to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-movflags".to_string(),
            "+faststart".to_string(),
            path_to_string(&paths.temp_output_path)?,
        ],
        duration_seconds,
    )
}

fn run_ffmpeg_copy(
    app: &AppHandle,
    paths: &ExportPaths,
    duration_seconds: f64,
    config: ExportConfig,
) -> ExportCommandResult<()> {
    run_ffmpeg(
        app,
        [
            "-hide_banner".to_string(),
            "-nostats".to_string(),
            "-progress".to_string(),
            "pipe:1".to_string(),
            "-y".to_string(),
            "-i".to_string(),
            path_to_string(&paths.input_path)?,
            "-r".to_string(),
            config.fps.to_string(),
            "-an".to_string(),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-preset".to_string(),
            "veryfast".to_string(),
            "-crf".to_string(),
            config.crf.to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-movflags".to_string(),
            "+faststart".to_string(),
            path_to_string(&paths.temp_output_path)?,
        ],
        duration_seconds,
    )
}

fn run_ffmpeg<I>(app: &AppHandle, args: I, duration_seconds: f64) -> ExportCommandResult<()>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let command = ffmpeg_sidecar_command(app)?;
    let program = command.get_program().to_string_lossy().to_string();
    println!("{}", command_string(&program, &args));

    let output = ffmpeg_sidecar_output_with_progress(app, args, duration_seconds)?;

    if output.status.success() {
        return Ok(());
    }

    Err(ExportError::new(
        "ffmpeg_export_failed",
        format!(
            "FFmpeg export failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        true,
    ))
}

fn ffmpeg_sidecar_command(app: &AppHandle) -> ExportCommandResult<Command> {
    let command = app.shell().sidecar(FFMPEG_SIDECAR_NAME).map_err(|error| {
        ExportError::new(
            "ffmpeg_sidecar_unavailable",
            format!("Could not resolve bundled FFmpeg sidecar: {error}"),
            true,
        )
    })?;

    Ok(command.into())
}

fn command_string(program: &str, args: &[String]) -> String {
    std::iter::once(program.to_string())
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

fn replace_output_file(temp_path: &Path, output_path: &Path) -> ExportCommandResult<()> {
    if output_path.exists() {
        remove_file(output_path)?;
    }

    fs::rename(temp_path, output_path).map_err(|error| {
        ExportError::new(
            "replace_output_failed",
            format!("Could not replace edited.mp4: {error}"),
            true,
        )
    })
}

fn ensure_input_exists(path: &Path, label: &str) -> ExportCommandResult<()> {
    if path.exists() {
        return Ok(());
    }

    Err(ExportError::new(
        "input_missing",
        format!("{label} does not exist at {}", path.display()),
        true,
    ))
}

fn remove_file(path: &Path) -> ExportCommandResult<()> {
    fs::remove_file(path).map_err(|error| {
        ExportError::new(
            "remove_file_failed",
            format!("Could not remove {}: {error}", path.display()),
            true,
        )
    })
}

fn ensure_windows() -> ExportCommandResult<()> {
    if cfg!(target_os = "windows") {
        Ok(())
    } else {
        Err(ExportError::new(
            "unsupported_platform",
            "This export implementation is Windows-only",
            false,
        ))
    }
}

fn path_to_string(path: &Path) -> ExportCommandResult<String> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        ExportError::new(
            "non_utf8_path",
            format!("Path is not valid UTF-8: {}", path.display()),
            false,
        )
    })
}

fn format_seconds(value: f64) -> String {
    format!("{value:.6}")
}
