use std::{
    ffi::OsString, fs::{self, File, OpenOptions}, io::{Read, Write, Seek, SeekFrom}, path::{Path, PathBuf},
    process::Command, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}},
    thread::JoinHandle, time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use serde::{Deserialize, Serialize};
use crate::{CoreResult, CoreError, ErrorCode, ffmpeg, paths, project::{invalid, require, MAX_REVISION},
    render::{self, ExportPreset}, store::{self, ProjectStore, RenderSnapshot}};

// Two bounded diagnostics can expand sixfold when JSON escapes control bytes.
const RECORD_LIMIT: u64 = 2 * 1024 * 1024;
const OUTPUT: &str = "output.mp4";
static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobKind { #[default] Export, PreviewVideo, PreviewStill }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputMetadata {
    pub basename: String, pub mime_type: String, pub width: u32, pub height: u32,
    pub first_frame: u64, pub frame_count: u64, pub fps: u32, pub start_us: u64, pub duration_us: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobOutput { pub path: PathBuf, pub size_bytes: u64, pub mime_type: String, pub metadata: Option<OutputMetadata> }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputChunk { pub bytes: Vec<u8>, pub total_bytes: u64, pub offset: u64, pub eof: bool }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState { Running, Completed, Cancelled, Failed, Interrupted }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobStatus {
    pub job_id: String, pub project_id: String, pub revision: u64,
    #[serde(default)] pub kind: JobKind,
    #[serde(default)] pub preview_request: Option<crate::preview::PreviewRequest>,
    #[serde(default)] pub output_metadata: Option<OutputMetadata>,
    pub state: JobState, pub percentage: f64, pub output_relative_path: Option<String>,
    pub error: Option<CoreError>, pub warnings: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JobRecord { schema_version: u32, preset: ExportPreset, status: JobStatus }
struct Control { cancel: AtomicBool, status: Mutex<JobStatus> }
struct Worker { control: Arc<Control>, thread: JoinHandle<()> }

/// Dropping the manager cancels and joins its workers. Workers never retain the manager itself.
pub struct JobManager { root: PathBuf, ffmpeg: PathBuf, workers: Mutex<Vec<Worker>>, closing: AtomicBool }
impl JobManager {
    pub fn new(root: &Path, ffmpeg: &Path) -> CoreResult<Self> {
        ProjectStore::new(root)?;
        Ok(Self { root: root.canonicalize().map_err(io_error)?, ffmpeg: ffmpeg.into(), workers: Mutex::new(Vec::new()), closing: AtomicBool::new(false) })
    }
    pub fn start_named_export(&self, id: &str, revision: u64, preset: ExportPreset, basename: &str) -> CoreResult<JobStatus> {
        paths::validate_export_basename(basename)?;
        let executable = self.ffmpeg.clone();
        self.start_job_with(id, revision, preset, None, basename, move |_, args| {
            let mut command = Command::new(&executable); command.args(args); command
        })
    }
    pub fn start_preview(&self, id: &str, revision: u64, request: crate::preview::PreviewRequest) -> CoreResult<JobStatus> {
        let executable = self.ffmpeg.clone();
        let basename = if matches!(request, crate::preview::PreviewRequest::Still { .. }) { "output.png" } else { OUTPUT };
        self.start_job_with(id, revision, ExportPreset::Balanced, Some(request), basename, move |_, args| {
            let mut command = Command::new(&executable); command.args(args); command
        })
    }
    pub fn output(&self, id: &str, job_id: &str) -> CoreResult<JobOutput> {
        let status = self.status(id, job_id)?;
        require(status.state == JobState::Completed, "output", "Output is available only for completed jobs")?;
        let project = paths::project_dir(&self.root, id)?;
        let dir = paths::job_dir(&project, job_id)?;
        // A local in-memory status cannot bypass strict persisted resource validation.
        let record = read_record(&dir, id, job_id)?;
        require(record.status.state == JobState::Completed, "output", "Completed output status has not been persisted")?;
        let metadata = record.status.output_metadata;
        let path = paths::job_file(&dir, metadata.as_ref().map_or(OUTPUT, |m| m.basename.as_str()))?;
        let size_bytes = fs::metadata(&path).map_err(io_error)?.len();
        Ok(JobOutput { path, size_bytes, mime_type: metadata.as_ref().map_or("video/mp4", |m| m.mime_type.as_str()).into(), metadata })
    }
    pub fn read_output_chunk(&self, id: &str, job_id: &str, offset: u64, length: u32) -> CoreResult<OutputChunk> {
        require((1..=1_048_576).contains(&length), "length", "Chunk length must be between 1 and 1048576 bytes")?;
        let output = self.output(id, job_id)?;
        require(offset <= output.size_bytes, "offset", "Chunk offset exceeds output size")?;
        let mut file = File::open(&output.path).map_err(io_error)?;
        let total_bytes = file.metadata().map_err(io_error)?.len();
        require(total_bytes == output.size_bytes, "output", "Output changed while opening the resource")?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        let count = (total_bytes - offset).min(length as u64) as usize;
        let mut bytes = vec![0; count]; file.read_exact(&mut bytes).map_err(io_error)?;
        Ok(OutputChunk { bytes, total_bytes, offset, eof: offset + count as u64 == total_bytes })
    }
    pub fn start_export(&self, id: &str, expected_revision: u64, preset: ExportPreset) -> CoreResult<JobStatus> {
        self.start_named_export(id, expected_revision, preset, OUTPUT)
    }
    #[cfg(test)]
    fn start_with(&self, id: &str, revision: u64, preset: ExportPreset,
        launch: impl Fn(&Path, Vec<OsString>) -> Command + Send + 'static) -> CoreResult<JobStatus> {
        self.start_job_with(id, revision, preset, None, OUTPUT, launch)
    }
    fn start_job_with(&self, id: &str, revision: u64, preset: ExportPreset,
        request: Option<crate::preview::PreviewRequest>, basename: &str,
        launch: impl Fn(&Path, Vec<OsString>) -> Command + Send + 'static) -> CoreResult<JobStatus> {
        let mut workers = self.workers.lock().map_err(|_| invalid("jobs", "Job manager lock unavailable"))?;
        if self.closing.load(Ordering::Acquire) { return Err(CoreError { code: ErrorCode::Cancelled, message: "Export manager is shutting down; no new exports can start".into(), field: None }); }
        let store = ProjectStore::new(&self.root)?;
        let render_lock = store.lock_render(id)?;
        let mut snapshot = store.snapshot(id, revision)?;
        let plan = render::compile(&snapshot.document, preset)?;
        let selection = request.as_ref().map(|r| crate::preview::select(&plan, r)).transpose()?;
        let kind = match request { None => JobKind::Export, Some(crate::preview::PreviewRequest::Video { .. }) => JobKind::PreviewVideo,
            Some(crate::preview::PreviewRequest::Still { .. }) => JobKind::PreviewStill };
        if let Some(selection) = &selection {
            // Check the aggregate decoder/input budget before creating a job or spawning a child.
            crate::preview::inputs(&snapshot, &plan, selection, kind != JobKind::PreviewStill)?;
        }
        let g = crate::frame::geometry(&snapshot.document.frame_style, snapshot.document.video.width, snapshot.document.video.height)?;
        let (width, height) = if selection.is_some() { crate::preview::dimensions(g.width, g.height) } else { (g.width,g.height) };
        let fps = preset.fps();
        let first_frame = selection.as_ref().map_or(0, |s| s.first_frame);
        let frame_count = selection.as_ref().map_or(render::grid_boundary(plan.duration_us, fps)?, |s| s.frame_count);
        let metadata = OutputMetadata { basename: basename.into(), mime_type: if kind == JobKind::PreviewStill { "image/png" } else { "video/mp4" }.into(),
            width, height, first_frame, frame_count, fps, start_us: first_frame * 1_000_000 / fps as u64,
            duration_us: frame_count * 1_000_000 / fps as u64 };
        let (job_id, dir) = reserve(&snapshot.project_dir)?;
        let status = JobStatus { job_id, project_id: id.into(), revision, kind, preview_request: request, output_metadata: Some(metadata), state: JobState::Running,
            percentage: 0.0, output_relative_path: None, error: None,
            warnings: snapshot.document.warnings.iter().map(|warning| format!("{warning:?}")).collect() };
        save(&dir, preset, &status)?;
        let control = Arc::new(Control { cancel: AtomicBool::new(false), status: Mutex::new(status.clone()) });
        let owned = control.clone();
        // Finished handles need no retained history; job records are the durable history.
        let mut index = 0;
        while index < workers.len() {
            if workers[index].thread.is_finished() { let _ = workers.swap_remove(index).thread.join(); }
            else { index += 1; }
        }
        let thread = std::thread::Builder::new().name(format!("export-{}", status.job_id)).spawn(move || {
            let _render_lock = render_lock;
            let mut current = lock_status(&owned).clone();
            let result = snapshot.prepare_cursor_with_cancel(&|| Ok(owned.cancel.load(Ordering::Acquire) || cancel_requested(&dir)?)).and_then(|()| snapshot.prepare_focus(&plan,&|| Ok(owned.cancel.load(Ordering::Acquire) || cancel_requested(&dir)?))).and_then(|()| encode(&dir, &snapshot, &plan, selection.as_ref(), &owned, &mut current, &launch));
            match result {
                Ok(()) => {
                    current.state = JobState::Completed; current.percentage = 100.0;
                    current.output_relative_path = Some(output_relative(&current));
                    if current.kind == JobKind::Export {
                        if let Err(error) = store.refresh_export_summary(&current.project_id) {
                            current.warnings.push(format!("Export completed, but session summary refresh failed: {error}"));
                        }
                    }
                }
                Err(error) => {
                    current.state = if error.code == ErrorCode::Cancelled { JobState::Cancelled } else { JobState::Failed };
                    current.error = Some(error);
                    // Only our partial is disposable; leave scripts and prior exports recoverable.
                    let _ = fs::remove_file(dir.join(partial_name(current.kind)));
                }
            }
            if let Err(error) = save(&dir, preset, &current) {
                current.warnings.push(format!("Could not persist terminal job status: {error}"));
            }
            *lock_status(&owned) = current;
        }).map_err(io_error)?;
        workers.push(Worker { control, thread });
        Ok(status)
    }
    pub fn status(&self, id: &str, job_id: &str) -> CoreResult<JobStatus> {
        let project = paths::project_dir(&self.root, id)?;
        let dir = paths::job_dir(&project, job_id)?;
        if let Some(status) = self.local_status(id, job_id)? { return Ok(status); }
        let record = read_record(&dir, id, job_id)?;
        if record.status.state != JobState::Running { return Ok(record.status); }
        let store = ProjectStore::new(&self.root)?;
        let _render_lock = match store.lock_render(id) {
            Ok(lock) => lock,
            Err(error) if error.code == ErrorCode::Busy => return Ok(record.status),
            Err(error) => return Err(error),
        };
        let mut record = read_record(&dir, id, job_id)?;
        if record.status.state == JobState::Running {
            // A completed publication can outlive a failed job-record refresh or a crash after rename.
            if published(&dir, &record.status)? {
                record.status.state = JobState::Completed; record.status.percentage = 100.0;
                record.status.output_relative_path = Some(output_relative(&record.status));
                record.status.warnings.push("Recovered published output after an incomplete job status update".into());
            } else {
                record.status.state = JobState::Interrupted;
                record.status.error = Some(CoreError { code: ErrorCode::Interrupted,
                    message: "Export owner is no longer holding the render lock; temporary files retained. Any former encoder's state is unknown".into(), field: None });
            }
            save(&dir, record.preset, &record.status)?;
        }
        Ok(record.status)
    }
    pub fn cancel(&self, id: &str, job_id: &str) -> CoreResult<JobStatus> {
        let status = self.status(id, job_id)?;
        if status.state != JobState::Running { return Ok(status); }
        let project = paths::project_dir(&self.root, id)?;
        let dir = paths::job_dir(&project, job_id)?;
        match OpenOptions::new().write(true).create_new(true).open(dir.join("cancel")) {
            Ok(mut file) => file.write_all(b"1").and_then(|_| file.sync_all()).map_err(io_error)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => { cancel_requested(&dir)?; }
            Err(error) => return Err(io_error(error)),
        }
        let workers = self.workers.lock().map_err(|_| invalid("jobs", "Job manager lock unavailable"))?;
        for worker in workers.iter() {
            let current = lock_status(&worker.control);
            if current.project_id == id && current.job_id == job_id { worker.control.cancel.store(true, Ordering::Release); }
        }
        drop(workers);
        self.status(id, job_id)
    }
    pub fn wait(&self, id: &str, job_id: &str) -> CoreResult<JobStatus> {
        loop {
            let status = self.status(id, job_id)?;
            if status.state != JobState::Running {
                // Joining also waits for the final render-lock release, so delete/start can follow wait.
                let mut workers = self.workers.lock().map_err(|_| invalid("jobs", "Job manager lock unavailable"))?;
                if let Some(index) = workers.iter().position(|w| {
                    let status = lock_status(&w.control); status.project_id == id && status.job_id == job_id
                }) { let _ = workers.swap_remove(index).thread.join(); }
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    /// Desktop exit must call this even if other application handles still retain the manager.
    pub fn shutdown(&self) -> CoreResult<()> {
        self.closing.store(true, Ordering::Release);
        let mut workers = self.workers.lock().unwrap_or_else(|poison| poison.into_inner());
        for worker in workers.iter() { worker.control.cancel.store(true, Ordering::Release); }
        let mut failed = false;
        for worker in workers.drain(..) { failed |= worker.thread.join().is_err(); }
        if failed { return Err(invalid("jobs", "An export worker stopped unexpectedly during shutdown; inspect its saved job record")); }
        Ok(())
    }
    fn local_status(&self, id: &str, job_id: &str) -> CoreResult<Option<JobStatus>> {
        let workers = self.workers.lock().map_err(|_| invalid("jobs", "Job manager lock unavailable"))?;
        for worker in workers.iter() {
            let status = lock_status(&worker.control);
            if status.project_id == id && status.job_id == job_id {
                if status.state == JobState::Running && worker.thread.is_finished() { return Ok(None); }
                return Ok(Some(status.clone()));
            }
        }
        Ok(None)
    }
}
impl Drop for JobManager {
    fn drop(&mut self) { let _ = self.shutdown(); }
}

fn lock_status(control: &Control) -> std::sync::MutexGuard<'_, JobStatus> {
    control.status.lock().unwrap_or_else(|poison| poison.into_inner())
}
fn encode(dir: &Path, snapshot: &RenderSnapshot, plan: &render::RenderPlan, selection: Option<&crate::preview::PreviewSelection>, control: &Control,
    current: &mut JobStatus, launch: &impl Fn(&Path, Vec<OsString>) -> Command) -> CoreResult<()> {
    let cancelled = || Ok(control.cancel.load(Ordering::Acquire) || cancel_requested(dir)?);
    if snapshot.document.cursor_settings.enabled || plan.follow.is_some() {
        encode_cursor(dir,snapshot,plan,selection,control,current,launch)?;
        return publish(dir,current,&cancelled);
    }
    let mut audio = snapshot.document.microphone.is_some() && current.kind != JobKind::PreviewStill;
    let mut last_save = Instant::now();
    loop {
        if cancelled()? { return Err(ffmpeg::cancel_error()); }
        let graph = match selection { Some(s) => render::preview_filter_graph(snapshot, plan, s, audio)?, None => render::filter_graph(snapshot, plan, audio)? };
        store::atomic_write(dir, ffmpeg::FILTER_SCRIPT, graph.as_bytes())?;
        let args = match selection { Some(s) => ffmpeg::preview_args(snapshot, plan, s, dir, audio, current.kind == JobKind::PreviewStill)?, None => ffmpeg::render_args(snapshot, plan, dir, audio)? };
        let result = ffmpeg::run_encoder(launch(dir, args), &cancelled, |time| {
            if last_save.elapsed() >= Duration::from_millis(250) {
                let duration = selection.map_or(plan.duration_us, |s| s.duration_us);
                let percentage = ((time.min(duration) as f64 / duration as f64) * 100.0).min(99.0);
                if percentage > current.percentage {
                    current.percentage = percentage;
                    save(dir, plan.preset, current)?;
                    *lock_status(control) = current.clone();
                }
                last_save = Instant::now();
            }
            Ok(())
        });
        if cancelled()? { return Err(ffmpeg::cancel_error()); }
        match result {
            Ok(()) => break,
            Err(error) if audio && error.code == ErrorCode::EncoderFailed => {
                audio = false;
                current.warnings.push(format!("Audio export failed; retrying once without audio: {error}"));
                let _ = fs::remove_file(dir.join(partial_name(current.kind)));
            }
            Err(error) => return Err(error),
        }
    }
    publish(dir,current,&cancelled)
}
fn publish(dir:&Path,current:&JobStatus,cancelled:&impl Fn()->CoreResult<bool>)->CoreResult<()> {
    let partial_name = partial_name(current.kind);
    let basename = current.output_metadata.as_ref().map_or(OUTPUT, |m| m.basename.as_str());
    let partial = paths::job_file(dir, partial_name)?;
    // Windows FlushFileBuffers requires a handle opened with write access.
    let media = OpenOptions::new().read(true).write(true).open(partial).map_err(io_error)?;
    require(media.metadata().map_err(io_error)?.len() > 0, "output", "Encoder produced an empty output")?;
    media.sync_all().map_err(io_error)?;
    drop(media);
    if cancelled()? { return Err(ffmpeg::cancel_error()); }
    match fs::symlink_metadata(dir.join(basename)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
        Ok(_) => return Err(invalid("output", "Export destination already exists; it will not be replaced")),
    }
    fs::rename(dir.join(partial_name), dir.join(basename)).map_err(io_error)?;
    // Publication is terminal: later bookkeeping errors must not turn a valid output into Failed.
    Ok(())
}
fn reserve(project: &Path) -> CoreResult<(String, PathBuf)> {
    let exports = paths::real_child_directory(project, "exports", true)?;
    loop {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| invalid("jobId", "System clock predates the Unix epoch"))?.as_nanos();
        let id = format!("job-{time}-{}-{}", std::process::id(), NEXT_JOB.fetch_add(1, Ordering::Relaxed));
        let dir = exports.join(&id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
}
fn partial_name(kind: JobKind) -> &'static str { if kind == JobKind::PreviewStill { "output.partial.png" } else { ffmpeg::RENDER_OUTPUT } }
fn output_relative(status: &JobStatus) -> String {
    format!("exports/{}/{}", status.job_id, status.output_metadata.as_ref().map_or(OUTPUT, |m| m.basename.as_str()))
}
fn published(dir: &Path, status: &JobStatus) -> CoreResult<bool> {
    match paths::job_file(dir, status.output_metadata.as_ref().map_or(OUTPUT, |m| m.basename.as_str())) {
        Ok(path) => Ok(fs::metadata(path).map_err(io_error)?.len() > 0),
        Err(error) if error.code == ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
fn cancel_requested(dir: &Path) -> CoreResult<bool> {
    match paths::job_file(dir, "cancel") {
        Ok(path) => { require(fs::metadata(path).map_err(io_error)?.len() <= 1, "cancel", "Invalid cancellation request")?; Ok(true) }
        Err(error) if error.code == ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
fn save(dir: &Path, preset: ExportPreset, status: &JobStatus) -> CoreResult<()> {
    let record = JobRecord { schema_version: if status.output_metadata.is_some() { 2 } else { 1 }, preset, status: status.clone() };
    let bytes = serde_json::to_vec(&record).map_err(|e| invalid("job", e.to_string()))?;
    require(bytes.len() as u64 <= RECORD_LIMIT, "job", "Job record exceeds 2 MiB")?;
    store::atomic_write(dir, "job.json", &bytes)
}
fn read_record(dir: &Path, id: &str, job_id: &str) -> CoreResult<JobRecord> {
    let mut bytes = Vec::new();
    File::open(paths::job_file(dir, "job.json")?).and_then(|file| file.take(RECORD_LIMIT + 1).read_to_end(&mut bytes)).map_err(io_error)?;
    require(bytes.len() as u64 <= RECORD_LIMIT, "job", "Job record exceeds 2 MiB")?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Version { schema_version: u64 }
    let version: Version = serde_json::from_slice(&bytes).map_err(|e| invalid("job", format!("Invalid job record: {e}")))?;
    if !matches!(version.schema_version, 1 | 2) { return Err(CoreError { code: ErrorCode::UnsupportedSchema, message: "Only job schema versions 1 and 2 are supported".into(), field: Some("schemaVersion".into()) }); }
    let record: JobRecord = serde_json::from_slice(&bytes).map_err(|e| invalid("job", format!("Invalid job record: {e}")))?;
    let status = &record.status;
    validate_metadata(&record)?;
    require(status.project_id == id && status.job_id == job_id, "job", "Job record identity must match its directory")?;
    require(status.revision <= MAX_REVISION && status.percentage.is_finite() && (0.0..=100.0).contains(&status.percentage), "job", "Invalid revision or percentage")?;
    match status.state {
        JobState::Completed => {
            require(status.output_relative_path.as_deref() == Some(output_relative(status).as_str()) && status.error.is_none() && status.percentage == 100.0, "job", "Invalid completed job record")?;
            require(published(dir, status)?, "output", "Completed job output is missing or empty")?;
        }
        JobState::Running => require(status.output_relative_path.is_none() && status.error.is_none(), "job", "Invalid running job record")?,
        _ => require(status.output_relative_path.is_none() && status.error.is_some(), "job", "Invalid terminal job record")?,
    }
    Ok(record)
}
fn validate_metadata(record: &JobRecord) -> CoreResult<()> {
    let status = &record.status;
    if record.schema_version == 1 {
        return require(status.kind == JobKind::Export && status.preview_request.is_none() && status.output_metadata.is_none(), "job", "Legacy jobs must be ordinary exports without new metadata");
    }
    let m = status.output_metadata.as_ref().ok_or_else(|| invalid("job", "Job schema 2 requires output metadata"))?;
    require(m.width >= 2 && m.height >= 2 && m.width <= 16384 && m.height <= 16384 && m.width % 2 == 0 && m.height % 2 == 0,
        "job", "Invalid output dimensions")?;
    require(m.fps == record.preset.fps() && m.frame_count > 0 && m.first_frame.checked_add(m.frame_count).is_some_and(|end| end <= 86400 * m.fps as u64)
        && m.start_us == m.first_frame * 1_000_000 / m.fps as u64 && m.duration_us == m.frame_count * 1_000_000 / m.fps as u64,
        "job", "Invalid output sampling metadata")?;
    if status.kind == JobKind::Export {
        paths::validate_export_basename(&m.basename)?;
        return require(m.mime_type == "video/mp4" && m.first_frame == 0 && status.preview_request.is_none(), "job", "Invalid export metadata");
    }
    require(m.fps == 30 && m.frame_count <= 900 && m.width.max(m.height) <= 960, "job", "Preview exceeds output bounds")?;
    match (&status.preview_request, status.kind) {
        (Some(crate::preview::PreviewRequest::Video { start_us, end_us }), JobKind::PreviewVideo) => {
            require(*start_us < *end_us && *end_us <= 86_400_000_000 && end_us - start_us <= 30_000_000, "job", "Invalid video preview request")?;
            require(m.basename == OUTPUT && m.mime_type == "video/mp4" && m.first_frame == render::grid_boundary(*start_us, 30)?
                && m.first_frame + m.frame_count == render::grid_boundary(*end_us, 30)?, "job", "Video preview request does not match sampling metadata")
        }
        (Some(crate::preview::PreviewRequest::Still { at_us }), JobKind::PreviewStill) =>
            require(*at_us < 86_400_000_000 && m.basename == "output.png" && m.mime_type == "image/png" && m.frame_count == 1
                && m.first_frame == at_us * 30 / 1_000_000, "job", "Still request does not match sampling metadata"),
        _ => Err(invalid("job", "Preview kind and request must match")),
    }
}

fn io_error(error: std::io::Error) -> CoreError {
    CoreError { code: ErrorCode::Io, message: format!("Export file operation failed: {error}"), field: None }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command, time::{Instant, Duration}};
    mod common { use crate as focusflow_core; include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/common/mod.rs")); }
    fn setup() -> (common::TestDir, PathBuf, JobManager) {
        let temp = common::TestDir::new(); let project = temp.path().join("fixture");
        fs::create_dir(&project).unwrap(); common::write_legacy(&project);
        fs::write(project.join("project.json"), serde_json::to_vec(&common::fixture_project()).unwrap()).unwrap();
        let manager = JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap();
        (temp, project, manager)
    }
    fn child(mode: &str, dir: &Path) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "jobs::tests::test_child", "--ignored", "--nocapture"])
            .env("JOB_CHILD_MODE", mode).env("JOB_CHILD_DIR", dir); cmd
    }
    fn until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() { assert!(Instant::now() < deadline, "child did not reach barrier"); std::thread::sleep(Duration::from_millis(5)); }
    }
    #[test]
    #[ignore = "private child entry point"]
    fn test_child() {
        let Some(mode) = std::env::var_os("JOB_CHILD_MODE") else { return; };
        let dir = PathBuf::from(std::env::var_os("JOB_CHILD_DIR").unwrap());
        if mode == "remote_cancel" {
            let manager = JobManager::new(&dir, Path::new("ffmpeg")).unwrap();
            let job_id = std::env::var("JOB_CHILD_ID").unwrap();
            assert_eq!(manager.status("fixture", &job_id).unwrap().state, JobState::Running);
            manager.cancel("fixture", &job_id).unwrap();
        } else if mode == "busy" {
            let manager = JobManager::new(&dir, Path::new("absent-encoder")).unwrap();
            assert_eq!(manager.start_export("fixture", 0, ExportPreset::Balanced).unwrap_err().code, ErrorCode::Busy);
        } else {
            use std::io::Write;
            fs::write(dir.join("child.pid"), std::process::id().to_string()).unwrap();
            if std::env::var_os("JOB_CHILD_STAGE_BARRIER").is_some() {fs::write(dir.join("stage-started"),b"ready").unwrap();}
            if mode == "silent" { std::thread::sleep(Duration::from_secs(30)); return; }
            if mode == "gated_success" { until(|| dir.join("finish").exists()); }
            if mode == "progress" {
                let mut output = std::io::stdout().lock();
                let mut errors = std::io::stderr().lock();
                output.write_all(&vec![b'x'; 2 * 1024 * 1024]).unwrap();
                writeln!(output, "\nout_time_us=NaN\nout_time_us=-1\nout_time_us=18446744073709551616").unwrap();
                errors.write_all(&vec![b'e'; 2 * 1024 * 1024]).unwrap();
                for n in 1..=12 { writeln!(output, "out_time_us={}\nprogress=continue", n * 500_000).unwrap(); output.flush().unwrap(); std::thread::sleep(Duration::from_millis(50)); }
            }
            let output_name = std::env::var("JOB_OUTPUT_NAME").unwrap_or(ffmpeg::RENDER_OUTPUT.into());
            fs::write(dir.join(output_name), b"synthetic encoded output").unwrap();
            if mode == "cancel_success" || mode == "cancel_failure" { fs::write(dir.join("cancel"), b"1").unwrap(); }
            if mode == "collision" { fs::write(dir.join(OUTPUT), b"existing destination").unwrap(); }
            if mode == "partial_directory" { fs::remove_file(dir.join(ffmpeg::RENDER_OUTPUT)).unwrap(); fs::create_dir(dir.join(ffmpeg::RENDER_OUTPUT)).unwrap(); }
            if mode == "record_directory" { fs::remove_file(dir.join("job.json")).unwrap(); fs::create_dir(dir.join("job.json")).unwrap(); }
            #[cfg(unix)]
            if mode == "readonly_dir" || mode == "summary_readonly" {
                use std::os::unix::fs::PermissionsExt;
                let target = if mode == "readonly_dir" { dir.clone() } else { dir.parent().unwrap().parent().unwrap().to_path_buf() };
                fs::set_permissions(target, fs::Permissions::from_mode(0o555)).unwrap();
            }
            if mode == "binary_failure" { std::io::stderr().write_all(&vec![0; 200 * 1024]).unwrap(); std::process::exit(9); }
            if mode == "failure" || mode == "cancel_failure" {
                std::io::stderr().write_all(&vec![b'e'; 200 * 1024]).unwrap(); std::process::exit(9);
            }
        }
    }

    #[test]
    fn second_process_is_busy_and_delete_cannot_race() {
        let (temp, project, manager) = setup();
        let started = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("silent", dir)).unwrap();
        assert!(child("busy", temp.path()).status().unwrap().success());
        let store = crate::store::ProjectStore::new(temp.path()).unwrap();
        assert_eq!(store.delete_project("fixture").unwrap_err().code, ErrorCode::Busy);
        assert!(project.exists());
        manager.cancel("fixture", &started.job_id).unwrap();
        assert_eq!(manager.wait("fixture", &started.job_id).unwrap().state, JobState::Cancelled);
    }
    #[test]
    fn cancel_reaps_silent_encoder() {
        let (_temp, project, manager) = setup();
        fs::create_dir_all(project.join("exports/previous")).unwrap();
        fs::write(project.join("exports/previous/output.mp4"), b"previous good export").unwrap();
        fs::write(project.join("edited.mp4"), b"legacy good export").unwrap();
        let originals: Vec<_> = ["screen.mp4", "metadata.json", "timeline.json", "clicks.json", "drags.json", "session.json", "project.json", "edited.mp4", "exports/previous/output.mp4"]
            .iter().map(|name| (*name, fs::read(project.join(name)).unwrap())).collect();
        let started = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("silent", dir)).unwrap();
        let dir = project.join("exports").join(&started.job_id);
        until(|| dir.join("child.pid").exists());
        let at = Instant::now();
        manager.cancel("fixture", &started.job_id).unwrap();
        assert_eq!(manager.wait("fixture", &started.job_id).unwrap().state, JobState::Cancelled);
        assert!(at.elapsed() < Duration::from_secs(2));
        assert!(!dir.join("output.mp4").exists());
        for (name, bytes) in originals { assert_eq!(fs::read(project.join(name)).unwrap(), bytes, "{name}"); }
        #[cfg(target_os = "linux")]
        assert!(!Path::new("/proc").join(fs::read_to_string(dir.join("child.pid")).unwrap()).exists(), "child must be reaped, not a zombie");
    }
    fn add_audio(project: &Path) {
        let mut writer = hound::WavWriter::create(project.join("mic.wav"), hound::WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }).unwrap();
        for _ in 0..8000 { writer.write_sample(100_i16).unwrap(); } writer.finalize().unwrap();
        let mut doc = common::fixture_project(); doc.microphone = Some(crate::project::AudioAsset { path: "mic.wav".into() });
        fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    }
    #[test]
    fn cancellation_before_retry_and_before_publication_never_publishes() {
        for mode in ["cancel_failure", "cancel_success"] {
            let (_temp, project, manager) = setup(); add_audio(&project);
            let count = Arc::new(AtomicU64::new(0)); let launches = count.clone();
            let job = manager.start_with("fixture", 0, ExportPreset::Balanced, move |dir, _| {
                launches.fetch_add(1, Ordering::Relaxed); child(mode, dir)
            }).unwrap();
            let done = manager.wait("fixture", &job.job_id).unwrap();
            assert_eq!(done.state, JobState::Cancelled); assert_eq!(count.load(Ordering::Relaxed), 1);
            assert!(!project.join("exports").join(&job.job_id).join(OUTPUT).exists());
            assert!(!done.warnings.iter().any(|w| w.contains("retrying")));
        }
    }
    #[test]
    fn audio_failure_retries_once_and_retry_can_be_cancelled() {
        for cancel_retry in [false, true] {
            let (_temp, project, manager) = setup(); add_audio(&project);
            let original = fs::read(project.join("mic.wav")).unwrap();
            let count = Arc::new(AtomicU64::new(0)); let launches = count.clone();
            let job = manager.start_with("fixture", 0, ExportPreset::Balanced, move |dir, args| {
                let attempt = launches.fetch_add(1, Ordering::Relaxed);
                if attempt == 0 { assert!(!args.contains(&OsString::from("-an"))); child("failure", dir) }
                else { assert!(args.contains(&OsString::from("-an"))); child(if cancel_retry { "silent" } else { "success" }, dir) }
            }).unwrap();
            until(|| count.load(Ordering::Relaxed) == 2);
            if cancel_retry { manager.cancel("fixture", &job.job_id).unwrap(); }
            let done = manager.wait("fixture", &job.job_id).unwrap();
            assert_eq!(done.state, if cancel_retry { JobState::Cancelled } else { JobState::Completed });
            assert_eq!(count.load(Ordering::Relaxed), 2);
            assert!(done.warnings.iter().any(|w| w.contains("retrying once without audio")));
            assert_eq!(original, fs::read(project.join("mic.wav")).unwrap());
        }
    }
    #[test]
    fn failed_second_attempt_stops_and_diagnostics_are_bounded() {
        let (_temp, project, manager) = setup(); add_audio(&project);
        let count = Arc::new(AtomicU64::new(0)); let launches = count.clone();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, move |dir, _| { launches.fetch_add(1, Ordering::Relaxed); child("failure", dir) }).unwrap();
        let done = manager.wait("fixture", &job.job_id).unwrap();
        assert_eq!(done.state, JobState::Failed); assert_eq!(count.load(Ordering::Relaxed), 2);
        assert!(done.error.unwrap().message.len() <= ffmpeg::DIAGNOSTIC_LIMIT + 100);
        assert!(fs::metadata(project.join("exports").join(&job.job_id).join("job.json")).unwrap().len() <= RECORD_LIMIT);
    }
    #[test]
    fn bounded_concurrent_drains_persist_progress_at_most_four_times_per_second() {
        let (_temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("progress", dir)).unwrap();
        let dir = project.join("exports").join(&job.job_id);
        let mut changes = Vec::new(); let mut previous = 0.0;
        loop {
            let status = manager.status("fixture", &job.job_id).unwrap();
            if status.state != JobState::Running { break; }
            let record = read_record(&dir, "fixture", &job.job_id).unwrap();
            // The worker may publish its terminal record between the two reads.
            if record.status.state != JobState::Running { break; }
            if record.status.percentage > previous {
                changes.push(Instant::now()); previous = record.status.percentage;
                assert!((0.0..=99.0).contains(&previous));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(previous > 0.0); assert!(changes.len() <= 3, "{changes:?}");
        for window in changes.windows(2) { assert!(window[1].duration_since(window[0]) >= Duration::from_millis(235)); }
        assert_eq!(manager.wait("fixture", &job.job_id).unwrap().state, JobState::Completed);
    }
    #[test]
    fn dropping_manager_cancels_and_reaps_owned_worker() {
        let (temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("silent", dir)).unwrap();
        let dir = project.join("exports").join(&job.job_id);
        until(|| dir.join("child.pid").exists()); let at = Instant::now(); drop(manager);
        assert!(at.elapsed() < Duration::from_secs(2));
        #[cfg(target_os = "linux")]
        assert!(!Path::new("/proc").join(fs::read_to_string(dir.join("child.pid")).unwrap()).exists());
        let reopened = JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap().state, JobState::Cancelled);
    }
    #[test]
    fn publication_failure_and_existing_destination_are_preserved() {
        for mode in ["partial_directory", "collision"] {
            let (_temp, project, manager) = setup();
            let original = fs::read(project.join("screen.mp4")).unwrap();
            let job = manager.start_with("fixture", 0, ExportPreset::Balanced, move |dir, _| child(mode, dir)).unwrap();
            assert_eq!(manager.wait("fixture", &job.job_id).unwrap().state, JobState::Failed);
            assert_eq!(original, fs::read(project.join("screen.mp4")).unwrap());
            if mode == "collision" { assert_eq!(fs::read(project.join("exports").join(&job.job_id).join(OUTPUT)).unwrap(), b"existing destination"); }
        }
    }
    #[test]
    fn terminal_record_write_failure_keeps_published_media_but_does_not_invent_metadata() {
        let (temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("record_directory", dir)).unwrap();
        let done = manager.wait("fixture", &job.job_id).unwrap();
        assert_eq!(done.state, JobState::Completed);
        assert!(done.warnings.iter().any(|w| w.contains("persist terminal")));
        assert!(project.join(done.output_relative_path.unwrap()).is_file());
        drop(manager);
        assert!(JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap().status("fixture", &job.job_id).is_err());
    }
    #[test]
    fn summary_refresh_failure_is_only_a_warning_and_keeps_original_bytes() {
        for bytes in [b"broken".as_slice(), br#"{"session_id":"a","session_id":"b","created_at":"x","duration_seconds":1,"recording_source":"screen","exported":false}"#, br#"{"session_id":"a","created_at":"x","duration_seconds":1,"recording_source":"screen","exported":false,"extra":1}"#] {
            let (_temp, project, manager) = setup(); fs::write(project.join("session.json"), bytes).unwrap();
            let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("success", dir)).unwrap();
            let done = manager.wait("fixture", &job.job_id).unwrap();
            assert_eq!(done.state, JobState::Completed);
            assert!(done.warnings.iter().any(|w| w.contains("summary refresh failed")));
            assert_eq!(fs::read(project.join("session.json")).unwrap(), bytes);
        }
    }
    #[test]
    fn reopened_records_are_validated_and_recovery_holds_render_lock() {
        let (temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("success", dir)).unwrap();
        let done = manager.wait("fixture", &job.job_id).unwrap();
        assert_eq!(done.state, JobState::Completed);
        let dir = project.join("exports").join(&job.job_id);
        let reopened = JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap(), done);
        save(&dir, ExportPreset::Balanced, &job).unwrap();
        let store = ProjectStore::new(temp.path()).unwrap();
        let guard = store.lock_render("fixture").unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap().state, JobState::Running);
        drop(guard);
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap().state, JobState::Completed);
        fs::remove_file(dir.join(OUTPUT)).unwrap(); save(&dir, ExportPreset::Balanced, &job).unwrap();
        fs::write(dir.join(ffmpeg::RENDER_OUTPUT), b"recoverable partial").unwrap();
        let interrupted = reopened.status("fixture", &job.job_id).unwrap();
        assert_eq!(interrupted.state, JobState::Interrupted);
        assert!(interrupted.error.unwrap().message.contains("unknown"));
        assert_eq!(fs::read(dir.join(ffmpeg::RENDER_OUTPUT)).unwrap(), b"recoverable partial");
        fs::write(dir.join("job.json"), b"malformed").unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap_err().code, ErrorCode::InvalidDocument);
    }

    #[test]
    fn another_process_reads_and_cancels_running_job() {
        let (temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("silent", dir)).unwrap();
        let dir = project.join("exports").join(&job.job_id); until(|| dir.join("child.pid").exists());
        let at = Instant::now();
        assert!(child("remote_cancel", temp.path()).env("JOB_CHILD_ID", &job.job_id).status().unwrap().success());
        assert_eq!(manager.wait("fixture", &job.job_id).unwrap().state, JobState::Cancelled);
        assert!(at.elapsed() < Duration::from_secs(2));
        #[cfg(target_os = "linux")]
        assert!(!Path::new("/proc").join(fs::read_to_string(dir.join("child.pid")).unwrap()).exists());
    }
    #[test]
    fn explicit_shutdown_reaps_with_other_handles_and_rejects_new_work() {
        let (temp, project, manager) = setup(); let manager = Arc::new(manager);
        let retained = manager.clone();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("silent", dir)).unwrap();
        let dir = project.join("exports").join(&job.job_id); until(|| dir.join("child.pid").exists());
        let at = Instant::now(); manager.shutdown().unwrap();
        assert!(at.elapsed() < Duration::from_secs(2));
        #[cfg(target_os = "linux")]
        assert!(!Path::new("/proc").join(fs::read_to_string(dir.join("child.pid")).unwrap()).exists());
        assert_eq!(retained.start_export("fixture", 0, ExportPreset::Balanced).unwrap_err().code, ErrorCode::Cancelled);
        assert_eq!(retained.status("fixture", &job.job_id).unwrap().state, JobState::Cancelled);
        manager.shutdown().unwrap();
        ProjectStore::new(temp.path()).unwrap().delete_project("fixture").unwrap();
    }

    #[test]
    fn delete_during_render_is_busy() {
        let (temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("silent", dir)).unwrap();
        let dir = project.join("exports").join(&job.job_id); until(|| dir.join("child.pid").exists());
        let store = ProjectStore::new(temp.path()).unwrap();
        assert_eq!(store.delete_project("fixture").unwrap_err().code, ErrorCode::Busy);
        let edited = store.apply_edits("fixture", 0, &[crate::edits::Edit::TrimClip { clip_id: "clip-0".into(), in_us: 1_000_000, out_us: 4_000_000 }]).unwrap();
        assert_eq!(edited.revision, 1);
        assert_eq!(manager.status("fixture", &job.job_id).unwrap().revision, 0);
        manager.cancel("fixture", &job.job_id).unwrap(); manager.wait("fixture", &job.job_id).unwrap();
        store.delete_project("fixture").unwrap(); assert!(!project.exists());
    }
    #[test]
    fn escaped_diagnostics_fit_bounded_record_and_reopen() {
        let (temp, project, manager) = setup(); add_audio(&project);
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("binary_failure", dir)).unwrap();
        let done = manager.wait("fixture", &job.job_id).unwrap();
        assert_eq!(done.state, JobState::Failed);
        assert!(!done.warnings.iter().any(|w| w.contains("persist terminal")));
        assert_eq!(JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap().status("fixture", &job.job_id).unwrap(), done);
    }
    #[cfg(unix)]
    #[test]
    fn rename_and_summary_write_failures_are_distinguished() {
        use std::os::unix::fs::PermissionsExt;
        for mode in ["readonly_dir", "summary_readonly"] {
            let (_temp, project, manager) = setup();
            let summary = fs::read(project.join("session.json")).unwrap();
            let job = manager.start_with("fixture", 0, ExportPreset::Balanced, move |dir, _| child(mode, dir)).unwrap();
            let done = manager.wait("fixture", &job.job_id).unwrap();
            let dir = project.join("exports").join(&job.job_id);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&project, fs::Permissions::from_mode(0o755)).unwrap();
            if mode == "readonly_dir" {
                assert_eq!(done.state, JobState::Failed); assert_eq!(done.error.unwrap().code, ErrorCode::Io);
                assert!(!dir.join(OUTPUT).exists());
            } else {
                assert_eq!(done.state, JobState::Completed); assert!(dir.join(OUTPUT).is_file());
                assert!(done.warnings.iter().any(|w| w.contains("summary refresh failed")));
            }
            assert_eq!(fs::read(project.join("session.json")).unwrap(), summary);
        }
    }
    #[test]
    fn record_validation_rejects_wrong_identity_output_and_oversize() {
        let (temp, project, manager) = setup();
        let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("success", dir)).unwrap();
        manager.wait("fixture", &job.job_id).unwrap();
        let dir = project.join("exports").join(&job.job_id);
        let bytes = fs::read(dir.join("job.json")).unwrap();
        let reopened = JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap();
        for field in ["projectId", "jobId", "outputRelativePath"] {
            let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            record["status"][field] = serde_json::json!("../outside");
            fs::write(dir.join("job.json"), serde_json::to_vec(&record).unwrap()).unwrap();
            assert_eq!(reopened.status("fixture", &job.job_id).unwrap_err().code, ErrorCode::InvalidDocument);
        }
        let mut record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        record["status"]["percentage"] = serde_json::json!(101);
        fs::write(dir.join("job.json"), serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap_err().code, ErrorCode::InvalidDocument);
        fs::write(dir.join("job.json"), String::from_utf8(bytes).unwrap().replacen("{", "{\"schemaVersion\":1,", 1)).unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap_err().code, ErrorCode::InvalidDocument);
        File::create(dir.join("job.json")).unwrap().set_len(RECORD_LIMIT + 1).unwrap();
        assert_eq!(reopened.status("fixture", &job.job_id).unwrap_err().code, ErrorCode::InvalidDocument);
    }

    #[test]
    fn missing_summary_is_normal_and_document_lock_contention_is_a_warning() {
        for missing in [true, false] {
            let (temp, project, manager) = setup();
            if missing { fs::remove_file(project.join("session.json")).unwrap(); }
            let job = manager.start_with("fixture", 0, ExportPreset::Balanced, |dir, _| child("gated_success", dir)).unwrap();
            let dir = project.join("exports").join(&job.job_id); until(|| dir.join("child.pid").exists());
            let lock = File::options().read(true).write(true).open(temp.path().join(".locks/fixture.document")).unwrap();
            if !missing { lock.try_lock().unwrap(); }
            fs::write(dir.join("finish"), b"go").unwrap();
            let done = manager.wait("fixture", &job.job_id).unwrap();
            assert_eq!(done.state, JobState::Completed);
            assert_eq!(done.warnings.iter().any(|w| w.contains("summary refresh failed")), !missing);
            if !missing { lock.unlock().unwrap(); }
            else { assert!(!project.join("session.json").exists()); }
        }
    }

    fn preview_child(manager: &JobManager, request: crate::preview::PreviewRequest, mode: &'static str) -> JobStatus {
        let basename=if matches!(request,crate::preview::PreviewRequest::Still{..}) {"output.png"} else {OUTPUT};
        manager.start_job_with("fixture",0,ExportPreset::Balanced,Some(request),basename,move |dir,args| {
            let mut cmd=child(mode,dir);cmd.env("JOB_OUTPUT_NAME",Path::new(args.last().unwrap()).file_name().unwrap());cmd
        }).unwrap()
    }
    #[test]
    fn preview_publication_keeps_summary_and_snapshot_metadata_and_reopens() {
        use crate::preview::PreviewRequest;
        for request in [PreviewRequest::Video{start_us:33_334,end_us:200_001},PreviewRequest::Still{at_us:100_001}] {
            let (temp,project,manager)=setup();let before=fs::read(project.join("session.json")).unwrap();
            let job=preview_child(&manager,request.clone(),"gated_success");
            let dir=project.join("exports").join(&job.job_id);until(||dir.join("child.pid").exists());
            assert!(manager.output("fixture",&job.job_id).is_err());
            ProjectStore::new(temp.path()).unwrap().apply_edits("fixture",0,&[crate::edits::Edit::TrimClip{clip_id:"clip-0".into(),in_us:1_000_000,out_us:2_000_000}]).unwrap();
            let reopened=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();
            assert_eq!(reopened.status("fixture",&job.job_id).unwrap().state,JobState::Running);
            assert_eq!(reopened.start_export("fixture",1,ExportPreset::Balanced).unwrap_err().code,ErrorCode::Busy);
            fs::write(dir.join("finish"),b"go").unwrap();let done=manager.wait("fixture",&job.job_id).unwrap();
            assert_eq!(done.state,JobState::Completed,"{done:?}");assert_ne!(done.kind,JobKind::Export);
            assert_eq!(done.preview_request,Some(request));assert_eq!(done.revision,0);
            assert_eq!(done.output_metadata,job.output_metadata);
            assert_eq!(fs::read(project.join("session.json")).unwrap(),before);
            assert_eq!(reopened.status("fixture",&job.job_id).unwrap(),done);
            let output=reopened.output("fixture",&job.job_id).unwrap();
            assert_eq!(output.metadata,done.output_metadata);assert_eq!(output.size_bytes,24);
            let chunk=reopened.read_output_chunk("fixture",&job.job_id,output.size_bytes,1).unwrap();
            assert!(chunk.bytes.is_empty() && chunk.eof);
            let value:serde_json::Value=serde_json::from_slice(&fs::read(dir.join("job.json")).unwrap()).unwrap();
            assert_eq!(value["schemaVersion"],2);
            save(&dir,ExportPreset::Balanced,&job).unwrap();
            let recovered=reopened.status("fixture",&job.job_id).unwrap();
            assert_eq!(recovered.state,JobState::Completed);assert_eq!(recovered.kind,done.kind);
            assert_eq!(recovered.output_metadata,done.output_metadata);
            assert_eq!(fs::read(project.join("session.json")).unwrap(),before);
        }
    }
    #[test]
    fn preview_cancel_and_shutdown_reap_and_release_shared_guard() {
        for shutdown in [false,true] {
            let (temp,project,manager)=setup();let before=fs::read(project.join("session.json")).unwrap();
            let job=preview_child(&manager,crate::preview::PreviewRequest::Video{start_us:0,end_us:1_000_000},"silent");
            let dir=project.join("exports").join(&job.job_id);until(||dir.join("child.pid").exists());
            assert_eq!(manager.start_export("fixture",0,ExportPreset::Balanced).unwrap_err().code,ErrorCode::Busy);
            if shutdown { manager.shutdown().unwrap(); } else { manager.cancel("fixture",&job.job_id).unwrap(); }
            assert_eq!(manager.wait("fixture",&job.job_id).unwrap().state,JobState::Cancelled);
            assert!(manager.output("fixture",&job.job_id).is_err());
            #[cfg(target_os="linux")]
            assert!(!Path::new("/proc").join(fs::read_to_string(dir.join("child.pid")).unwrap()).exists());
            assert_eq!(fs::read(project.join("session.json")).unwrap(),before);
            let store=ProjectStore::new(temp.path()).unwrap();assert!(store.lock_render("fixture").is_ok());
        }
    }
    #[test]
    fn preview_audio_failure_retries_video_only_and_preserves_summary() {
        let (_temp,project,manager)=setup();add_audio(&project);let before=fs::read(project.join("session.json")).unwrap();
        let attempts=Arc::new(AtomicU64::new(0));let launches=attempts.clone();
        let job=manager.start_job_with("fixture",0,ExportPreset::Balanced,
            Some(crate::preview::PreviewRequest::Video{start_us:0,end_us:1_000_000}),OUTPUT,move |dir,args| {
                launches.fetch_add(1,Ordering::Relaxed);
                child(if args.contains(&OsString::from("[a]")) {"failure"} else {"success"},dir)
            }).unwrap();
        let done=manager.wait("fixture",&job.job_id).unwrap();assert_eq!(done.state,JobState::Completed,"{done:?}");
        assert_eq!(attempts.load(Ordering::Relaxed),2);assert!(done.warnings.iter().any(|w|w.contains("retrying once without audio")));
        assert_eq!(fs::read(project.join("session.json")).unwrap(),before);
    }
    #[test]
    fn named_export_publication_and_resource_validation_do_not_trust_local_status() {
        let (temp,project,manager)=setup();
        let job=manager.start_job_with("fixture",0,ExportPreset::HighQuality,None,"My demo.mp4",|dir,_|child("success",dir)).unwrap();
        let done=manager.wait("fixture",&job.job_id).unwrap();assert_eq!(done.state,JobState::Completed);
        let output=manager.output("fixture",&job.job_id).unwrap();assert_eq!(output.path.file_name().unwrap(),"My demo.mp4");
        assert_eq!(output.metadata.as_ref().unwrap().frame_count,300);assert_eq!(output.metadata.as_ref().unwrap().fps,60);
        let dir=output.path.parent().unwrap();let original=fs::read(dir.join("job.json")).unwrap();
        for (field,value) in [("basename",serde_json::json!("../screen.mp4")),("mimeType",serde_json::json!("text/plain")),("fps",serde_json::json!(0)),("firstFrame",serde_json::json!(u64::MAX)),("width",serde_json::json!(1)),("durationUs",serde_json::json!(1))] {
            let mut record:serde_json::Value=serde_json::from_slice(&original).unwrap();record["status"]["outputMetadata"][field]=value;
            fs::write(dir.join("job.json"),serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(manager.output("fixture",&job.job_id).is_err(),"accepted altered {field}");
            assert!(JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap().status("fixture",&job.job_id).is_err());
        }
        let mut record:serde_json::Value=serde_json::from_slice(&original).unwrap();record["status"].as_object_mut().unwrap().remove("outputMetadata");
        fs::write(dir.join("job.json"),serde_json::to_vec(&record).unwrap()).unwrap();assert!(manager.output("fixture",&job.job_id).is_err());
        fs::write(dir.join("job.json"),original).unwrap();
        #[cfg(unix)] {
            fs::remove_file(&output.path).unwrap();std::os::unix::fs::symlink(project.join("screen.mp4"),&output.path).unwrap();
            assert_eq!(manager.output("fixture",&job.job_id).unwrap_err().code,ErrorCode::InvalidPath);
            assert!(manager.read_output_chunk("fixture",&job.job_id,0,100).is_err());
        }
    }
    #[test]
    fn preview_metadata_rejects_kind_request_mime_and_sampling_mismatches() {
        let (_temp,project,manager)=setup();let job=preview_child(&manager,crate::preview::PreviewRequest::Still{at_us:100_001},"success");
        manager.wait("fixture",&job.job_id).unwrap();let dir=project.join("exports").join(&job.job_id);let original=fs::read(dir.join("job.json")).unwrap();
        for (pointer,value) in [("/status/kind",serde_json::json!("export")),("/status/previewRequest/atUs",serde_json::json!(0)),("/status/outputMetadata/frameCount",serde_json::json!(2)),("/status/outputMetadata/basename",serde_json::json!("screen.mp4")),("/status/outputMetadata/width",serde_json::json!(962)),("/schemaVersion",serde_json::json!(1))] {
            let mut record:serde_json::Value=serde_json::from_slice(&original).unwrap();*record.pointer_mut(pointer).unwrap()=value;
            fs::write(dir.join("job.json"),serde_json::to_vec(&record).unwrap()).unwrap();assert!(manager.output("fixture",&job.job_id).is_err(),"accepted {pointer}");
        }
    }


    #[test]
    fn cursor_cancellation_reaps_active_window_concat_and_audio_children() {
        for stage in ["cursor-segment.mp4","cursor-video.mp4","output.partial.mp4"] {
            let (_temp,project,manager)=setup();add_audio(&project);add_cursor(&project);
            let job=manager.start_with("fixture",0,ExportPreset::Balanced,move|dir,args|{
                let output=Path::new(args.last().unwrap()).file_name().unwrap();
                
                let mut command=child(if output==stage{"silent"}else{"success"},dir);
                command.env("JOB_OUTPUT_NAME",output);if output==stage {command.env("JOB_CHILD_STAGE_BARRIER","1");}command
            }).unwrap();
            let dir=project.join("exports").join(&job.job_id);
            until(||dir.join("stage-started").exists());
            manager.cancel("fixture",&job.job_id).unwrap();
            assert_eq!(manager.wait("fixture",&job.job_id).unwrap().state,JobState::Cancelled);
            assert!(!dir.join(OUTPUT).exists());
            #[cfg(target_os="linux")]
            if let Ok(pid)=fs::read_to_string(dir.join("child.pid")){assert!(!Path::new("/proc").join(pid).exists());}
            assert!(!fs::read_dir(&dir).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with("cursor-")));
        }
    }
    #[test]
    fn follow_only_failure_cancel_and_audio_fallback_preserve_source_and_cleanup() {
        for stage in ["cursor-segment.mp4","cursor-video.mp4","output.partial.mp4"] {
            for cancel in [false,true] {
                let (_temp,project,manager)=setup();add_audio(&project);add_cursor(&project);
                let mut doc:crate::project::ProjectDocument=serde_json::from_slice(&fs::read(project.join("project.json")).unwrap()).unwrap();
                doc.schema_version=4;doc.cursor_settings.enabled=false;
                doc.zooms=vec![crate::project::ZoomElement{mode:crate::project::ZoomMode::Follow,id:"z".into(),clip_id:"clip-0".into(),start_us:0,end_us:5_000_000,x:0.5,y:0.5,scale:2.0,intro_us:0,hold_us:0,outro_us:0}];
                fs::write(project.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
                let source=fs::read(project.join("screen.mp4")).unwrap();
                let job=manager.start_with("fixture",0,ExportPreset::Balanced,move|dir,args|{
                    assert!(!dir.join("cursor-atlas.pam").exists());
                    assert!(!args.iter().any(|a|a.to_string_lossy().contains(".pam")));
                    let output=Path::new(args.last().unwrap()).file_name().unwrap();
                    let mode=if output==stage {if cancel {"cancel_success"}else{"failure"}}else{"success"};
                    let mut command=child(mode,dir);command.env("JOB_OUTPUT_NAME",output);command
                }).unwrap();
                let done=manager.wait("fixture",&job.job_id).unwrap();
                assert_eq!(done.state,if cancel{JobState::Cancelled}else if stage=="output.partial.mp4"{JobState::Completed}else{JobState::Failed});
                assert_eq!(done.warnings.iter().any(|w|w.contains("retrying")),!cancel&&stage=="output.partial.mp4");
                assert!(done.warnings.iter().any(|w|w.contains("follow holds")));
                assert_eq!(fs::read(project.join("screen.mp4")).unwrap(),source);
                let dir=project.join("exports").join(&job.job_id);
                assert!(!fs::read_dir(&dir).unwrap().any(|e|e.unwrap().file_name().to_string_lossy().starts_with("cursor-")));
            }
        }
    }
    fn add_cursor(project:&Path) {
        let mut doc=common::fixture_project();doc.schema_version=3;doc.video.cursor=crate::project::CursorMode::Separate;
        doc.cursor=Some(crate::project::CursorAsset{index_path:"index.json".into()});doc.cursor_settings.enabled=true;doc.cursor_settings.style=crate::project::CursorStyle::Arrow;
        doc.microphone=Some(crate::project::AudioAsset{path:"mic.wav".into()});
        fs::write(project.join("index.json"),serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5000000,"samplesPath":"samples.ndjson","shapes":[]}).to_string()).unwrap();
        fs::write(project.join("samples.ndjson"),serde_json::json!({"atUs":0,"x":0.5,"y":0.5,"visible":true,"shapeId":null,"discontinuity":false}).to_string()).unwrap();
        fs::write(project.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
    }
    #[test]
    fn cursor_stage_failures_and_cancellation_preserve_sources_and_never_reencode() {
        for stage in ["cursor-segment.mp4","cursor-video.mp4","output.partial.mp4"] {
            for cancel in [false,true] {
                let (_temp,project,manager)=setup();add_audio(&project);add_cursor(&project);
                fs::write(project.join("edited.mp4"),b"previous").unwrap();
                let before:Vec<_>=["screen.mp4","mic.wav","project.json","samples.ndjson","edited.mp4"].map(|name|(name,fs::read(project.join(name)).unwrap())).into();
                let calls=Arc::new(AtomicU64::new(0));let launches=calls.clone();
                let job=manager.start_with("fixture",0,ExportPreset::Balanced,move|dir,args|{
                    launches.fetch_add(1,Ordering::Relaxed);
                    let output=Path::new(args.last().unwrap()).file_name().unwrap();
                    let mode=if output==stage {if cancel {"cancel_success"}else{"failure"}}else{"success"};
                    let mut command=child(mode,dir);command.env("JOB_OUTPUT_NAME",output);command
                }).unwrap();
                let done=manager.wait("fixture",&job.job_id).unwrap();
                let expected=if cancel{JobState::Cancelled}else if stage=="output.partial.mp4"{JobState::Completed}else{JobState::Failed};
                assert_eq!(done.state,expected,"{stage} {cancel}: {done:?}");
                assert_eq!(calls.load(Ordering::Relaxed),match stage{"cursor-segment.mp4"=>1,"cursor-video.mp4"=>2,_=>3});
                assert_eq!(done.warnings.iter().any(|w|w.contains("retrying")),!cancel&&stage=="output.partial.mp4");
                for(name,bytes)in before{assert_eq!(fs::read(project.join(name)).unwrap(),bytes);}
                let dir=project.join("exports").join(&job.job_id);
                assert!(!fs::read_dir(&dir).unwrap().any(|entry|entry.unwrap().file_name().to_string_lossy().starts_with("cursor-")));
            }
        }
    }

}

// Every temporary is inside the reserved job directory and is removed on all exit paths.
struct CursorScratch { paths: Vec<PathBuf> }
impl Drop for CursorScratch {fn drop(&mut self){for path in &self.paths {let _=fs::remove_file(path);}}}
fn encode_cursor(dir:&Path,snapshot:&RenderSnapshot,plan:&render::RenderPlan,selection:Option<&crate::preview::PreviewSelection>,control:&Control,current:&mut JobStatus,launch:&impl Fn(&Path,Vec<OsString>)->Command)->CoreResult<()> {
 let cancelled=||Ok(control.cancel.load(Ordering::Acquire)||cancel_requested(dir)?);
 let mut scratch=CursorScratch{paths:["cursor-atlas.pam","cursor-segment.mp4","cursor-concat.txt","cursor-video.mp4"].map(|p|dir.join(p)).into()};
 let first=selection.map_or(0,|s|s.first_frame);let end=first+selection.map_or(render::grid_boundary(plan.duration_us,plan.preset.fps())?,|s|s.frame_count);
 let mut next=first;let mut segments=String::new();let mut segment=0;
 if snapshot.prepared_cursor.as_ref().is_some_and(|c|c.has_hidden_or_stale_gaps){current.warnings.push(if plan.follow.is_some(){"Cursor telemetry contains hidden or stale intervals; follow holds the last eligible focus target or uses fallback focus".into()}else{"Cursor telemetry contains hidden or stale intervals; those frames are transparent".into()});}
 while next<end {
  if cancelled()?{return Err(ffmpeg::cancel_error());}
  let (window,graph)=render::cursor_window::next_with_graph(snapshot,plan,next,end,selection,&control.cancel)?;
  if let Some(atlas)=&window.atlas {store::atomic_write(dir,"cursor-atlas.pam",atlas)?;}
  store::atomic_write(dir,ffmpeg::FILTER_SCRIPT,graph.as_bytes())?;
  let args=ffmpeg::cursor_window_args(snapshot,plan,&window,dir,current.kind==JobKind::PreviewStill)?;
  let mut last=Instant::now();
  ffmpeg::run_encoder(launch(dir,args),&cancelled,|time|{
   if last.elapsed()>=Duration::from_millis(250){
    let frames=time.saturating_mul(plan.preset.fps() as u64)/1_000_000;
    current.percentage=(((next-first+frames.min(window.count)) as f64/(end-first) as f64)*90.0).min(90.0);
    save(dir,plan.preset,current)?;*lock_status(control)=current.clone();last=Instant::now();
   }Ok(())
  })?;
  if window.atlas.is_some() {fs::remove_file(dir.join("cursor-atlas.pam")).map_err(io_error)?;}
  if current.kind==JobKind::PreviewStill{return Ok(());}
  let name=format!("cursor-{segment}.mp4");scratch.paths.push(dir.join(&name));
  fs::rename(dir.join("cursor-segment.mp4"),dir.join(&name)).map_err(io_error)?;
  segments.push_str(&format!("file '{name}'\n"));segment+=1;next+=window.count;
 }
 store::atomic_write(dir,"cursor-concat.txt",segments.as_bytes())?;
 ffmpeg::run_encoder(launch(dir,ffmpeg::cursor_concat_args(dir,plan.preset.fps())),&cancelled,|_|Ok(()))?;
 if cancelled()?{return Err(ffmpeg::cancel_error());}
 let audio=render::mux_audio_graph(snapshot,plan,selection)?;
 if !audio.is_empty(){
  store::atomic_write(dir,ffmpeg::FILTER_SCRIPT,audio.as_bytes())?;
  let result=ffmpeg::run_encoder(launch(dir,ffmpeg::cursor_audio_args(snapshot,plan,selection,dir)?),&cancelled,|_|Ok(()));
  if cancelled()?{return Err(ffmpeg::cancel_error());}
  match result {
   Ok(())=>return Ok(()),
   Err(e) if e.code==ErrorCode::EncoderFailed=>{current.warnings.push(format!("Audio export failed; retrying once without audio using completed video: {e}"));let _=fs::remove_file(dir.join(ffmpeg::RENDER_OUTPUT));},
   Err(e)=>return Err(e),
  }
 }
 fs::rename(dir.join("cursor-video.mp4"),dir.join(ffmpeg::RENDER_OUTPUT)).map_err(io_error)
}
