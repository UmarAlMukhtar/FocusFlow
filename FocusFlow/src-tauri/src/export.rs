use serde::{Deserialize, Serialize};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

const EDITED_FILE_NAME: &str = "edited.mp4";
const OUTPUT_FILE_NAME: &str = "screen.mp4";
const OUTPUT_RECORDINGS_DIR_NAME: &str = "Recordings";
const OUTPUT_ROOT_DIR_NAME: &str = "FocusFlow";
const TARGET_FPS: u32 = 60;
const TIMELINE_FILE_NAME: &str = "timeline.json";
const ZOOM_RAMP_SECONDS: f64 = 0.35;
const ZOOMPAN_FRAMES_PER_INPUT: u32 = 2;

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

#[derive(Debug, Clone, Deserialize)]
struct TimelineSegment {
    start: f64,
    end: f64,
    x: i32,
    y: i32,
    scale: f64,
}

#[derive(Debug, Clone, Copy)]
struct VideoSize {
    width: u32,
    height: u32,
}

#[tauri::command]
pub async fn export_edited_mp4(app: AppHandle) -> ExportCommandResult<ExportStatus> {
    let paths = ExportPaths::resolve(&app)?;

    tauri::async_runtime::spawn_blocking(move || export_edited_mp4_blocking(paths))
        .await
        .map_err(|error| {
            ExportError::new(
                "export_join_failed",
                format!("Could not join export task: {error}"),
                true,
            )
        })?
}

fn export_edited_mp4_blocking(paths: ExportPaths) -> ExportCommandResult<ExportStatus> {
    ensure_windows()?;
    ensure_input_exists(&paths.input_path, "screen.mp4")?;
    ensure_input_exists(&paths.timeline_path, "timeline.json")?;

    let timeline = read_timeline(&paths.timeline_path)?;
    let video_size = probe_video_size(&paths.input_path)?;

    if paths.temp_output_path.exists() {
        remove_file(&paths.temp_output_path)?;
    }

    if timeline.is_empty() {
        run_ffmpeg_copy(&paths)?;
    } else {
        let filter = build_zoom_filter(&timeline, video_size);
        run_ffmpeg_zoom_export(&paths, &filter)?;
    }

    replace_output_file(&paths.temp_output_path, &paths.output_path)?;

    Ok(ExportStatus {
        input_path: path_to_string(&paths.input_path)?,
        timeline_path: path_to_string(&paths.timeline_path)?,
        output_path: path_to_string(&paths.output_path)?,
        segment_count: timeline.len(),
    })
}

#[derive(Debug, Clone)]
struct ExportPaths {
    input_path: PathBuf,
    timeline_path: PathBuf,
    output_path: PathBuf,
    temp_output_path: PathBuf,
}

impl ExportPaths {
    fn resolve(app: &AppHandle) -> ExportCommandResult<ExportPaths> {
        let dir = app
            .path()
            .document_dir()
            .map_err(|error| {
                ExportError::new(
                    "documents_dir_unavailable",
                    format!("Could not resolve Documents directory: {error}"),
                    true,
                )
            })?
            .join(OUTPUT_ROOT_DIR_NAME)
            .join(OUTPUT_RECORDINGS_DIR_NAME);

        fs::create_dir_all(&dir).map_err(|error| {
            ExportError::new(
                "create_output_dir_failed",
                format!("Could not create recording output directory: {error}"),
                true,
            )
        })?;

        let session_dir = latest_session_dir(&dir)?;

        Ok(ExportPaths {
            input_path: session_dir.join(OUTPUT_FILE_NAME),
            timeline_path: session_dir.join(TIMELINE_FILE_NAME),
            output_path: session_dir.join(EDITED_FILE_NAME),
            temp_output_path: session_dir.join("edited.tmp.mp4"),
        })
    }
}

fn latest_session_dir(recordings_dir: &Path) -> ExportCommandResult<PathBuf> {
    let mut candidates = Vec::new();
    let legacy_input_path = recordings_dir.join(OUTPUT_FILE_NAME);
    let legacy_timeline_path = recordings_dir.join(TIMELINE_FILE_NAME);

    if legacy_input_path.exists() && legacy_timeline_path.exists() {
        candidates.push((modified_time(recordings_dir), recordings_dir.to_path_buf()));
    }

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

        if path.join(OUTPUT_FILE_NAME).exists() && path.join(TIMELINE_FILE_NAME).exists() {
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
                    "No recording session containing screen.mp4 and timeline.json was found in {}",
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

fn probe_video_size(input_path: &Path) -> ExportCommandResult<VideoSize> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
        ])
        .arg(input_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            ExportError::new(
                "ffprobe_spawn_failed",
                format!("Could not start ffprobe: {error}"),
                true,
            )
        })?;

    if !output.status.success() {
        return Err(ExportError::new(
            "ffprobe_failed",
            format!(
                "ffprobe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            true,
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let size = stdout
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| {
            ExportError::new(
                "video_size_missing",
                "ffprobe did not return a video size",
                true,
            )
        })?;
    let (width, height) = size.split_once('x').ok_or_else(|| {
        ExportError::new(
            "video_size_invalid",
            format!("ffprobe returned an invalid video size: {size}"),
            true,
        )
    })?;

    Ok(VideoSize {
        width: width.trim().parse().map_err(|error| {
            ExportError::new(
                "video_width_invalid",
                format!("Could not parse video width: {error}"),
                true,
            )
        })?,
        height: height.trim().parse().map_err(|error| {
            ExportError::new(
                "video_height_invalid",
                format!("Could not parse video height: {error}"),
                true,
            )
        })?,
    })
}

fn build_zoom_filter(timeline: &[TimelineSegment], size: VideoSize) -> String {
    let zoom_expr = nested_zoom_expression(timeline);
    let x_expr = nested_coordinate_expression(timeline, Axis::X, f64::from(size.width) / 2.0);
    let y_expr = nested_coordinate_expression(timeline, Axis::Y, f64::from(size.height) / 2.0);

    format!(
        "[0:v]zoompan=z='{zoom}':x='clip(({x})-ow/(2*({zoom})),0,iw-ow/({zoom}))':y='clip(({y})-oh/(2*({zoom})),0,ih-oh/({zoom}))':d={duration}:s={width}x{height}:fps={fps},setsar=1[v]",
        zoom = zoom_expr,
        x = x_expr,
        y = y_expr,
        duration = ZOOMPAN_FRAMES_PER_INPUT,
        width = size.width,
        height = size.height,
        fps = TARGET_FPS
    )
}

fn nested_zoom_expression(timeline: &[TimelineSegment]) -> String {
    timeline
        .iter()
        .rev()
        .fold("1".to_string(), |next, segment| {
            format!(
                "if(between(out_time,{start},{end}),{active},{next})",
                start = format_seconds(segment.start),
                end = format_seconds(segment.end),
                active = segment_zoom_expression(segment),
                next = next
            )
        })
}

fn segment_zoom_expression(segment: &TimelineSegment) -> String {
    let amount = segment_amount_expression(segment);

    format!(
        "1+({scale}-1)*({amount})",
        scale = format_seconds(segment.scale),
        amount = amount
    )
}

fn segment_amount_expression(segment: &TimelineSegment) -> String {
    let duration = segment.end - segment.start;
    let ramp = ZOOM_RAMP_SECONDS.min(duration / 3.0).max(0.001);
    let ramp_end = segment.start + ramp;
    let hold_end = (segment.end - ramp).max(ramp_end);

    let ease_in = ease_amount_expression(segment.start, ramp, EaseDirection::In);
    let ease_out = ease_amount_expression(segment.end, ramp, EaseDirection::Out);

    format!(
        "if(between(out_time,{start},{ramp_end}),{ease_in},if(between(out_time,{ramp_end},{hold_end}),1,if(between(out_time,{hold_end},{end}),{ease_out},0)))",
        start = format_seconds(segment.start),
        ramp_end = format_seconds(ramp_end),
        hold_end = format_seconds(hold_end),
        end = format_seconds(segment.end),
        ease_in = ease_in,
        ease_out = ease_out
    )
}

#[derive(Debug, Clone, Copy)]
enum EaseDirection {
    In,
    Out,
}

fn ease_amount_expression(anchor: f64, ramp: f64, direction: EaseDirection) -> String {
    let progress = match direction {
        EaseDirection::In => format!(
            "((out_time-{})/{})",
            format_seconds(anchor),
            format_seconds(ramp)
        ),
        EaseDirection::Out => format!(
            "(({}-out_time)/{})",
            format_seconds(anchor),
            format_seconds(ramp)
        ),
    };

    format!("0.5-0.5*cos(PI*{progress})", progress = progress)
}

#[derive(Debug, Clone, Copy)]
enum Axis {
    X,
    Y,
}

fn nested_coordinate_expression(
    timeline: &[TimelineSegment],
    axis: Axis,
    default_value: f64,
) -> String {
    timeline
        .iter()
        .rev()
        .fold(format_seconds(default_value), |next, segment| {
            let coordinate = match axis {
                Axis::X => segment.x,
                Axis::Y => segment.y,
            };
            let amount = segment_amount_expression(segment);

            format!(
                "if(between(out_time,{start},{end}),{default}+({coordinate}-{default})*({amount}),{next})",
                start = format_seconds(segment.start),
                end = format_seconds(segment.end),
                default = format_seconds(default_value),
                coordinate = coordinate,
                amount = amount,
                next = next
            )
        })
}

fn run_ffmpeg_zoom_export(paths: &ExportPaths, filter: &str) -> ExportCommandResult<()> {
    run_ffmpeg([
        "-hide_banner".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        path_to_string(&paths.input_path)?,
        "-filter_complex".to_string(),
        filter.to_string(),
        "-map".to_string(),
        "[v]".to_string(),
        "-r".to_string(),
        TARGET_FPS.to_string(),
        "-an".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        "veryfast".to_string(),
        "-crf".to_string(),
        "22".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        path_to_string(&paths.temp_output_path)?,
    ])
}

fn run_ffmpeg_copy(paths: &ExportPaths) -> ExportCommandResult<()> {
    run_ffmpeg([
        "-hide_banner".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        path_to_string(&paths.input_path)?,
        "-c".to_string(),
        "copy".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        path_to_string(&paths.temp_output_path)?,
    ])
}

fn run_ffmpeg<I>(args: I) -> ExportCommandResult<()>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    println!("{}", command_string("ffmpeg", &args));

    let output = Command::new("ffmpeg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            ExportError::new(
                "ffmpeg_spawn_failed",
                format!("Could not start FFmpeg export: {error}"),
                true,
            )
        })?;

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
