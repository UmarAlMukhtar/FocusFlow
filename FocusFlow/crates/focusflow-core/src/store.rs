use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use crate::{
    audio::is_usable_wav, edits::{self, Edit}, ffmpeg::probe_video,
    legacy::{read_legacy, LegacyDefaults}, paths,
    project::{self, invalid, require, ProjectDocument, ProjectWarning, MAX_JSON_BYTES, MAX_REVISION},
    CoreError, CoreResult, ErrorCode,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub struct ProjectStore { root: PathBuf }

#[derive(Debug, Clone)]
pub struct RenderSnapshot {
    pub project_dir: PathBuf,
    pub document: ProjectDocument,
    pub prepared_cursor: Option<std::sync::Arc<crate::cursor::PreparedCursor>>,
    pub prepared_focus: Option<std::sync::Arc<crate::render::follow::PreparedFocus>>,
}

impl RenderSnapshot {
    pub fn prepare_focus(&mut self,plan:&crate::render::RenderPlan,cancel:&impl Fn()->CoreResult<bool>)->CoreResult<()> {
        crate::render::validate_plan(self,plan)?;
        self.prepared_focus=if plan.follow.is_some() {
            let cursor=self.prepared_cursor.as_ref().ok_or_else(||invalid("cursor","Follow positions were not prepared"))?;
            crate::render::follow::PreparedFocus::prepare(plan,cursor,cancel)?
        } else {None};
        Ok(())
    }
    pub fn prepare_cursor_with_cancel(&mut self, cancel: &impl Fn() -> CoreResult<bool>) -> CoreResult<()> {
        self.prepared_focus = None;
        self.prepared_cursor = crate::cursor::prepare_cursor_with_cancel(&self.project_dir, &self.document, cancel)?;
        Ok(())
    }
    /// Run in the reserved job worker; positions are also needed when follow is enabled.
    pub fn prepare_cursor(&mut self, cancel: &std::sync::atomic::AtomicBool) -> CoreResult<()> {
        self.prepared_focus = None;
        self.prepared_cursor = crate::cursor::prepare_cursor(&self.project_dir, &self.document, cancel)?;
        Ok(())
    }
}

// Keeping the file open owns the OS lock; the persistent lock file is never unlinked.
pub(crate) struct ProjectLock { _file: File }
impl Drop for ProjectLock {
    fn drop(&mut self) {
        // A forked child can retain this open file description until exec; close alone can delay release.
        let _ = self._file.unlock();
    }
}

impl ProjectStore {
    /// Restore only editable fields against the latest revision, never source metadata or history.
    pub fn restore_edits(&self, id: &str, expected_revision: u64, state: &project::EditableState) -> CoreResult<ProjectDocument> {
        require((1..=4096).contains(&state.clips.len()), "clips", "Expected between 1 and 4096 clips")?;
        require(state.zooms.len() <= 16384, "zooms", "At most 16384 zooms are supported")?;
        let _lock = self.lock(id, "document")?;
        let dir = paths::project_dir(&self.root, id)?;
        let mut document = read_document(&dir, id)?;
        check_revision(&document, expected_revision)?;
        let previous = document.clone();
        document.clips = state.clips.clone();
        document.zooms = state.zooms.clone();
        document.frame_style = state.frame_style.clone();
        document.cursor_settings = state.cursor_settings.clone();
        document.schema_version = 4;
        project::validate(&document)?;
        validate_cursor_change(&dir, &previous, &document)?;
        document.revision = next_revision(document.revision)?;
        write_document(&dir, &document)?;
        Ok(document)
    }
    /// Lexicographic direct-child IDs; issues consume page slots and advance the same cursor.
    pub fn list_projects(&self, cursor: Option<&str>, limit: u16) -> CoreResult<ProjectPage> {
        require((1..=100).contains(&limit), "limit", "Page limit must be between 1 and 100")?;
        if let Some(cursor) = cursor { paths::validate_id(cursor, "cursor")?; }
        let mut ids = Vec::new();
        for (index, entry) in fs::read_dir(&self.root).map_err(|error| io_error("list projects", error))?.enumerate() {
            require(index < 10_000, "projects", "Projects root exceeds the 10000 direct-entry scan limit")?;
            let entry = entry.map_err(|error| io_error("inspect project entry", error))?;
            if entry.file_type().is_ok_and(|kind| kind.is_file()) { continue; }
            let name = entry.file_name();
            let Some(id) = name.to_str().filter(|id| project::valid_id(id)) else { continue; };
            if cursor.is_none_or(|cursor| id > cursor) { ids.push(id.to_owned()); }
        }
        ids.sort_unstable();
        let has_more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let next_cursor = if has_more { ids.last().cloned() } else { None };
        let mut page = ProjectPage { projects: vec![], issues: vec![], next_cursor };
        for id in ids {
            match self.inspect_project(&id) {
                Ok(summary) => page.projects.push(summary),
                Err(mut error) => {
                    // Malformed JSON can echo a huge unknown field name; page responses stay bounded.
                    truncate_text(&mut error.message, 1024);
                    if let Some(field) = &mut error.field { truncate_text(field, 128); }
                    page.issues.push(ProjectIssue { entry: id, error });
                }
            }
        }
        Ok(page)
    }

    fn inspect_project(&self, id: &str) -> CoreResult<ProjectSummary> {
        // Reject aliases before locking or reading any candidate document/legacy file.
        let metadata = fs::symlink_metadata(self.root.join(id)).map_err(|error| io_error("inspect project directory", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() { return Err(invalid_path("Project entry must be a real directory")); }
        let _lock = self.lock(id, "document")?;
        let dir = paths::project_dir(&self.root, id)?;
        let document = match fs::symlink_metadata(dir.join("project.json")) {
            Ok(_) => Some(read_document(&dir, id)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(manifest) = crate::capture::read_authority(&dir, id)? {
                    let doc = manifest.finalized_project()?;
                    return Ok(ProjectSummary {project_id:id.into(),name:id.into(),revision:None,duration_us:Some(doc.video.duration_us),needs_import:true,capabilities:crate::cursor::capabilities(doc,&crate::cursor::CursorAssetReport::Unchecked)});
                }
                paths::asset_path(&dir, "screen.mp4")?;
                None
            }
            Err(error) => return Err(io_error("inspect project.json", error)),
        };
        Ok(ProjectSummary {
            project_id: id.into(), name: id.into(),
            revision: document.as_ref().map(|doc| doc.revision),
            duration_us: document.as_ref().map(|doc| doc.clips.iter().map(|clip| clip.out_us - clip.in_us).sum()),
            needs_import: document.is_none(), capabilities: document.as_ref().map(|doc| crate::cursor::capabilities(doc, &crate::cursor::CursorAssetReport::Unchecked)).unwrap_or_default(),
        })
    }

    pub fn new(root: &Path) -> CoreResult<Self> {
        let root = root.canonicalize().map_err(|error| io_error("resolve recordings directory", error))?;
        if !root.is_dir() { return Err(invalid_path("Recordings root must be a directory")); }
        let store = Self { root };
        store.lock_dir()?;
        Ok(store)
    }

    pub fn open_or_import(&self, id: &str, ffmpeg: &Path, defaults: LegacyDefaults) -> CoreResult<ProjectDocument> {
        let _lock = self.lock(id, "document")?;
        let dir = paths::project_dir(&self.root, id)?;
        // Even an invalid, future-schema or dangling project.json must win over legacy import.
        match fs::symlink_metadata(dir.join("project.json")) {
            Ok(_) => return read_document(&dir, id),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect project.json", error)),
        }
        if let Some(manifest) = crate::capture::read_authority(&dir, id)? {
            let document = crate::capture::import_authority(&dir, manifest, ffmpeg)?;
            write_document(&dir, &document)?;
            return Ok(document);
        }
        let video = paths::asset_path(&dir, "screen.mp4")?;
        let document = read_legacy(&dir, id, probe_video(ffmpeg, &video)?, defaults)?;
        write_document(&dir, &document)?;
        Ok(document)
    }

    pub fn get(&self, id: &str) -> CoreResult<ProjectDocument> {
        let _lock = self.lock(id, "document")?;
        read_document(&paths::project_dir(&self.root, id)?, id)
    }

    pub fn apply_edits(&self, id: &str, expected_revision: u64, operations: &[Edit]) -> CoreResult<ProjectDocument> {
        let _lock = self.lock(id, "document")?;
        let dir = paths::project_dir(&self.root, id)?;
        let document = read_document(&dir, id)?;
        check_revision(&document, expected_revision)?;
        let mut candidate = edits::apply(&document, operations)?;
        validate_cursor_change(&dir, &document, &candidate)?;
        candidate.revision = next_revision(document.revision)?;
        write_document(&dir, &candidate)?;
        Ok(candidate)
    }

    pub fn snapshot(&self, id: &str, expected_revision: u64) -> CoreResult<RenderSnapshot> {
        let _lock = self.lock(id, "document")?;
        let project_dir = paths::project_dir(&self.root, id)?;
        let mut document = read_document(&project_dir, id)?;
        check_revision(&document, expected_revision)?;
        paths::asset_path(&project_dir, &document.video.path)?;
        if let Some(audio) = &document.microphone {
            let usable = match paths::asset_path(&project_dir, &audio.path) {
                Ok(path) => is_usable_wav(&path),
                Err(error) if error.code == ErrorCode::NotFound => {
                    validate_missing_asset(&project_dir, &audio.path)?;
                    false
                }
                Err(error) => return Err(error),
            };
            if !usable {
                // Optional audio fallback affects this render copy only, never the saved revision.
                document.microphone = None;
                if !document.warnings.contains(&ProjectWarning::InvalidOptionalAudio) {
                    document.warnings.push(ProjectWarning::InvalidOptionalAudio);
                }
            }
        }
        Ok(RenderSnapshot { project_dir, document, prepared_cursor: None, prepared_focus: None })
    }

    /// Bounded validation for get/edit adapter responses; listings deliberately remain unchecked.
    pub fn capabilities(&self, document: &ProjectDocument) -> ProjectCapabilities {
        let report = match paths::project_dir(&self.root, &document.project_id) {
            Ok(dir) => crate::cursor::inspect_cursor(&dir, document),
            Err(error) => crate::cursor::CursorAssetReport::Unavailable(error),
        };
        crate::cursor::capabilities(document, &report)
    }

    pub fn delete_project(&self, id: &str) -> CoreResult<()> {
        let _render = self.lock_render(id)?;
        let _document = self.lock(id, "document")?;
        let dir = paths::project_dir(&self.root, id)?;
        fs::remove_dir_all(dir).map_err(|error| io_error("delete project", error))
    }

    pub(crate) fn refresh_export_summary(&self, id: &str) -> CoreResult<()> {
        let _document = self.lock(id, "document")?;
        let dir = paths::project_dir(&self.root, id)?;
        match fs::symlink_metadata(dir.join("session.json")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error("inspect session summary", error)),
            Ok(_) => {}
        }
        let path = paths::asset_path(&dir, "session.json")?;
        let mut bytes = Vec::new();
        File::open(path).and_then(|file| file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| io_error("read session summary", error))?;
        require(bytes.len() as u64 <= MAX_JSON_BYTES, "session", "Session summary exceeds 8 MiB")?;
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Summary {
            #[serde(alias = "sessionId")] session_id: String,
            #[serde(alias = "createdAt")] created_at: String,
            #[serde(alias = "durationSeconds")] duration_seconds: f64,
            #[serde(alias = "recordingSource")] recording_source: String,
            exported: bool,
        }
        let mut summary: Summary = serde_json::from_slice(&bytes).map_err(|error| invalid("session", format!("Invalid session summary: {error}")))?;
        require(summary.duration_seconds.is_finite() && summary.duration_seconds >= 0.0, "session", "Invalid session duration")?;
        summary.exported = true;
        let bytes = serde_json::to_vec(&summary).map_err(|error| invalid("session", error.to_string()))?;
        atomic_write(&dir, "session.json", &bytes)
    }

    /// Jobs keep this guard until rendering finishes, and acquire it before the document lock.
    pub(crate) fn lock_render(&self, id: &str) -> CoreResult<ProjectLock> {
        self.lock(id, "render")
    }

    fn lock_dir(&self) -> CoreResult<PathBuf> {
        let dir = self.root.join(".locks");
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io_error("create project lock directory", error)),
        }
        let metadata = fs::symlink_metadata(&dir).map_err(|error| io_error("inspect project lock directory", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir()
            || dir.canonicalize().map_err(|error| io_error("resolve project lock directory", error))? != dir {
            return Err(invalid_path("Project lock directory must be a real directory inside recordings"));
        }
        Ok(dir)
    }

    pub(crate) fn lock(&self, id: &str, kind: &str) -> CoreResult<ProjectLock> {
        paths::project_dir(&self.root, id)?;
        let path = self.lock_dir()?.join(format!("{id}.{kind}"));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(invalid_path("Project lock must be a regular file inside the lock directory"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect project lock", error)),
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(&path).map_err(|error| io_error("open project lock", error))?;
        if path.canonicalize().map_err(|error| io_error("resolve project lock", error))? != path {
            return Err(invalid_path("Project lock cannot resolve outside its own lock path"));
        }
        match file.try_lock() {
            Ok(()) => Ok(ProjectLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CoreError {
                code: ErrorCode::Busy, message: format!("Project {id} is busy; retry when its {kind} operation finishes"), field: Some("projectId".into()),
            }),
            Err(TryLockError::Error(error)) => Err(io_error("lock project", error)),
        }
    }
}

fn read_document(dir: &Path, id: &str) -> CoreResult<ProjectDocument> {
    let path = paths::asset_path(dir, "project.json")?;
    let file = File::open(path).map_err(|error| io_error("open project.json", error))?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON_BYTES + 1).read_to_end(&mut bytes).map_err(|error| io_error("read project.json", error))?;
    require(bytes.len() as u64 <= MAX_JSON_BYTES, "project", "Project JSON exceeds the 8 MiB limit")?;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SchemaVersion { schema_version: u64, frame_style: Option<serde::de::IgnoredAny> }
    let version: SchemaVersion = serde_json::from_slice(&bytes).map_err(|error| invalid("project", format!("Invalid project JSON: {error}")))?;
    if !matches!(version.schema_version, 1 | 2 | 3 | 4) {
        return Err(CoreError { code: ErrorCode::UnsupportedSchema, message: "Only project schema versions 1, 2, 3 and 4 are supported".into(), field: Some("schemaVersion".into()) });
    }
    require(version.schema_version == 1 || version.frame_style.is_some(), "frameStyle", "Schemas 2, 3 and 4 require an explicit frameStyle")?;
    let document: ProjectDocument = serde_json::from_slice(&bytes).map_err(|error| invalid("project", format!("Invalid project document: {error}")))?;
    project::validate(&document)?;
    require(document.project_id == id, "projectId", "Document project ID must equal its directory ID")?;
    Ok(document)
}

fn check_revision(document: &ProjectDocument, expected: u64) -> CoreResult<()> {
    if document.revision != expected {
        return Err(CoreError { code: ErrorCode::RevisionConflict, message: format!("Expected project revision {expected}, found {}; reload before retrying", document.revision), field: Some("expectedRevision".into()) });
    }
    Ok(())
}

// A missing optional leaf is allowed only when its nearest existing ancestor is still contained.
pub(crate) fn validate_missing_asset(dir: &Path, relative: &str) -> CoreResult<()> {
    let mut path = dir.join(relative);
    loop {
        match path.canonicalize() {
            Ok(canonical) if canonical.starts_with(dir) => return Ok(()),
            Ok(_) => return Err(invalid_path("Asset must remain inside its project directory")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                    return Err(invalid_path("Missing asset cannot be a dangling symlink"));
                }
                if !path.pop() { return Err(invalid_path("Could not contain missing asset")); }
            }
            Err(error) => return Err(io_error("resolve missing asset", error)),
        }
    }
}

fn write_document(dir: &Path, document: &ProjectDocument) -> CoreResult<()> {
    write_document_with(dir, document, |_| Ok(()), sync_directory)
}

// Fault callbacks stay private; callers can save only through import or validated Edit batches.
fn write_document_with(
    dir: &Path, document: &ProjectDocument,
    before_rename: impl FnOnce(&Path) -> std::io::Result<()>,
    sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
) -> CoreResult<()> {
    project::validate(document)?;
    let bytes = serde_json::to_vec(document).map_err(|error| invalid("project", format!("Could not serialize project: {error}")))?;
    require(bytes.len() as u64 <= MAX_JSON_BYTES, "project", "Project JSON exceeds the 8 MiB limit")?;
    replace_bytes(dir, "project.json", &bytes, before_rename)?;
    sync_parent(dir).map_err(|error| CoreError {
        code: ErrorCode::Io,
        message: format!("Project revision {} was committed, but directory synchronization failed; durability is unconfirmed. Reload the project before retrying: {error}", document.revision),
        field: Some("revision".into()),
    })
}

pub(crate) fn atomic_write(dir: &Path, name: &str, bytes: &[u8]) -> CoreResult<()> {
    replace_bytes(dir, name, bytes, |_| Ok(()))?;
    sync_directory(dir).map_err(|error| io_error("synchronize saved file directory", error))
}

fn replace_bytes(dir: &Path, name: &str, bytes: &[u8], before_rename: impl FnOnce(&Path) -> std::io::Result<()>) -> CoreResult<()> {
    replace_bytes_with(dir, name, bytes, |_| Ok(()), before_rename)
}

pub(crate) fn replace_bytes_with(dir: &Path, name: &str, bytes: &[u8], before_write: impl FnOnce(&Path) -> std::io::Result<()>, before_rename: impl FnOnce(&Path) -> std::io::Result<()>) -> CoreResult<()> {
    let (temp, mut file) = loop {
        let temp = dir.join(format!(".{name}-{}-{}.tmp", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("create temporary saved file", error)),
        }
    };
    let result = (|| {
        before_write(&temp).map_err(|error| io_error("prepare temporary saved file", error))?;
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|error| io_error("flush temporary saved file", error))?;
        drop(file);
        before_rename(&temp).map_err(|error| io_error("prepare saved file replacement", error))?;
        fs::rename(&temp, dir.join(name)).map_err(|error| io_error("replace saved file", error))
    })();
    let _ = fs::remove_file(temp);
    result
}

#[cfg(unix)]
pub(crate) fn sync_directory(dir: &Path) -> std::io::Result<()> { File::open(dir)?.sync_all() }
// Standard File cannot fsync directories on Windows; the prepared file is still synced.
#[cfg(not(unix))]
pub(crate) fn sync_directory(_dir: &Path) -> std::io::Result<()> { Ok(()) }

fn invalid_path(message: &str) -> CoreError {
    CoreError { code: ErrorCode::InvalidPath, message: message.into(), field: Some("path".into()) }
}
fn io_error(action: &str, error: std::io::Error) -> CoreError {
    CoreError {
        code: if error.kind() == std::io::ErrorKind::NotFound { ErrorCode::NotFound } else { ErrorCode::Io },
        message: format!("Could not {action}: {error}"), field: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::*;
    use std::{fs, sync::atomic::{AtomicU64, Ordering}};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("focusflow-store-unit-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
    fn document() -> ProjectDocument {
        ProjectDocument {
            frame_style: Default::default(), schema_version: 1, project_id: "fixture".into(), revision: 1,
            cursor: None, cursor_settings: Default::default(),
        video: VideoAsset { cursor: crate::project::CursorMode::Embedded, path: "screen.mp4".into(), width: 100, height: 50, duration_us: 5_000_000 },
            microphone: None, timing: Timing { provenance: TimingProvenance::LegacyUnknown, audio_offset_us: None },
            camera: CameraDefaults { pan_transition_us: 200_000, settle_us: 100_000 },
            clips: vec![Clip { id: "clip-0".into(), in_us: 0, out_us: 5_000_000 }],
            zooms: vec![], clicks: vec![], warnings: vec![],
        }
    }
    #[test]
    fn injected_error_before_rename_retains_previous_json() {
        let dir = Directory::new();
        fs::write(dir.0.join("project.json"), b"previous document bytes").unwrap();
        let error = write_document_with(&dir.0, &document(), |temp| {
            let prepared: ProjectDocument = serde_json::from_slice(&fs::read(temp)?).unwrap();
            assert_eq!(prepared.revision, 1);
            Err(std::io::Error::other("injected before rename"))
        }, |_| Ok(()));
        assert!(error.is_err());
        assert_eq!(fs::read(dir.0.join("project.json")).unwrap(), b"previous document bytes");
    }
    #[test]
    fn oversized_write_retains_previous_json() {
        let dir = Directory::new();
        fs::write(dir.0.join("project.json"), b"previous document bytes").unwrap();
        let mut doc = document();
        doc.warnings = vec![ProjectWarning::EmbeddedCursor; 600_000];
        let result = write_document_with(&dir.0, &doc, |_| Ok(()), |_| Ok(()));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::InvalidDocument);
        assert_eq!(fs::read(dir.0.join("project.json")).unwrap(), b"previous document bytes");
    }
    #[test]
    fn directory_sync_error_reports_committed_revision() {
        let dir = Directory::new();
        fs::write(dir.0.join("project.json"), b"previous document bytes").unwrap();
        let result = write_document_with(&dir.0, &document(), |_| Ok(()), |_| Err(std::io::Error::other("injected directory sync failure")));
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::Io);
        assert!(error.message.contains("revision 1 was committed"), "{error}");
        assert!(error.message.contains("durability is unconfirmed"), "{error}");
        let saved: ProjectDocument = serde_json::from_slice(&fs::read(dir.0.join("project.json")).unwrap()).unwrap();
        assert_eq!(saved.revision, 1);
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectCapabilities {
    pub fixed_zoom: bool, pub frame_style: bool,
    pub cursor_replacement: bool, pub cursor_smoothing: bool, pub cursor_follow: bool,
    pub cursor_native_style: bool,
    pub cursor_unavailable_reason: String,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSummary {
    pub project_id: String, pub name: String, pub revision: Option<u64>, pub duration_us: Option<u64>,
    pub needs_import: bool, pub capabilities: ProjectCapabilities,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectIssue { pub entry: String, pub error: CoreError }
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectPage { pub projects: Vec<ProjectSummary>, pub issues: Vec<ProjectIssue>, pub next_cursor: Option<String> }

impl Default for ProjectCapabilities {
    fn default() -> Self { Self { fixed_zoom: true, frame_style: true,
        cursor_replacement: false, cursor_smoothing: false, cursor_follow: false, cursor_native_style: false,
        cursor_unavailable_reason: "Recordings contain an embedded cursor and no separate continuous cursor telemetry".into() } }
}
fn next_revision(revision: u64) -> CoreResult<u64> {
    revision.checked_add(1).filter(|value| *value <= MAX_REVISION)
        .ok_or_else(|| invalid("revision", "Project revision limit reached"))
}

pub(crate) fn truncate_text(value: &mut String, max_bytes: usize) {
    if value.len() > max_bytes {
        let mut end = max_bytes - 3;
        while !value.is_char_boundary(end) { end -= 1; }
        value.truncate(end);
        value.push_str("...");
    }
}

fn validate_cursor_change(dir:&Path,previous:&ProjectDocument,document:&ProjectDocument)->CoreResult<()> {
    let old_zooms:std::collections::BTreeMap<_,_>=previous.zooms.iter().map(|z|(z.id.as_str(),z)).collect();
    let changed_follow=document.zooms.iter().filter(|z|z.mode==project::ZoomMode::Follow).any(|z|
        previous.cursor_settings.smoothing_us!=document.cursor_settings.smoothing_us ||
        old_zooms.get(z.id.as_str()).copied()!=Some(z));
    if changed_follow {
        let mut positions=document.clone();positions.cursor_settings.enabled=false;
        crate::cursor::prepare_cursor(dir,&positions,&std::sync::atomic::AtomicBool::new(false))?;
    }
    if document.cursor_settings.enabled && previous.cursor_settings != document.cursor_settings {
        crate::cursor::prepare_cursor(dir, document, &std::sync::atomic::AtomicBool::new(false))?;
    }
    Ok(())
}
