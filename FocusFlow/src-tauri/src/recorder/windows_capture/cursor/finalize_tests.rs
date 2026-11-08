//! Device-free producer receipts and publication tests. The grid is a literal synthetic fixture.
use super::super::{Completion, Summary};
use super::*;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ff-production-cursor-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn setup(
    usable: bool,
) -> (
    Fixture,
    WindowsCaptureManifestV2,
    LegacyDefaults,
    CursorObservationCompletion,
    focusflow_core::ffmpeg::SavedCfrGrid,
) {
    let f = Fixture::new();
    let defaults = crate::export::ExportSettings::default()
        .legacy_defaults()
        .unwrap();
    let id = f.0.file_name().unwrap().to_str().unwrap();
    let base = super::super::incomplete(
        id,
        42,
        322,
        272,
        capture::Bounds {
            x: 40,
            y: 40,
            width: 336,
            height: 279,
        },
        defaults,
    )
    .unwrap();
    publish_windows_observations_incomplete(&f.0, &incomplete(&base, defaults)).unwrap();
    fs::write(f.0.join("screen.mp4"), b"retained synthetic media sentinel").unwrap();
    let mut streams = storage::Streams::create(&f.0).unwrap();
    let id = streams
        .shape(image::Shape {
            width: 1,
            height: 1,
            hotspot: [0, 0],
            rgba: vec![0, 0, 0, 255],
        })
        .unwrap();
    let w = super::tests::window();
    let pointer = PointerObservation {
        physical_position: if usable { [56, 79] } else { [-100, -100] },
        cursor_position: if usable { [56, 79] } else { [-100, -100] },
        flags: 1,
        handle: "55".into(),
        hit_hwnd: "42".into(),
        root_hwnd: "42".into(),
        hit_pid: 7,
        hit_tid: 9,
    };
    streams
        .pose(&WindowsCursorRowV2::Observed {
            q0: "3000000".into(),
            q1: "3000001".into(),
            before: w.clone(),
            after: w,
            pointer_before: pointer.clone(),
            pointer_after: pointer,
            shape_id: Some(id),
            barrier_revision: 0,
        })
        .unwrap();
    for (q, left) in [(3000002, false), (3000020, true), (3000040, false)] {
        streams
            .input(&WindowsInputRowV1::Observed {
                q0: q.to_string(),
                q1: (q + 1).to_string(),
                physical_position: [56, 79],
                hit_hwnd: "42".into(),
                root_hwnd: "42".into(),
                hit_pid: 7,
                hit_tid: 9,
                buttons_swapped: false,
                left: Some(left),
                right: Some(false),
                basis_pose_row: Some(0),
                barrier_revision_before: 0,
                barrier_revision_after: 0,
            })
            .unwrap();
    }
    let mut completion = streams
        .finish(
            ModelProfile::Decorated,
            ObservationTerminal {
                pose_barrier: None,
                input_barrier: None,
                source_cutoff_qpc: 6000000,
                stop_kind: StopKind::Stop,
            },
            storage::Cadence {
                samples: 1,
                first_qpc: Some(3000000),
                last_qpc: Some(3000001),
                ..Default::default()
            },
            storage::Cadence {
                samples: 3,
                first_qpc: Some(3000002),
                last_qpc: Some(3000041),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    completion.pose_joined = true;
    completion.input_joined = true;
    let grid = focusflow_core::ffmpeg::SavedCfrGrid {
        completed_frames: 30,
        video: focusflow_core::ffmpeg::ProbedVideoEvidence {
            video: focusflow_core::ffmpeg::ProbedVideo {
                width: 322,
                height: 272,
                duration_us: 1_000_000,
            },
            endpoint: focusflow_core::ffmpeg::PacketTime {
                ticks: 30000,
                numerator: 1,
                denominator: 30000,
            },
            first_packet_duration: focusflow_core::ffmpeg::PacketTime {
                ticks: 1000,
                numerator: 1,
                denominator: 30000,
            },
        },
    };
    (f, base, defaults, completion, grid)
}
fn summary() -> Summary {
    Summary {
        frequency: 3_000_000,
        admission: 2_997_000,
        epoch: 10_000_000,
        cutoff: 6_000_000,
        closed: false,
        selected: 30,
        written: 30,
        encoder_exit: 0,
        geometry_changed: false,
        source_availability: None,
    }
}
fn completion(
    base: WindowsCaptureManifestV2,
    defaults: LegacyDefaults,
    c: CursorObservationCompletion,
) -> super::super::RecorderResult<Completion> {
    Completion::from_worker(
        base,
        defaults,
        &Admission::Separate {
            profile: ModelProfile::Decorated,
        },
        Terminal::Separate { completion: c },
        summary(),
        0,
    )
}
fn manifest(f: &Fixture) -> WindowsCaptureManifestV3 {
    decode_windows_observations(
        &fs::read(f.0.join("capture.json")).unwrap(),
        f.0.file_name().unwrap().to_str().unwrap(),
    )
    .unwrap()
}
#[test]
fn real_finalizer_joins_clock_grid_and_canonical_interactions_without_legacy_reread() {
    let (f, base, defaults, c, grid) = setup(true);
    for name in ["clicks.json", "drags.json", "timeline.json"] {
        fs::write(
            f.0.join(name),
            b"malformed legacy data must not supply V3 authority",
        )
        .unwrap();
    }
    completion(base, defaults, c)
        .unwrap()
        .publish(&f.0, &grid, false)
        .unwrap();
    let m = manifest(&f);
    let doc = m.finalized_project().unwrap();
    assert_eq!(doc.video.cursor, CursorMode::Separate);
    assert!(doc.cursor_settings.enabled);
    assert_eq!(doc.clicks.len(), 1);
    assert!(!doc.zooms.is_empty());
    assert_eq!(doc.clicks[0].at_us, 7);
    assert_eq!(doc.clicks[0].x, 9.0 / 322.0);
    assert_eq!(doc.clicks[0].y, 39.0 / 272.0);
    assert_eq!(
        fs::read(f.0.join("screen.mp4")).unwrap(),
        b"retained synthetic media sentinel"
    );
    assert!(!f.0.join("capture.windows-v2").exists());
    assert_eq!(read_observations(&f.0).unwrap().inputs.len(), 3);
}
#[test]
fn changed_or_malformed_resource_spool_never_creates_canonical_index() {
    for change in 0..3 {
        let (f, base, defaults, mut c, grid) = setup(true);
        let p = f.0.join("cursor/resources.json");
        let mut bytes = fs::read(&p).unwrap();
        match change {
            0 => {
                let at = bytes.iter().position(|b| *b == b'1').unwrap();
                bytes[at] = b'2';
            }
            1 => {
                bytes = vec![b'x'; 65537];
                c.resources_bytes = bytes.len() as u64;
            }
            _ => {
                bytes = br#"{"schemaVersion":1,"schemaVersion":1,"shapes":[]}"#.to_vec();
                c.resources_bytes = bytes.len() as u64;
                c.resources_sha256 = format!("{:x}", Sha256::digest(&bytes));
            }
        }
        fs::write(p, bytes).unwrap();
        let result = completion(base, defaults, c).and_then(|c| c.publish(&f.0, &grid, false));
        assert!(result.is_err());
        assert!(!f.0.join("cursor/index.json").exists());
        assert!(matches!(
            manifest(&f).state,
            WindowsCaptureStateV3::Incomplete { .. }
        ));
    }
}
#[test]
fn missing_telemetry_saves_absent_video_and_unusable_positions_get_explicit_diagnostic() {
    for missing in [false, true] {
        let (f, base, defaults, mut c, grid) = setup(false);
        if missing {
            c.files_usable = false;
            c.degraded = true;
            c.terminal.stop_kind = StopKind::SamplerFailure;
        }
        completion(base, defaults, c)
            .unwrap()
            .publish(&f.0, &grid, false)
            .unwrap();
        let m = manifest(&f);
        let d = m.finalized_project().unwrap();
        assert_eq!(d.video.cursor, CursorMode::Absent);
        assert!(!d.cursor_settings.enabled);
        assert!(d.clicks.is_empty());
        assert!(m.diagnostic.is_some());
        assert!(d
            .warnings
            .contains(&ProjectWarning::CursorCaptureIncomplete));
        assert_eq!(f.0.join("cursor/index.json").exists(), !missing);
    }
}
#[test]
fn stopped_joined_receipts_cannot_change_mode_and_failed_grid_stays_incomplete() {
    let (f, base, defaults, mut c, mut grid) = setup(true);
    c.input_joined = false;
    assert!(completion(base.clone(), defaults, c.clone()).is_err());
    c.input_joined = true;
    assert!(Completion::from_worker(
        base.clone(),
        defaults,
        &Admission::embedded(),
        Terminal::Separate {
            completion: c.clone()
        },
        summary(),
        0
    )
    .is_err());
    grid.completed_frames = 29;
    assert!(completion(base, defaults, c)
        .unwrap()
        .publish(&f.0, &grid, false)
        .is_err());
    assert!(matches!(
        manifest(&f).state,
        WindowsCaptureStateV3::Incomplete { .. }
    ));
}
#[test]
fn optional_compatibility_write_failure_retains_canonical_snapshot() {
    let (f, base, defaults, c, grid) = setup(true);
    fs::create_dir(f.0.join("clicks.json")).unwrap();
    assert!(completion(base, defaults, c)
        .unwrap()
        .publish(&f.0, &grid, false)
        .unwrap()
        .is_some());
    assert_eq!(manifest(&f).finalized_project().unwrap().clicks.len(), 1);
}
#[test]
fn compact_protocol_stays_bounded_and_missing_or_wrong_mode_is_rejected() {
    let (_f, _base, _defaults, c, _grid) = setup(true);
    let message = super::super::Message::Terminal {
        summary: summary(),
        cursor: Terminal::Separate { completion: c },
    };
    let mut bytes = serde_json::to_vec(&message).unwrap();
    assert!(bytes.len() < 4096);
    bytes.push(b'\n');
    assert!(super::super::read_message(&mut &bytes[..]).is_ok());
    for bytes in [b"{\"event\":\"Prepared\",\"width\":322,\"height\":272}\n".as_slice(),b"{\"event\":\"Prepared\",\"width\":322,\"height\":272,\"cursor\":{\"mode\":\"separate\"}}\n"]{assert!(super::super::read_message(&mut &bytes[..]).is_err());}
}

#[test]
fn sampler_failure_retains_positions_and_disables_initial_drawing() {
    let (f, base, defaults, mut c, grid) = setup(true);
    c.degraded = true;
    c.terminal.stop_kind = StopKind::SamplerFailure;
    completion(base, defaults, c)
        .unwrap()
        .publish(&f.0, &grid, false)
        .unwrap();
    let m = manifest(&f);
    let p = m.finalized_project().unwrap();
    assert_eq!(p.video.cursor, CursorMode::Separate);
    assert!(!p.cursor_settings.enabled);
    assert!(m.diagnostic.is_some());
    assert_eq!(p.clicks.len(), 1);
}
