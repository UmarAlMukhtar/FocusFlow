//! Bounded current observations; admission never claims historical cursor alignment.
#[path = "cursor/image.rs"]
pub(in crate::recorder) mod image;
#[cfg(windows)]
#[path = "cursor/image/native.rs"]
pub(in crate::recorder) mod image_native;
#[path = "cursor/storage.rs"]
mod storage;
use focusflow_core::cursor_observation::{ModelProfile, WindowObservation};

fn candidate_family(w: &WindowObservation, raw: [u32; 2]) -> Option<ModelProfile> {
    if w.pid == 0
        || w.tid == 0
        || !w.observer_pmv2
        || !w.target_pmv2
        || w.dpi != 96
        || w.monitor_count != 1
        || w.monitor_rect[0] != 0
        || w.monitor_rect[1] != 0
        || !w.visible
        || w.minimized
        || raw.contains(&0)
        || i64::from(w.dwm_rect[2]) - i64::from(w.dwm_rect[0]) != i64::from(raw[0])
        || i64::from(w.dwm_rect[3]) - i64::from(w.dwm_rect[1]) != i64::from(raw[1])
    {
        return None;
    }
    ModelProfile::from_window_style(w.style, w.ex_style)
}
pub(super) fn release_qualified(_profile: ModelProfile) -> bool {
    false
}
#[derive(Default)]
struct ArmGate {
    started: bool,
}
impl ArmGate {
    fn arm(&mut self, ready: bool, admission: i64) -> Result<(), String> {
        if self.started || !ready || admission < 0 {
            return Err("Cursor readiness lost or duplicate admission".into());
        }
        self.started = true;
        Ok(())
    }
    fn sampling_allowed(&self) -> bool {
        self.started
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn window() -> WindowObservation {
        WindowObservation {
            hwnd: "42".into(),
            pid: 7,
            tid: 9,
            client_origin: [47, 63],
            client_size: [322, 249],
            window_rect: [40, 40, 376, 319],
            dwm_rect: [47, 40, 369, 312],
            dpi: 96,
            observer_pmv2: true,
            target_pmv2: true,
            style: 0x10cf0000,
            ex_style: 0,
            visible: true,
            minimized: false,
            monitor_rect: [0, 0, 1920, 1080],
            monitor_count: 1,
        }
    }
    #[test]
    fn family_accepts_only_fixed_model_and_exact_raw_extent() {
        let w = window();
        assert_eq!(
            candidate_family(&w, [322, 272]),
            Some(ModelProfile::Decorated)
        );
        let mut p = w.clone();
        p.style = 0x90000000;
        assert_eq!(candidate_family(&p, [322, 272]), Some(ModelProfile::Popup));
        for i in 0..10 {
            let mut bad = w.clone();
            match i {
                0 => bad.dpi = 144,
                1 => bad.target_pmv2 = false,
                2 => bad.observer_pmv2 = false,
                3 => bad.monitor_count = 2,
                4 => bad.monitor_rect[0] = -1920,
                5 => bad.ex_style = 1,
                6 => bad.style = 0,
                7 => bad.minimized = true,
                8 => bad.visible = false,
                _ => bad.pid = 0,
            };
            assert_eq!(candidate_family(&bad, [322, 272]), None);
        }
        assert_eq!(candidate_family(&w, [323, 272]), None);
        assert_eq!(candidate_family(&w, [322, 271]), None);
        for p in [ModelProfile::Popup, ModelProfile::Decorated] {
            assert!(
                !release_qualified(p),
                "Historical private proof must not enable production"
            );
        }
    }
    #[test]
    fn observed_decorated_window_edge_is_a_candidate_not_release_qualification() {
        let mut w = window();
        w.style = 0x14cf_0000; w.ex_style = 0x0000_0100; // Observed run42568819755 pair.
        let original = w.clone();
        assert_eq!(candidate_family(&w, [322, 272]), Some(ModelProfile::Decorated));
        assert_eq!(w, original);
        assert!(!release_qualified(ModelProfile::Decorated));
    }

    #[test]
    fn candidate_extended_style_is_exact_and_specific_to_decorated() {
        for (profile, style) in [(ModelProfile::Popup, 0x9000_0000), (ModelProfile::Popup, 0x9400_0000),
            (ModelProfile::Decorated, 0x10cf_0000), (ModelProfile::Decorated, 0x14cf_0000)] {
            let mut w = window(); w.style = style;
            for ex_style in [0, 0x100] {
                w.ex_style = ex_style;
                let accepted = ex_style == 0 || profile == ModelProfile::Decorated;
                assert_eq!(candidate_family(&w, [322, 272]), accepted.then_some(profile), "{style:#x}/{ex_style:#x}");
                assert!(!release_qualified(profile));
                if !accepted { continue; }
                for bit in 0..32 {
                    let flag = 1_u32 << bit;
                    if ex_style & flag == 0 && !(profile == ModelProfile::Decorated && flag == 0x100) {
                        w.ex_style = ex_style | flag;
                        assert_eq!(candidate_family(&w, [322, 272]), None, "extra extended bit {flag:#x} on {style:#x}/{ex_style:#x}");
                    }
                }
            }
        }
    }

    #[test]
    fn observed_popup_clipping_style_is_a_candidate_not_release_qualification() {
        let mut w = window();
        w.style = 0x9400_0000; // Literal observed popup style in run42565662101.
        w.client_origin = [40, 40];
        w.client_size = [320, 240];
        w.window_rect = [40, 40, 360, 280];
        w.dwm_rect = [40, 40, 360, 280];
        let original = w.clone();
        assert_eq!(candidate_family(&w, [320, 240]), Some(ModelProfile::Popup));
        assert_eq!(w, original);
        assert!(!release_qualified(ModelProfile::Popup));
    }

    #[test]
    fn candidate_style_rule_ignores_only_clipping_and_preserves_other_gates() {
        for (profile, style) in [(ModelProfile::Popup, 0x9000_0000), (ModelProfile::Popup, 0x9400_0000),
            (ModelProfile::Decorated, 0x10cf_0000), (ModelProfile::Decorated, 0x14cf_0000)] {
            let mut w = window(); w.style = style;
            assert_eq!(candidate_family(&w, [322, 272]), Some(profile));
            assert!(!release_qualified(profile));
            for bit in 0..32 {
                let flag = 1_u32 << bit;
                if style & flag == 0 && flag != 0x0400_0000 {
                    w.style = style | flag;
                    assert_eq!(candidate_family(&w, [322, 272]), None, "extra {flag:#x} on {style:#x}");
                }
            }
            w.style = style; w.ex_style = 1;
            assert_eq!(candidate_family(&w, [322, 272]), None);
            w.ex_style = 0;
            assert_eq!(candidate_family(&w, [321, 272]), None, "producer extent remains exact");
            assert_eq!(candidate_family(&w, [323, 272]), None);
        }
    }

    #[test]
    fn no_prearm_sampling_no_second_start_and_lost_readiness_fails() {
        let mut gate = ArmGate::default();
        assert!(!gate.sampling_allowed());
        assert!(gate.arm(false, 10).is_err());
        assert!(!gate.sampling_allowed());
        assert!(gate.arm(true, -1).is_err());
        assert!(gate.arm(true, 10).is_ok());
        assert!(gate.sampling_allowed());
        assert!(gate.arm(true, 11).is_err());
    }
}

#[cfg(windows)]
#[path = "cursor/native.rs"]
mod native;
use focusflow_core::{
    capture::{self, windows::*, windows_observations::*},
    cursor_observation::*,
    legacy::LegacyDefaults,
    project::*,
};
#[cfg(windows)]
pub(super) use native::CursorObserver;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};
pub(super) use storage::CursorObservationCompletion;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Admission {
    Embedded { notice: String },
    Separate { profile: ModelProfile },
}
impl Admission {
    pub fn embedded() -> Self {
        Self::Embedded {
            notice: String::new(),
        }
    }
    pub fn separate(&self) -> bool {
        matches!(self, Self::Separate { .. })
    }
    pub fn notice(&self) -> Option<String> {
        match self {
            Self::Embedded { notice } if !notice.is_empty() => Some(notice.clone()),
            _ => None,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Embedded { notice } if notice.len() > 512 => {
                Err("Cursor admission notice exceeds cap".into())
            }
            _ => Ok(()),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum Terminal {
    Embedded,
    Separate {
        completion: CursorObservationCompletion,
    },
}
impl Terminal {
    pub fn validate(&self, admission: &Admission, cutoff: i64) -> Result<(), String> {
        match (self, admission) {
            (Self::Embedded, Admission::Embedded { .. }) => Ok(()),
            (Self::Separate { completion }, Admission::Separate { profile }) => {
                completion.validate(*profile, cutoff)
            }
            _ => Err("Cursor terminal cannot change admitted video mode".into()),
        }
    }
}
pub(super) fn incomplete(
    base: &WindowsCaptureManifestV2,
    defaults: LegacyDefaults,
) -> WindowsCaptureManifestV3 {
    WindowsCaptureManifestV3 {
        schema_version: 3,
        capture_id: base.capture_id.clone(),
        source: base.source.clone(),
        initial_defaults: base.initial_defaults.clone(),
        interaction_defaults: defaults.into(),
        diagnostic: None,
        state: WindowsCaptureStateV3::Incomplete {
            video_path: capture::VideoPath::Screen,
            video_cursor: capture::SeparateCursor::Separate,
            cursor_index_path: ObservationIndexPath::Index,
        },
    }
}
fn fixed_asset(dir: &Path, name: &str) -> Result<std::path::PathBuf, String> {
    let mut path = dir.to_owned();
    for component in name.split('/') {
        path.push(component);
        if std::fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Cursor assets cannot use aliases".into());
        }
    }
    focusflow_core::paths::asset_path(dir, name).map_err(|e| e.to_string())
}
fn read_bounded(dir: &Path, name: &str, expected: u64, cap: u64) -> Result<Vec<u8>, String> {
    let path = fixed_asset(dir, name)?;
    if expected > cap {
        return Err("Cursor asset receipt exceeds cap".into());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() != expected {
        return Err("Cursor asset length does not match completion".into());
    }
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != expected {
        return Err("Cursor asset changed during read".into());
    }
    Ok(bytes)
}
/// Bind original raw evidence to the actual Summary clock and checked saved grid before import.
pub(super) fn publish(
    dir: &Path,
    mut manifest: WindowsCaptureManifestV3,
    clock: WindowsOwnedClockV1,
    grid: &focusflow_core::ffmpeg::SavedCfrGrid,
    completion: CursorObservationCompletion,
    has_mic: bool,
    source_notice: Option<String>,
) -> Result<Option<String>, String> {
    let cutoff = qpc(&clock.cutoff_qpc).map_err(|e| e.to_string())?;
    completion.validate(completion.profile, cutoff)?;
    let timing = WindowsVideoTimingV2 {
        endpoint: WindowsSavedEndpointV2 {
            provenance: WindowsEndpointProvenance::SavedPacketFullGrid,
            end_pts: grid.video.endpoint.ticks,
            time_base: capture::TimeBase {
                numerator: grid.video.endpoint.numerator,
                denominator: grid.video.endpoint.denominator,
            },
            interval: capture::Interval::HalfOpen,
        },
        clock: clock.clone(),
    };
    let mut degraded = completion.degraded || !completion.files_usable;
    let mut notice = source_notice;
    let data = if completion.files_usable {
        let resources = read_bounded(
            dir,
            "cursor/resources.json",
            completion.resources_bytes,
            focusflow_core::cursor::MAX_INDEX_BYTES,
        )?;
        if format!("{:x}", Sha256::digest(&resources)) != completion.resources_sha256 {
            return Err("Cursor resource spool SHA-256 mismatch".into());
        }
        let shapes = storage::decode_resources(&resources)?;
        if shapes.len() != completion.resources_count as usize {
            return Err("Cursor resource count mismatch".into());
        }
        // The two fixed file receipts bind counts and bytes. Strict canonical readers validate every row next.
        for (name, receipt) in [
            (OBSERVATIONS_PATH, &completion.pose),
            (INPUT_OBSERVATIONS_PATH, &completion.input),
        ] {
            let path = fixed_asset(dir, name)?;
            let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            if file.metadata().map_err(|e| e.to_string())?.len() != receipt.bytes {
                return Err("Observation byte receipt mismatch".into());
            }
            let mut hash = Sha256::new();
            let mut kept = 0u64;
            let mut chunk = [0; 8192];
            loop {
                let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                kept += n as u64;
                if kept > receipt.bytes {
                    return Err("Observation changed during hashing".into());
                }
                hash.update(&chunk[..n]);
            }
            if kept != receipt.bytes || format!("{:x}", hash.finalize()) != receipt.sha256 {
                return Err("Observation SHA-256 receipt mismatch".into());
            }
        }
        let index = CursorObservationIndexV2 {
            schema_version: 2,
            width: manifest.source.encoded_size.width,
            height: manifest.source.encoded_size.height,
            duration_us: timing.endpoint.duration_us().map_err(|e| e.to_string())?,
            observations_path: OBSERVATIONS_PATH.into(),
            input_observations_path: INPUT_OBSERVATIONS_PATH.into(),
            shapes,
            clock,
            model: GeometryModel::DwmExtendedFrameTranslationV1,
            profile: completion.profile,
            observation: ObservationPolicy::WindowsCurrentQueryBracket,
            reconstruction: ReconstructionPolicy::CausalAfterBracket,
            max_hold_us: 250_000,
            spatial_error_px: NullOnly,
            frame_alignment_error_us: NullOnly,
            animation_phase: AnimationPhase::Unknown,
            button_coverage: ButtonCoverage::SampledEdges,
            drag_coverage: DragCoverage::SampledPath,
            terminal: completion.terminal,
        };
        index.validate().map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(&index).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > focusflow_core::cursor::MAX_INDEX_BYTES {
            return Err("Final cursor index exceeds cap".into());
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("cursor/index.json"))
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        let data = read_observations(dir).map_err(|e| e.to_string())?;
        if data.poses.len() != completion.pose.records as usize
            || data.inputs.len() != completion.input.records as usize
        {
            return Err("Observation record receipt mismatch".into());
        }
        Some(data)
    } else {
        None
    };
    if degraded {
        notice=Some("Video saved. Sampled cursor tracking ended incompletely; cursor drawing starts disabled.".into());
    }
    manifest.diagnostic = notice.clone().map(|message| capture::CaptureDiagnosticV1 {
        stage: capture::DiagnosticStage::Cursor,
        message,
    });
    let (mut project, result) = manifest
        .build_initial_snapshot(
            &timing,
            data.as_ref(),
            has_mic.then_some(AudioAsset {
                path: "mic.wav".into(),
            }),
            CursorSettings::default(),
        )
        .map_err(|e| e.to_string())?;
    degraded |= project.video.cursor == CursorMode::Absent || result.truncated;
    if degraded || manifest.diagnostic.is_some() {
        let message=notice.get_or_insert_with(||"Video saved with unavailable or incomplete sampled cursor tracking; cursor drawing starts disabled.".into()).clone();
        manifest.diagnostic = Some(capture::CaptureDiagnosticV1 {
            stage: capture::DiagnosticStage::Cursor,
            message,
        });
        if !project
            .warnings
            .contains(&ProjectWarning::CursorCaptureIncomplete)
        {
            project
                .warnings
                .push(ProjectWarning::CursorCaptureIncomplete);
        }
    } else {
        project.cursor_settings.enabled = true;
    }
    manifest.state = WindowsCaptureStateV3::Finalized {
        video_timing: timing,
        cursor_timing: ObservationPolicy::WindowsCurrentQueryBracket,
        initial_project: project,
    };
    // These are derived compatibility exports only. No later V3 stage rereads them as authority.
    for (name, bytes) in [
        (
            "clicks.json",
            serde_json::to_vec_pretty(&result.compatibility.clicks),
        ),
        (
            "drags.json",
            serde_json::to_vec_pretty(&result.compatibility.drags),
        ),
        ("timeline.json", serde_json::to_vec_pretty(&result.timeline)),
    ] {
        if bytes
            .map_err(|e| e.to_string())
            .and_then(|b| {
                super::super::write_json_atomically(&dir.join(name), &b).map_err(|e| e.to_string())
            })
            .is_err()
        {
            notice=Some("Video and cursor observations saved; an optional compatibility file could not be written.".into());
        }
    }
    publish_windows_observations_finalized_assets(dir, &manifest, grid)
        .map_err(|e| e.to_string())?;
    Ok(notice)
}

#[derive(Debug, PartialEq, Eq)]
enum OfferFailure {
    Stopped,
    Overflow,
}
fn offer_sample<T>(
    tx: &std::sync::mpsc::SyncSender<T>,
    value: T,
    stopped: &std::sync::atomic::AtomicBool,
) -> Result<(), OfferFailure> {
    use std::sync::atomic::Ordering;
    if stopped.load(Ordering::SeqCst) {
        return Err(OfferFailure::Stopped);
    }
    tx.try_send(value).map_err(|_| {
        stopped.store(true, Ordering::SeqCst);
        OfferFailure::Overflow
    })
}
fn join_before<T>(
    owner: &mut Option<std::thread::JoinHandle<T>>,
    deadline: std::time::Instant,
) -> Result<T, String> {
    let handle = owner.as_ref().ok_or("Sampler owner missing")?;
    while !handle.is_finished() {
        if std::time::Instant::now() >= deadline {
            return Err("Sampler owner did not join before cleanup deadline".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    owner
        .take()
        .ok_or("Sampler owner missing")?
        .join()
        .map_err(|_| "Sampler panicked".into())
}
#[cfg(test)]
mod ownership_tests {
    use super::*;
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };
    #[test]
    fn queue_overflow_cannot_resume_after_the_consumer_catches_up() {
        let (tx, rx) = mpsc::sync_channel(64);
        let stopped = AtomicBool::new(false);
        for i in 0..64 {
            offer_sample(&tx, i, &stopped).unwrap();
        }
        assert!(offer_sample(&tx, 64, &stopped).is_err());
        for i in 0..64 {
            assert_eq!(rx.recv().unwrap(), i);
        }
        assert!(offer_sample(&tx, 65, &stopped).is_err());
        assert!(stopped.load(Ordering::SeqCst));
    }
    #[test]
    fn late_owner_cannot_claim_normal_completion() {
        let mut owner = Some(thread::spawn(|| {
            thread::sleep(Duration::from_millis(100));
            42
        }));
        assert!(join_before(&mut owner, Instant::now()).is_err());
        assert!(owner.is_some());
        assert_eq!(
            join_before(&mut owner, Instant::now() + Duration::from_secs(1)).unwrap(),
            42
        );
    }
    #[test]
    fn stopped_owner_is_actually_joined() {
        let mut owner = Some(thread::spawn(|| 42));
        assert_eq!(
            join_before(&mut owner, Instant::now() + Duration::from_secs(1)).unwrap(),
            42
        );
        assert!(owner.is_none());
        assert!(join_before(&mut owner, Instant::now()).is_err());
    }
}

#[cfg(test)]
#[path = "cursor/finalize_tests.rs"]
mod finalize_tests;
