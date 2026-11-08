//! Windows V3 authority binds sampled evidence to the existing owned video grid.
use super::windows::{WindowsInitialDefaults, WindowsSourceV2, WindowsVideoTimingV2};
use super::*;
use crate::{
    cursor_observation::{self as observation, *},
    legacy::{self, LegacyDefaults},
};
use std::{
    io::Write,
    time::{Duration, Instant},
};
pub const MARKER_NAME: &str = "capture.windows-v3";
const MARKER_BYTES: &[u8] = b"focusflow-windows-capture-v3\n";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenInteractionDefaultsV1 {
    pub scale: f64,
    pub intro_us: u64,
    pub hold_us: u64,
    pub outro_us: u64,
    pub pan_transition_us: u64,
    pub settle_us: u64,
}
impl From<LegacyDefaults> for FrozenInteractionDefaultsV1 {
    fn from(d: LegacyDefaults) -> Self {
        Self {
            scale: d.scale,
            intro_us: d.intro_us,
            hold_us: d.hold_us,
            outro_us: d.outro_us,
            pan_transition_us: d.pan_transition_us,
            settle_us: d.settle_us,
        }
    }
}
impl From<&FrozenInteractionDefaultsV1> for LegacyDefaults {
    fn from(d: &FrozenInteractionDefaultsV1) -> Self {
        Self {
            scale: d.scale,
            intro_us: d.intro_us,
            hold_us: d.hold_us,
            outro_us: d.outro_us,
            pan_transition_us: d.pan_transition_us,
            settle_us: d.settle_us,
        }
    }
}
literals!(ObservationIndexPath { Index => "cursor/index.json" });
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowsCaptureManifestV3 {
    pub schema_version: u32,
    pub capture_id: String,
    pub source: WindowsSourceV2,
    pub initial_defaults: WindowsInitialDefaults,
    pub interaction_defaults: FrozenInteractionDefaultsV1,
    #[serde(deserialize_with = "required")]
    pub diagnostic: Option<CaptureDiagnosticV1>,
    pub state: WindowsCaptureStateV3,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum WindowsCaptureStateV3 {
    #[serde(rename_all = "camelCase")]
    Incomplete {
        video_path: VideoPath,
        video_cursor: SeparateCursor,
        cursor_index_path: ObservationIndexPath,
    },
    #[serde(rename_all = "camelCase")]
    Finalized {
        video_timing: WindowsVideoTimingV2,
        cursor_timing: ObservationPolicy,
        #[serde(deserialize_with = "strict_project")]
        initial_project: ProjectDocument,
    },
}
impl WindowsCaptureManifestV3 {
    pub fn validate(&self, id: &str) -> CoreResult<()> {
        require(
            self.schema_version == 3,
            "schemaVersion",
            "Expected Windows capture schema 3",
        )?;
        require(
            project::valid_id(id) && self.capture_id == id,
            "captureId",
            "Capture ID must equal containing directory",
        )?;
        windows::validate_source(&self.source, &self.initial_defaults)?;
        legacy::validate_defaults((&self.interaction_defaults).into())?;
        require(
            self.interaction_defaults.pan_transition_us
                == self.initial_defaults.camera.pan_transition_us
                && self.interaction_defaults.settle_us == self.initial_defaults.camera.settle_us,
            "interactionDefaults",
            "Interaction camera defaults must equal frozen Start defaults",
        )?;
        if let Some(d) = &self.diagnostic {
            require(
                !d.message.is_empty() && d.message.len() <= 1024,
                "diagnostic",
                "Diagnostic requires 1..1024 UTF-8 bytes",
            )?;
        }
        if let WindowsCaptureStateV3::Finalized {
            video_timing,
            initial_project: doc,
            ..
        } = &self.state
        {
            video_timing.clock.validate(&video_timing.endpoint)?;
            project::validate(doc)?;
            require(
                doc.schema_version == 3 && doc.revision == 0 && doc.project_id == id,
                "initialProject",
                "Initial snapshot requires schema3/revision0/capture ID",
            )?;
            require(
                doc.video.path == "screen.mp4"
                    && (doc.video.width, doc.video.height)
                        == (
                            self.source.encoded_size.width,
                            self.source.encoded_size.height,
                        )
                    && doc.video.duration_us == video_timing.endpoint.duration_us()?,
                "initialProject.video",
                "Initial video must match frozen grid and owned endpoint",
            )?;
            require(
                doc.clips.len() == 1
                    && doc.clips[0].id == "clip-0"
                    && doc.clips[0].in_us == 0
                    && doc.clips[0].out_us == doc.video.duration_us,
                "initialProject.clips",
                "Expected one complete clip-0",
            )?;
            require(
                doc.frame_style == self.initial_defaults.frame_style
                    && doc.camera == self.initial_defaults.camera,
                "initialProject",
                "Frame and camera must match frozen Start defaults",
            )?;
            require(
                doc.microphone.as_ref().is_none_or(|m| m.path == "mic.wav")
                    && doc.timing.provenance == TimingProvenance::LegacyUnknown
                    && doc.timing.audio_offset_us.is_none(),
                "initialProject.timing",
                "Existing optional microphone and unmeasured timing rules apply",
            )?;
            require(
                matches!(doc.video.cursor, CursorMode::Separate | CursorMode::Absent),
                "initialProject.video.cursor",
                "Cursor-free admission cannot revert to embedded",
            )?;
            if doc.video.cursor == CursorMode::Separate {
                require(
                    doc.cursor
                        .as_ref()
                        .is_some_and(|c| c.index_path == "cursor/index.json"),
                    "initialProject.cursor",
                    "Separate source must reference observation index",
                )?;
            } else {
                require(
                    doc.cursor.is_none()
                        && !doc.cursor_settings.enabled
                        && self.diagnostic.is_some(),
                    "initialProject.cursor",
                    "Absent telemetry requires disabled settings and diagnostic",
                )?;
            }
            require(
                self.diagnostic.is_some()
                    == doc
                        .warnings
                        .contains(&ProjectWarning::CursorCaptureIncomplete)
                    && (!doc
                        .warnings
                        .contains(&ProjectWarning::CursorCaptureIncomplete)
                        || !doc.cursor_settings.enabled),
                "diagnostic",
                "Diagnostic and disabled cursorCaptureIncomplete must agree",
            )?;
            require(
                !doc.warnings.contains(&ProjectWarning::EmbeddedCursor)
                    && doc.warnings.contains(&ProjectWarning::LegacyTimingUnknown)
                    && doc
                        .warnings
                        .iter()
                        .enumerate()
                        .all(|(i, w)| !doc.warnings[..i].contains(w))
                    && doc.warnings.iter().all(|w| {
                        matches!(
                            w,
                            ProjectWarning::LegacyTimingUnknown
                                | ProjectWarning::InvalidOptionalAudio
                                | ProjectWarning::CursorCaptureIncomplete
                                | ProjectWarning::LegacyScaleOverridden
                                | ProjectWarning::LegacyInteractionClipped
                                | ProjectWarning::LegacyZoomOverlapNormalized
                        )
                    }),
                "initialProject.warnings",
                "Unexpected observation snapshot warnings",
            )?;
        }
        Ok(())
    }
    pub fn finalized_project(&self) -> CoreResult<&ProjectDocument> {
        match &self.state {
            WindowsCaptureStateV3::Finalized {
                initial_project, ..
            } => Ok(initial_project),
            _ => Err(recovery("Observation capture is incomplete")),
        }
    }
}
fn recovery(s: &str) -> CoreError {
    invalid("capture.json", format!("{s}; retain files for recovery"))
}
pub fn decode_windows_observations(bytes: &[u8], id: &str) -> CoreResult<WindowsCaptureManifestV3> {
    require(
        bytes.len() as u64 <= MAX_JSON_BYTES,
        "capture.json",
        "Capture exceeds 8 MiB",
    )?;
    let m: WindowsCaptureManifestV3 = observation::decode_strict(bytes)?;
    m.validate(id)?;
    Ok(m)
}
pub(super) fn read_marker(dir: &Path) -> CoreResult<bool> {
    let Some(path) = asset(dir, MARKER_NAME)? else {
        return Ok(false);
    };
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(30).read_to_end(&mut bytes))
        .map_err(io_error)?;
    require(
        bytes == MARKER_BYTES,
        MARKER_NAME,
        "Invalid or incomplete Windows V3 intent marker",
    )?;
    Ok(true)
}
/// Index-only projection is provisional; the immutable raw grid/clock/owner join is mandatory.
fn bind_source(
    source: &WindowsSourceV2,
    timing: &WindowsVideoTimingV2,
    data: &ObservationData,
) -> CoreResult<()> {
    data.index.validate()?;
    timing.clock.validate(&timing.endpoint)?;
    let index = &data.index;
    require(
        (index.width, index.height, index.duration_us)
            == (
                source.encoded_size.width,
                source.encoded_size.height,
                timing.endpoint.duration_us()?,
            )
            && index.clock == timing.clock,
        "cursor.index",
        "Observation grid, duration and clock must exactly match V3 authority",
    )?;
    for (row, segment) in data
        .poses
        .iter()
        .zip(eligible_pose_segments(&data.poses, index)?)
    {
        if segment.is_some() {
            if let WindowsCursorRowV2::Observed { before, after, .. } = row {
                for w in [before, after] {
                    require(
                        w.hwnd == source.window_id,
                        "observations.owner",
                        "Pose identity must equal captured target HWND",
                    )?;
                    require(
                        i64::from(w.dwm_rect[2]) - i64::from(w.dwm_rect[0])
                            == i64::from(source.raw_size.width)
                            && i64::from(w.dwm_rect[3]) - i64::from(w.dwm_rect[1])
                                == i64::from(source.raw_size.height),
                        "observations.rawSize",
                        "DWM extent must match exact frozen raw grid, including odd padding",
                    )?;
                }
            }
        }
    }
    Ok(())
}
pub fn bind_observations(
    manifest: &WindowsCaptureManifestV3,
    data: &ObservationData,
) -> CoreResult<()> {
    manifest.validate(&manifest.capture_id)?;
    let WindowsCaptureStateV3::Finalized { video_timing, .. } = &manifest.state else {
        return Err(recovery("Cannot bind incomplete capture"));
    };
    bind_source(&manifest.source, video_timing, data)
}
impl WindowsCaptureManifestV3 {
    /// One Start-defaults snapshot policy for the producer and strict importer; no compatibility reread.
    pub fn build_initial_snapshot(
        &self,
        timing: &WindowsVideoTimingV2,
        data: Option<&ObservationData>,
        microphone: Option<project::AudioAsset>,
        cursor_settings: project::CursorSettings,
    ) -> CoreResult<(ProjectDocument, SampledInteractions)> {
        self.validate(&self.capture_id)?;
        timing.clock.validate(&timing.endpoint)?;
        let mut usable = false;
        let result = if let Some(data) = data {
            bind_source(&self.source, timing, data)?;
            require(
                data.index.terminal.stop_kind != StopKind::SamplerFailure
                    || (self.diagnostic.is_some() && !cursor_settings.enabled),
                "cursor",
                "Sampler failure requires declared degradation and disabled initial drawing",
            )?;
            usable = eligible_pose_segments(&data.poses, &data.index)?
                .iter()
                .any(Option::is_some);
            sampled_interactions(
                &data.poses,
                &data.inputs,
                &data.index.terminal,
                &data.index,
                (&self.interaction_defaults).into(),
            )?
        } else {
            require(
                self.diagnostic.is_some(),
                "cursor",
                "Missing canonical observations require declared degradation",
            )?;
            SampledInteractions {
                click_evidence: vec![],
                drag_evidence: vec![],
                compatibility: crate::interaction_timeline::InteractionEvents {
                    clicks: vec![],
                    drags: vec![],
                },
                timeline: vec![],
                project: legacy::LegacyInteractions {
                    zooms: vec![],
                    clicks: vec![],
                    warnings: vec![],
                },
                truncated: false,
            }
        };
        require(
            usable
                || (!cursor_settings.enabled
                    && result.project.clicks.is_empty()
                    && result.project.zooms.is_empty()),
            "cursor",
            "Absent telemetry requires disabled settings and empty derived interactions",
        )?;
        let mut warnings = vec![ProjectWarning::LegacyTimingUnknown];
        warnings.extend(result.project.warnings.clone());
        if self.diagnostic.is_some() {
            warnings.push(ProjectWarning::CursorCaptureIncomplete);
        }
        let duration = timing.endpoint.duration_us()?;
        let doc = ProjectDocument {
            schema_version: 3,
            project_id: self.capture_id.clone(),
            revision: 0,
            video: project::VideoAsset {
                path: "screen.mp4".into(),
                width: self.source.encoded_size.width,
                height: self.source.encoded_size.height,
                duration_us: duration,
                cursor: if usable {
                    CursorMode::Separate
                } else {
                    CursorMode::Absent
                },
            },
            cursor: usable.then(|| project::CursorAsset {
                index_path: "cursor/index.json".into(),
            }),
            cursor_settings,
            audio_tracks: None,
            microphone,
            timing: project::Timing {
                provenance: TimingProvenance::LegacyUnknown,
                audio_offset_us: None,
            },
            frame_style: self.initial_defaults.frame_style.clone(),
            camera: self.initial_defaults.camera.clone(),
            clips: vec![project::Clip {
                id: "clip-0".into(),
                in_us: 0,
                out_us: duration,
            }],
            zooms: result.project.zooms.clone(),
            clicks: result.project.clicks.clone(),
            warnings,
        };
        project::validate(&doc)?;
        Ok((doc, result))
    }
}
/// Read-only checked conversion; compatibility JSON never supplies V3 authority.
pub fn checked_interactions(
    manifest: &WindowsCaptureManifestV3,
    data: &ObservationData,
) -> CoreResult<SampledInteractions> {
    bind_observations(manifest, data)?;
    let WindowsCaptureStateV3::Finalized {
        video_timing,
        initial_project: doc,
        ..
    } = &manifest.state
    else {
        return Err(recovery("Incomplete snapshot"));
    };
    let (mut expected, result) = manifest.build_initial_snapshot(
        video_timing,
        Some(data),
        doc.microphone.clone(),
        doc.cursor_settings.clone(),
    )?;
    if doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio) {
        expected.warnings.push(ProjectWarning::InvalidOptionalAudio);
    }
    require(*doc==expected,"initialProject","Initial snapshot must equal the shared canonical interactions and frozen-defaults conversion")?;
    Ok(result)
}
fn inventory(
    dir: &Path,
    manifest: &WindowsCaptureManifestV3,
) -> CoreResult<(Vec<PathBuf>, bool, bool, bool)> {
    let doc = manifest.finalized_project()?;
    if asset(dir, "cursor/index.json")?.is_none() {
        require(
            doc.video.cursor == CursorMode::Absent
                && doc.cursor.is_none()
                && !doc.cursor_settings.enabled
                && manifest.diagnostic.is_some()
                && doc
                    .warnings
                    .contains(&ProjectWarning::CursorCaptureIncomplete)
                && doc.clicks.is_empty()
                && doc.zooms.is_empty(),
            "cursor",
            "Missing index is allowed only for declared absent telemetry with empty interactions",
        )?;
        let WindowsCaptureStateV3::Finalized { video_timing, .. } = &manifest.state else {
            return Err(recovery("Incomplete absent capture"));
        };
        let (mut expected, _) = manifest.build_initial_snapshot(
            video_timing,
            None,
            doc.microphone.clone(),
            doc.cursor_settings.clone(),
        )?;
        if doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio) {
            expected.warnings.push(ProjectWarning::InvalidOptionalAudio);
        }
        require(
            *doc == expected,
            "initialProject",
            "Absent snapshot must equal frozen defaults with empty interactions",
        )?;
        return Ok((vec![], false, false, false));
    }
    let data = read_observations(dir)?;
    let result = checked_interactions(manifest, &data)?;
    let mut assets = vec![
        dir.join("cursor/index.json"),
        dir.join(OBSERVATIONS_PATH),
        dir.join(INPUT_OBSERVATIONS_PATH),
    ];
    let mut usable = false;
    let mut native = true;
    for (row, segment) in data
        .poses
        .iter()
        .zip(eligible_pose_segments(&data.poses, &data.index)?)
    {
        if segment.is_some() {
            usable = true;
            if let WindowsCursorRowV2::Observed { shape_id: None, .. } = row {
                native = false;
            }
        }
    }
    for shape in &data.index.shapes {
        let image = asset(dir, &shape.path)?;
        let valid = match image.as_ref() {
            Some(path) => cursor::read_stable(
                path,
                cursor::MAX_HEADER_BYTES as u64
                    + u64::from(shape.width) * u64::from(shape.height) * 4,
                |f| cursor::read_pam(f, shape),
            )
            .is_ok(),
            None => false,
        };
        native &= valid;
        if let Some(p) = image {
            assets.push(p);
        }
    }
    let doc = manifest.finalized_project()?;
    if doc.video.cursor == CursorMode::Separate {
        require(
            usable,
            "cursor",
            "Separate initial capture requires an eligible projectable position",
        )?;
    } else {
        require(
            !usable,
            "cursor",
            "Absent capture cannot discard usable canonical positions",
        )?;
    }
    if !usable || !native || result.truncated || manifest.diagnostic.is_some() {
        require(!doc.cursor_settings.enabled&&manifest.diagnostic.is_some()&&doc.warnings.contains(&ProjectWarning::CursorCaptureIncomplete),"cursor","Degraded telemetry requires disabled initial drawing, diagnostic and cursorCaptureIncomplete")?;
    }
    Ok((assets, usable, native, result.truncated))
}
/// Structural asset check using already-probed complete CFR packet evidence; no subprocess or writes.
pub fn validate_windows_observation_assets(
    dir: &Path,
    manifest: &WindowsCaptureManifestV3,
    grid: &ffmpeg::SavedCfrGrid,
) -> CoreResult<ProjectDocument> {
    manifest.validate(session_id(dir)?)?;
    let WindowsCaptureStateV3::Finalized {
        video_timing,
        initial_project: doc,
        ..
    } = &manifest.state
    else {
        return Err(recovery("Observation capture is incomplete"));
    };
    asset(dir, "screen.mp4")?.ok_or_else(|| recovery("Missing screen.mp4"))?;
    require(
        grid.completed_frames == video_timing.clock.completed_frames
            && equal_time(
                grid.video.endpoint,
                ffmpeg::PacketTime {
                    ticks: video_timing.endpoint.end_pts,
                    numerator: video_timing.endpoint.time_base.numerator,
                    denominator: video_timing.endpoint.time_base.denominator,
                },
            )?
            && equal_time(
                grid.video.first_packet_duration,
                ffmpeg::PacketTime {
                    ticks: 1,
                    numerator: 1,
                    denominator: 30,
                },
            )?
            && (
                grid.video.video.width,
                grid.video.video.height,
                grid.video.video.duration_us,
            ) == (doc.video.width, doc.video.height, doc.video.duration_us),
        "video",
        "Saved complete packet grid does not match V3 authority",
    )?;
    inventory(dir, manifest)?;
    let mut doc = doc.clone();
    project_optional_audio(dir, &mut doc)?;
    Ok(doc)
}
pub(super) fn import_windows_observations(
    dir: &Path,
    manifest: WindowsCaptureManifestV3,
    ffmpeg: &Path,
) -> CoreResult<ProjectDocument> {
    let WindowsCaptureStateV3::Finalized { video_timing, .. } = &manifest.state else {
        return Err(recovery("Observation capture incomplete"));
    };
    let path = asset(dir, "screen.mp4")?.ok_or_else(|| recovery("Missing screen.mp4"))?;
    let grid = ffmpeg::probe_cfr_grid(
        ffmpeg,
        &path,
        video_timing.clock.completed_frames,
        &AtomicBool::new(false),
        Instant::now() + Duration::from_secs(600),
    )?;
    validate_windows_observation_assets(dir, &manifest, &grid)
}
fn session_id(dir: &Path) -> CoreResult<&str> {
    require(
        dir.canonicalize().map_err(io_error)? == dir
            && fs::symlink_metadata(dir).map_err(io_error)?.is_dir(),
        "capture.path",
        "Capture requires real canonical directory",
    )?;
    dir.file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| path_error("Capture ID must be UTF-8"))
}
pub fn publish_windows_observations_incomplete(
    dir: &Path,
    manifest: &WindowsCaptureManifestV3,
) -> CoreResult<()> {
    require(
        matches!(manifest.state, WindowsCaptureStateV3::Incomplete { .. }),
        "state",
        "Expected incomplete intent",
    )?;
    publish(dir, manifest, None).map(|_| ())
}
pub fn publish_windows_observations_finalized_assets(
    dir: &Path,
    manifest: &WindowsCaptureManifestV3,
    grid: &ffmpeg::SavedCfrGrid,
) -> CoreResult<WindowsCaptureManifestV3> {
    require(
        matches!(manifest.state, WindowsCaptureStateV3::Finalized { .. }),
        "state",
        "Expected finalized state",
    )?;
    publish(dir, manifest, Some(grid))
}
fn sync(path: &Path, directory: bool) -> CoreResult<()> {
    if directory {
        store::sync_directory(path).map_err(io_error)
    } else {
        fs::OpenOptions::new()
            .read(true)
            .write(cfg!(windows))
            .open(path)
            .and_then(|f| f.sync_all())
            .map_err(io_error)
    }
}
fn publish(
    dir: &Path,
    manifest: &WindowsCaptureManifestV3,
    grid: Option<&ffmpeg::SavedCfrGrid>,
) -> CoreResult<WindowsCaptureManifestV3> {
    let id = session_id(dir)?;
    manifest.validate(id)?;
    let parent = dir
        .parent()
        .ok_or_else(|| path_error("Capture requires parent"))?;
    let store = store::ProjectStore::new(parent)?;
    let _lock = store.lock(id, "document")?;
    let marker = read_marker(dir)?;
    require(
        !windows::read_marker(dir)?,
        "capture.json",
        "Conflicting Windows capture intent",
    )?;
    let previous = if asset(dir, "capture.json")?.is_some() {
        read_authority(dir, id)?
    } else {
        None
    };
    match previous {
        Some(CaptureAuthority::WindowsV3(previous)) => {
            require(
                matches!(previous.state, WindowsCaptureStateV3::Incomplete { .. }),
                "capture.json",
                "Finalized V3 authority is immutable",
            )?;
            require(
                previous.capture_id == manifest.capture_id
                    && previous.source == manifest.source
                    && previous.initial_defaults == manifest.initial_defaults
                    && previous.interaction_defaults == manifest.interaction_defaults,
                "capture.json",
                "Capture source and frozen Start defaults are immutable",
            )?;
        }
        Some(_) => return Err(recovery("Cannot replace other capture authority")),
        None => require(
            matches!(manifest.state, WindowsCaptureStateV3::Incomplete { .. }),
            "capture.json",
            "Finalization requires prior incomplete V3 authority",
        )?,
    }
    let mut committed = manifest.clone();
    let mut assets = Vec::new();
    if let Some(grid) = grid {
        let doc = validate_windows_observation_assets(dir, manifest, grid)?;
        assets = inventory(dir, manifest)?.0;
        assets.push(dir.join("screen.mp4"));
        if doc.microphone.is_some() {
            assets.push(dir.join("mic.wav"));
        }
        if let WindowsCaptureStateV3::Finalized {
            initial_project, ..
        } = &mut committed.state
        {
            *initial_project = doc;
        }
    }
    let bytes =
        serde_json::to_vec(&committed).map_err(|e| invalid("capture.json", e.to_string()))?;
    require(
        bytes.len() as u64 <= MAX_JSON_BYTES,
        "capture.json",
        "Capture exceeds 8 MiB",
    )?;
    if !marker {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(MARKER_NAME))
            .map_err(io_error)?;
        file.write_all(MARKER_BYTES)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(io_error)?;
    } else {
        sync(&dir.join(MARKER_NAME), false)?;
    }
    sync(dir, true)?;
    for path in assets {
        sync(&path, false)?;
    }
    if dir.join("cursor/shapes").exists() {
        sync(&dir.join("cursor/shapes"), true)?;
    }
    if dir.join("cursor").exists() {
        sync(&dir.join("cursor"), true)?;
    }
    sync(dir, true)?;
    store::replace_bytes_with(dir, "capture.json", &bytes, |_| Ok(()), |_| Ok(()))?;
    let durability = (|| {
        sync(dir, true)?;
        if matches!(committed.state, WindowsCaptureStateV3::Incomplete { .. }) {
            sync(parent, true)?;
        }
        Ok::<(), CoreError>(())
    })();
    durability.map_err(|e|invalid("capture.json",format!("V3 authority committed; directory durability unconfirmed. Reload before retrying: {}",e.message)))?;
    Ok(committed)
}
/// Checked index2 asset capability requires the immutable V3 source join, even after project edits.
pub(crate) fn inspect_checked(dir: &Path, doc: &ProjectDocument) -> CoreResult<(bool, bool, bool)> {
    let Some(CaptureAuthority::WindowsV3(m)) = read_authority(dir, &doc.project_id)? else {
        return Err(recovery("Observation index requires Windows V3 authority"));
    };
    let initial = m.finalized_project()?;
    require(
        doc.video == initial.video && doc.cursor == initial.cursor,
        "cursor",
        "Edited project cannot replace immutable V3 source",
    )?;
    let (_, usable, native, truncated) = inventory(dir, &m)?;
    Ok((usable, native, truncated))
}
