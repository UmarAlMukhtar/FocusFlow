use std::{path::{Path, PathBuf}, process::Command, thread, time::Duration};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use focusflow_core::{CoreError, CoreResult, ErrorCode, edits::{self, Edit, MappedClip}, jobs::{JobManager, JobOutput, JobState, JobStatus},
    legacy::LegacyDefaults, paths, preview::PreviewRequest, project::{EditableState, ProjectDocument}, render::ExportPreset,
    store::{ProjectCapabilities, ProjectStore}};

pub struct DesktopProjects {
    pub root: PathBuf,
    pub store: ProjectStore,
    pub jobs: JobManager,
    ffmpeg: PathBuf,
}
impl DesktopProjects {
    pub fn new(root: &Path, ffmpeg: &Path) -> CoreResult<Self> {
        Ok(Self { root: root.canonicalize().map_err(|error| invalid(error.to_string()))?,
            store: ProjectStore::new(root)?, jobs: JobManager::new(root, ffmpeg)?, ffmpeg: ffmpeg.into() })
    }
}

pub type ExportCommandResult<T> = Result<T, ExportError>;
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportError { pub code: ErrorCode, pub message: String, pub recoverable: bool, pub field: Option<String> }
impl From<CoreError> for ExportError {
    fn from(error: CoreError) -> Self {
        Self { code: error.code, message: error.message, recoverable: true, field: error.field }
    }
}
impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for ExportError {}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportSettings {
    pub zoom_scale: Option<f64>, pub zoom_in_ms: Option<f64>, pub zoom_out_ms: Option<f64>,
    pub pan_transition_ms: Option<f64>, pub preset: Option<ExportPreset>,
}
impl ExportSettings {
    pub(crate) fn legacy_defaults(&self) -> CoreResult<LegacyDefaults> {
        fn micros(value: Option<f64>, default: u64) -> CoreResult<u64> {
            match value {
                None => Ok(default),
                Some(ms) if ms.is_finite() && (0.0..=86_400_000.0).contains(&ms) => Ok((ms * 1000.0).round() as u64),
                _ => Err(invalid("Camera timing must be finite, nonnegative and at most 24 hours")),
            }
        }
        let defaults = LegacyDefaults { scale: self.zoom_scale.unwrap_or(2.3),
            intro_us: micros(self.zoom_in_ms, 180_000)?, hold_us: 150_000,
            outro_us: micros(self.zoom_out_ms, 220_000)?,
            pan_transition_us: micros(self.pan_transition_ms, 220_000)?, settle_us: 120_000 };
        focusflow_core::legacy::validate_defaults(defaults)?;
        Ok(defaults)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportStatus {
    pub input_path: String, pub timeline_path: String, pub output_path: String, pub segment_count: usize,
    pub job_id: String, pub project_id: String, pub revision: u64, pub warnings: Vec<String>, pub request_id: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgress<'a> {
    request_id: &'a str, project_id: &'a str, revision: u64, job_id: &'a str, percentage: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorProject {
    pub document: ProjectDocument,
    pub capabilities: ProjectCapabilities,
    pub mapped_clips: Vec<MappedClip>,
    pub duration_us: u64,
}

fn editor_project(store: &ProjectStore, document: ProjectDocument, validate_cursor_assets: bool) -> CoreResult<EditorProject> {
    let mapped_clips = edits::map_clips(&document)?;
    let duration_us = mapped_clips.last().map(|clip|
        clip.output_start_us + (clip.source_end_us - clip.source_start_us)).unwrap_or(0);
    let capabilities = if validate_cursor_assets { store.capabilities(&document) } else {
        focusflow_core::cursor::capabilities(&document, &focusflow_core::cursor::CursorAssetReport::Unchecked)
    };
    Ok(EditorProject { document, capabilities, mapped_clips, duration_us })
}

fn join_error(error: impl std::fmt::Display) -> CoreError {
    CoreError { code: ErrorCode::Io, message: format!("Could not join project task: {error}"), field: None }
}

#[tauri::command]
pub async fn open_project(app: AppHandle, session_id: String, settings: ExportSettings) -> ExportCommandResult<ProjectDocument> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        state.store.open_or_import(&session_id, &state.ffmpeg, settings.legacy_defaults()?).map_err(Into::into)
    }).await.map_err(|error| ExportError::from(invalid(format!("Could not join project load: {error}"))))?
}

#[tauri::command]
pub async fn get_project(app: AppHandle, session_id: String, validate_cursor_assets: Option<bool>) -> CoreResult<EditorProject> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        editor_project(&state.store, state.store.get(&session_id)?, validate_cursor_assets.unwrap_or(true))
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn apply_project_edits(app: AppHandle, session_id: String, expected_revision: u64,
    edits: Vec<Edit>) -> CoreResult<EditorProject> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        editor_project(&state.store, state.store.apply_edits(&session_id, expected_revision, &edits)?, true)
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn restore_project_edits(app: AppHandle, session_id: String, expected_revision: u64,
    state: EditableState) -> CoreResult<EditorProject> {
    tauri::async_runtime::spawn_blocking(move || {
        let projects = app.state::<DesktopProjects>();
        editor_project(&projects.store, projects.store.restore_edits(&session_id, expected_revision, &state)?, true)
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn render_project_preview(app: AppHandle, session_id: String, expected_revision: u64,
    request: PreviewRequest) -> CoreResult<JobStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        state.jobs.start_preview(&session_id, expected_revision, request)
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn get_project_job(app: AppHandle, session_id: String, job_id: String) -> CoreResult<JobStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        let status = state.jobs.status(&session_id, &job_id)?;
        if status.state != JobState::Running {
            return state.jobs.wait(&session_id, &job_id);
        }
        Ok(status)
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn cancel_project_job(app: AppHandle, session_id: String, job_id: String) -> CoreResult<JobStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        state.jobs.cancel(&session_id, &job_id)?;
        // Joining releases the render guard before a replacement preview or export can start.
        state.jobs.wait(&session_id, &job_id)
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn get_project_media(app: AppHandle, session_id: String, job_id: String) -> CoreResult<JobOutput> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        let output = state.jobs.output(&session_id, &job_id)?;
        app.asset_protocol_scope().allow_file(&output.path).map_err(|error| CoreError {
            code: ErrorCode::Io, message: format!("Could not allow completed project media: {error}"), field: None })?;
        Ok(output)
    }).await.map_err(join_error)?
}

#[tauri::command]
pub async fn export_edited_mp4(app: AppHandle, session_id: String, expected_revision: u64,
    settings: ExportSettings, request_id: String) -> ExportCommandResult<ExportStatus> {
    if request_id.trim().is_empty() || request_id.len() > 128 {
        return Err(invalid("Export request ID must be nonempty and at most 128 bytes").into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<DesktopProjects>();
        let snapshot = state.store.snapshot(&session_id, expected_revision)?;
        let project = snapshot.project_dir;
        let document = snapshot.document;
        let input = paths::asset_path(&project, &document.video.path)?;
        eprintln!("Export {session_id} revision {expected_revision}: mic.wav detected={}, audio enabled={}",
            project.join("mic.wav").exists(), document.microphone.is_some());
        let mut job = state.jobs.start_export(&session_id, expected_revision, settings.preset.unwrap_or(ExportPreset::Balanced))?;
        loop {
            if let Err(error) = app.emit("export-progress", ExportProgress { request_id: &request_id,
                project_id: &job.project_id, revision: job.revision, job_id: &job.job_id, percentage: job.percentage }) {
                eprintln!("Could not emit export progress: {error}");
            }
            if job.state != JobState::Running { break; }
            let previous = job.clone();
            loop {
                thread::sleep(Duration::from_millis(100));
                job = state.jobs.status(&session_id, &previous.job_id)?;
                if job != previous { break; }
            }
        }
        let job = state.jobs.wait(&session_id, &job.job_id)?;
        if job.state != JobState::Completed {
            return Err(ExportError::from(job.error.unwrap_or_else(|| CoreError {
                code: match job.state { JobState::Cancelled => ErrorCode::Cancelled, JobState::Interrupted => ErrorCode::Interrupted, _ => ErrorCode::EncoderFailed },
                message: format!("Export ended with {:?}", job.state), field: None })));
        }
        let output = completed_output(&project, &job)?;
        eprintln!("Export {} completed: {} bytes, warnings: {:?}", job.job_id,
            std::fs::metadata(&output).map(|metadata| metadata.len()).unwrap_or(0), job.warnings);
        Ok(ExportStatus { input_path: input.to_string_lossy().into_owned(), timeline_path: project.join("timeline.json").to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(), segment_count: document.zooms.len(),
            job_id: job.job_id, project_id: job.project_id, revision: job.revision, warnings: job.warnings, request_id })
    }).await.map_err(|error| ExportError::from(invalid(format!("Could not join export task: {error}"))))?
}

#[tauri::command]
pub fn get_export_job(app: AppHandle, session_id: String, job_id: String) -> ExportCommandResult<JobStatus> {
    app.state::<DesktopProjects>().jobs.status(&session_id, &job_id).map_err(Into::into)
}
#[tauri::command]
pub fn cancel_export_job(app: AppHandle, session_id: String, job_id: String) -> ExportCommandResult<JobStatus> {
    app.state::<DesktopProjects>().jobs.cancel(&session_id, &job_id).map_err(Into::into)
}

pub fn completed_output(project: &Path, job: &JobStatus) -> CoreResult<PathBuf> {
    if job.state != JobState::Completed { return Err(invalid("Export is not completed")); }
    paths::job_dir(project, &job.job_id)?;
    paths::asset_path(project, job.output_relative_path.as_deref().ok_or_else(|| invalid("Completed export has no output"))?)
}
fn invalid(message: impl Into<String>) -> CoreError {
    CoreError { code: ErrorCode::InvalidDocument, message: message.into(), field: None }
}
pub fn ffmpeg_sidecar_command(app: &AppHandle) -> ExportCommandResult<Command> {
    crate::ffmpeg::command(app).map_err(|error| ExportError::from(CoreError {
        code: ErrorCode::UnsupportedCapability, message: format!("Could not resolve bundled FFmpeg sidecar: {error}"), field: None }))
}

#[cfg(test)]
mod defaults_tests {
    use super::*;

    fn settings() -> ExportSettings {
        serde_json::from_value(serde_json::json!({})).unwrap()
    }

    #[test]
    fn legacy_defaults_reject_invalid_scale_before_capture() {
        for scale in [0.0, 0.999, 8.001, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut settings = settings();
            settings.zoom_scale = Some(scale);
            assert!(settings.legacy_defaults().is_err(), "invalid scale {scale} must fail before capture");
        }
    }

    #[test]
    fn legacy_defaults_freeze_current_omitted_and_custom_settings() {
        let mut settings = settings();
        let frozen = settings.legacy_defaults().unwrap();
        settings.zoom_scale = Some(4.0);
        settings.zoom_in_ms = Some(123.456);
        settings.zoom_out_ms = Some(234.567);
        settings.pan_transition_ms = Some(345.678);
        let later = settings.legacy_defaults().unwrap();
        assert_eq!((frozen.scale, frozen.intro_us, frozen.hold_us, frozen.outro_us, frozen.pan_transition_us, frozen.settle_us),
            (2.3, 180_000, 150_000, 220_000, 220_000, 120_000));
        assert_eq!((later.scale, later.intro_us, later.hold_us, later.outro_us, later.pan_transition_us, later.settle_us),
            (4.0, 123_456, 150_000, 234_567, 345_678, 120_000));
    }

    #[test]
    fn legacy_defaults_validate_each_settings_timing_and_scale_boundary() {
        for scale in [1.0, 8.0] {
            let mut settings = settings(); settings.zoom_scale = Some(scale);
            assert_eq!(settings.legacy_defaults().unwrap().scale, scale);
        }
        for field in ["zoomInMs", "zoomOutMs", "panTransitionMs"] {
            for ms in [-0.001, 86_400_000.001] {
                let settings: ExportSettings = serde_json::from_value(serde_json::json!({field: ms})).unwrap();
                assert!(settings.legacy_defaults().is_err(), "{field}={ms}");
            }
            for ms in [0.0, 86_400_000.0] {
                let settings: ExportSettings = serde_json::from_value(serde_json::json!({field: ms})).unwrap();
                let defaults = settings.legacy_defaults().unwrap();
                let actual = match field { "zoomInMs" => defaults.intro_us, "zoomOutMs" => defaults.outro_us, _ => defaults.pan_transition_us };
                assert_eq!(actual, if ms == 0.0 { 0 } else { 86_400_000_000 });
            }
        }
        for set in [
            (|s: &mut ExportSettings, v| s.zoom_in_ms = Some(v)) as fn(&mut ExportSettings, f64),
            |s: &mut ExportSettings, v| s.zoom_out_ms = Some(v),
            |s: &mut ExportSettings, v| s.pan_transition_ms = Some(v),
        ] {
            for ms in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let mut settings = settings(); set(&mut settings, ms);
                assert!(settings.legacy_defaults().is_err());
            }
        }
    }
}
