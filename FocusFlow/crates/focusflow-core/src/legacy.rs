use std::{fs::File, io::Read, path::Path};
use serde::Deserialize;
use serde_json::Value;
use crate::{audio::is_usable_wav, ffmpeg::ProbedVideo, paths::asset_path, project::*, CoreError, CoreResult, ErrorCode};
use crate::project::{invalid, require, valid_id, validate_video};

#[derive(Debug, Clone, Copy)]
pub struct LegacyDefaults {
    pub scale: f64, pub intro_us: u64, pub hold_us: u64, pub outro_us: u64,
    pub pan_transition_us: u64, pub settle_us: u64,
}

pub fn seconds_to_us(seconds: f64) -> CoreResult<u64> {
    require(
        seconds.is_finite() && (0.0..=86_400.0).contains(&seconds),
        "time",
        "Time must be finite, nonnegative and at most 24 hours",
    )?;
    Ok((seconds * 1_000_000.0).round() as u64)
}

pub fn read_legacy(project_dir: &Path, project_id: &str, video: ProbedVideo, defaults: LegacyDefaults) -> CoreResult<ProjectDocument> {
    require(valid_id(project_id), "projectId", "Invalid project ID")?;
    validate_video(video.width, video.height, video.duration_us)?;
    validate_defaults(defaults)?;
    asset_path(project_dir, "screen.mp4")?;
    let mut warnings = vec![ProjectWarning::LegacyTimingUnknown, ProjectWarning::EmbeddedCursor];
    let declared_audio = if let Some(value) = optional_json(project_dir, "metadata")? {
        let version = value.get("version").and_then(Value::as_u64).ok_or_else(|| invalid("metadata.version", "Metadata version must be an integer"))?;
        if version != 1 { return Err(CoreError { code: ErrorCode::UnsupportedSchema, message: "Only legacy metadata version 1 is supported".into(), field: Some("metadata.version".into()) }); }
        let meta: Metadata = decode(value, "metadata")?;
        require(
            (1..=16384).contains(&meta.capture_bounds.width),
            "metadata.captureBounds.width",
            "Invalid capture width",
        )?;
        require(
            (1..=16384).contains(&meta.capture_bounds.height),
            "metadata.captureBounds.height",
            "Invalid capture height",
        )?;
        if let Source::Region { width, height, .. } = meta.source {
            require(
                (1..=16384).contains(&width) && (1..=16384).contains(&height),
                "metadata.source",
                "Invalid region dimensions",
            )?;
        }
        meta.has_mic_audio
    } else { warnings.push(ProjectWarning::LegacyMetadataMissing); false };
    if let Some(value) = optional_json(project_dir, "session")? {
        let session: Session = decode(value, "session")?;
        time_at(session.duration_seconds, "session.durationSeconds")?;
    }
    let microphone = match asset_path(project_dir, "mic.wav") {
        Ok(path) if is_usable_wav(&path) => Some(AudioAsset { path: "mic.wav".into() }),
        Ok(_) => { warnings.push(ProjectWarning::InvalidOptionalAudio); None },
        Err(error) if error.code == ErrorCode::NotFound && !declared_audio && is_absent(&project_dir.join("mic.wav")) => None,
        Err(_) => { warnings.push(ProjectWarning::InvalidOptionalAudio); None },
    };
    let interactions = convert_interactions(video, defaults, read_json(project_dir, "timeline")?,
        read_json(project_dir, "clicks")?, optional_json(project_dir, "drags")?)?;
    let LegacyInteractions { zooms, clicks, warnings: interaction_warnings } = interactions;
    warnings.extend(interaction_warnings);
    let document = ProjectDocument {
        frame_style: Default::default(), schema_version: 3, project_id: project_id.into(), revision: 0,
        cursor: None, cursor_settings: Default::default(),
        video: VideoAsset { cursor: crate::project::CursorMode::Embedded, path: "screen.mp4".into(), width: video.width, height: video.height, duration_us: video.duration_us },
        microphone, timing: Timing { provenance: TimingProvenance::LegacyUnknown, audio_offset_us: None },
        camera: CameraDefaults { pan_transition_us: defaults.pan_transition_us, settle_us: defaults.settle_us },
        clips: vec![Clip { id: "clip-0".into(), in_us: 0, out_us: video.duration_us }], zooms, clicks, warnings,
    };
    validate(&document)?;
    Ok(document)
}

/// Validate the camera defaults shared by legacy import and native recording snapshots.
pub fn validate_defaults(defaults: LegacyDefaults) -> CoreResult<()> {
    require(
        defaults.scale.is_finite() && (1.0..=8.0).contains(&defaults.scale),
        "defaults.scale",
        "Scale must be finite and between 1 and 8",
    )?;
    for (field, time) in [("introUs", defaults.intro_us), ("holdUs", defaults.hold_us), ("outroUs", defaults.outro_us), ("panTransitionUs", defaults.pan_transition_us), ("settleUs", defaults.settle_us)] {
        require(time <= MAX_DURATION_US, &format!("defaults.{field}"), "Time exceeds 24 hours")?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct LegacyInteractions {
    pub zooms: Vec<ZoomElement>,
    pub clicks: Vec<Click>,
    pub warnings: Vec<ProjectWarning>,
}

/// Convert already loaded legacy interactions without reading or writing session files.
pub fn convert_interactions(video: ProbedVideo, defaults: LegacyDefaults, timeline: Value,
    clicks: Value, drags: Option<Value>) -> CoreResult<LegacyInteractions>
{
    validate_video(video.width, video.height, video.duration_us)?;
    validate_defaults(defaults)?;
    let mut warnings = Vec::new();
    let timeline = array(timeline, "timeline")?;
    require(timeline.len() <= 16384, "timeline", "At most 16384 zooms are supported")?;
    let mut zooms = Vec::with_capacity(timeline.len());
    let mut interaction_clipped = false;
    for (index, value) in timeline.into_iter().enumerate() {
        let field = format!("timeline[{index}]");
        let segment: Segment = decode(value, &field)?;
        let start_us = time_at(segment.start, &format!("{field}.start"))?;
        let end_us = time_at(segment.end, &format!("{field}.end"))?;
        require(
            segment.start < segment.end,
            &format!("{field}.end"),
            "Timeline end must follow start",
        )?;
        let (x, y) = normalize(segment.x, segment.y, video, &field)?;
        require(
            segment.scale.is_finite() && segment.scale > 0.0,
            &format!("{field}.scale"),
            "Legacy scale must be finite and positive",
        )?;
        if segment.scale != defaults.scale && !warnings.contains(&ProjectWarning::LegacyScaleOverridden) { warnings.push(ProjectWarning::LegacyScaleOverridden); }
        // Legacy targets include idle tails; only their intersection with source media can render.
        let clipped_end = end_us.min(video.duration_us);
        if clipped_end != end_us || start_us >= clipped_end {
            interaction_clipped = true;
        }
        if start_us >= clipped_end {
            continue;
        }
        let end_us = clipped_end;
        zooms.push(ZoomElement { mode: ZoomMode::Fixed, id: format!("zoom-{index}"), clip_id: "clip-0".into(), start_us, end_us, x, y, scale: defaults.scale, intro_us: defaults.intro_us, hold_us: defaults.hold_us, outro_us: defaults.outro_us });
    }
    zooms.sort_by_key(|zoom| zoom.start_us);
    let mut normalized: Vec<ZoomElement> = Vec::with_capacity(zooms.len());
    let mut overlap_normalized = false;
    for mut zoom in zooms {
        if let Some(previous) = normalized.last_mut() {
            if previous.end_us > zoom.start_us {
                // The old camera keeps the later target through the merged sequence tail.
                zoom.end_us = zoom.end_us.max(previous.end_us);
                previous.end_us = zoom.start_us;
                overlap_normalized = true;
                if previous.start_us == previous.end_us {
                    normalized.pop();
                }
            }
        }
        normalized.push(zoom);
    }
    let zooms = normalized;
    if overlap_normalized {
        warnings.push(ProjectWarning::LegacyZoomOverlapNormalized);
    }
    let raw_clicks = array(clicks, "clicks")?;
    let mut clicks = Vec::with_capacity(raw_clicks.len());
    for (index, value) in raw_clicks.into_iter().enumerate() {
        let field = format!("clicks[{index}]");
        let click: LegacyClick = decode(value, &field)?;
        let at_us = time_at(click.timestamp, &format!("{field}.timestamp"))?;
        let (x, y) = normalize(click.x, click.y, video, &field)?;
        if at_us >= video.duration_us {
            interaction_clipped = true;
            continue;
        }
        clicks.push(Click { at_us, x, y });
    }
    // Preserve the saved timeline; validate retained drag records without generating replacement targets.
    if let Some(value) = drags {
        for (index, value) in array(value, "drags")?.into_iter().enumerate() {
            let field = format!("drags[{index}]");
            let drag: Drag = decode(value, &field)?;
            let start = time_at(drag.start, &format!("{field}.start"))?;
            let end = time_at(drag.end, &format!("{field}.end"))?;
            require(
                drag.start <= drag.end,
                &format!("{field}.end"),
                "Drag end must follow start",
            )?;
            if start >= video.duration_us || end > video.duration_us {
                interaction_clipped = true;
            }
            let mut previous_timestamp = drag.start;
            for (index, point) in drag.points.into_iter().enumerate() {
                let point_field = format!("{field}.points[{index}]");
                let timestamp = time_at(point.timestamp, &format!("{point_field}.timestamp"))?;
                require(
                    point.timestamp >= previous_timestamp && point.timestamp <= drag.end,
                    &format!("{point_field}.timestamp"),
                    "Drag point must be ordered and inside its drag interval",
                )?;
                normalize(point.x, point.y, video, &point_field)?;
                previous_timestamp = point.timestamp;
                if timestamp >= video.duration_us {
                    interaction_clipped = true;
                }
            }
        }
    }
    if interaction_clipped {
        warnings.push(ProjectWarning::LegacyInteractionClipped);
    }
    Ok(LegacyInteractions { zooms, clicks, warnings })
}

fn normalize(x: i32, y: i32, video: ProbedVideo, field: &str) -> CoreResult<(f64, f64)> {
    // Recorder already saved capture-relative pixels; the desktop origin must not be subtracted again.
    require(x >= 0 && (x as u32) < video.width, &format!("{field}.x"), "X lies outside the capture")?;
    require(y >= 0 && (y as u32) < video.height, &format!("{field}.y"), "Y lies outside the capture")?;
    Ok((f64::from(x) / f64::from(video.width), f64::from(y) / f64::from(video.height)))
}
fn time_at(seconds: f64, field: &str) -> CoreResult<u64> {
    seconds_to_us(seconds).map_err(|mut error| { error.field = Some(field.into()); error })
}
fn optional_json(dir: &Path, name: &str) -> CoreResult<Option<Value>> {
    match read_json(dir, name) { Err(error) if error.code == ErrorCode::NotFound && is_absent(&dir.join(format!("{name}.json"))) => Ok(None), result => result.map(Some) }
}
fn is_absent(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}
fn read_json(dir: &Path, name: &str) -> CoreResult<Value> {
    let path = asset_path(dir, &format!("{name}.json")).map_err(|mut error| { error.field = Some(name.into()); error })?;
    let file = File::open(path).map_err(|error| io_error(name, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes).map_err(|error| io_error(name, error))?;
    require(bytes.len() as u64 <= MAX_JSON_BYTES, name, "JSON exceeds the 8 MiB limit")?;
    serde_json::from_slice(&bytes).map_err(|error| invalid(name, format!("Could not parse {name}.json: {error}")))
}
fn io_error(field: &str, error: std::io::Error) -> CoreError { CoreError { code: ErrorCode::Io, message: format!("Could not read {field}.json: {error}"), field: Some(field.into()) } }
fn array(value: Value, field: &str) -> CoreResult<Vec<Value>> {
    match value { Value::Array(values) => Ok(values), _ => Err(invalid(field, "Expected a JSON array")) }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value, field: &str) -> CoreResult<T> {
    serde_json::from_value(value).map_err(|error| invalid(field, format!("Invalid {field}: {error}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Segment { start: f64, end: f64, x: i32, y: i32, scale: f64 }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyClick { timestamp: f64, x: i32, y: i32, #[serde(rename = "button")] _button: Button }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Drag { start: f64, end: f64, #[serde(rename = "button")] _button: Button, points: Vec<DragPoint> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DragPoint { timestamp: f64, x: i32, y: i32 }
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Button { Left, Right }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Metadata {
    #[serde(rename = "version")] _version: u32,
    source: Source, capture_bounds: Bounds,
    #[serde(rename = "captureBackend")] _capture_backend: Backend,
    #[serde(default)] has_mic_audio: bool,
    #[serde(default, rename = "micDeviceName")] _mic_device_name: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bounds { #[serde(rename = "x")] _x: i32, #[serde(rename = "y")] _y: i32, width: u32, height: u32 }
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Source { Screen, Window { #[serde(rename = "hwnd")] _hwnd: String, #[serde(rename = "title")] _title: String }, Region { #[serde(rename = "x")] _x: i32, #[serde(rename = "y")] _y: i32, width: u32, height: u32 } }
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Backend { Ffmpeg, WindowsCapture }
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Session {
    #[serde(rename = "session_id", alias = "sessionId")] _session_id: String,
    #[serde(rename = "created_at", alias = "createdAt")] _created_at: String,
    #[serde(alias = "durationSeconds")] duration_seconds: f64,
    #[serde(rename = "recording_source", alias = "recordingSource")] _recording_source: String,
    #[serde(rename = "exported")] _exported: bool,
}
