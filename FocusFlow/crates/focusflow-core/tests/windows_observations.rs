mod common;
#[path = "support/observations.rs"]
mod support;
use common::TestDir;
use focusflow_core::{
    capture::{self, windows::*, windows_observations::*},
    cursor_observation::*,
    ffmpeg::*,
    project::*,
    store::ProjectStore,
};
use serde_json::{json, Value};
use std::{fs, path::Path};
fn manifest() -> WindowsCaptureManifestV3 {
    let i = support::index();
    let d = support::defaults();
    let doc = ProjectDocument {
        schema_version: 3,
        project_id: "fixture".into(),
        revision: 0,
        video: VideoAsset {
            path: "screen.mp4".into(),
            width: 322,
            height: 272,
            duration_us: 1_000_000,
            cursor: CursorMode::Separate,
        },
        cursor: Some(CursorAsset {
            index_path: "cursor/index.json".into(),
        }),
        cursor_settings: CursorSettings::default(),
        audio_tracks: None,
        microphone: None,
        timing: Timing {
            provenance: TimingProvenance::LegacyUnknown,
            audio_offset_us: None,
        },
        camera: CameraDefaults {
            pan_transition_us: d.pan_transition_us,
            settle_us: d.settle_us,
        },
        frame_style: Default::default(),
        clips: vec![Clip {
            id: "clip-0".into(),
            in_us: 0,
            out_us: 1_000_000,
        }],
        zooms: vec![],
        clicks: vec![],
        warnings: vec![
            ProjectWarning::LegacyTimingUnknown,
            ProjectWarning::CursorCaptureIncomplete,
        ],
    };
    WindowsCaptureManifestV3 {
        schema_version: 3,
        capture_id: "fixture".into(),
        source: WindowsSourceV2 {
            backend: WindowsBackend::WindowsWgcOwned,
            kind: WindowsSourceKind::Window,
            window_id: "123".into(),
            observed_window_bounds: None,
            raw_size: PixelSize {
                width: 322,
                height: 272,
            },
            encoded_size: PixelSize {
                width: 322,
                height: 272,
            },
            content_rect: capture::Bounds {
                x: 0,
                y: 0,
                width: 322,
                height: 272,
            },
            pointer_origin: NullOnly,
        },
        initial_defaults: WindowsInitialDefaults {
            frame_style: doc.frame_style.clone(),
            camera: doc.camera.clone(),
        },
        interaction_defaults: d.into(),
        diagnostic: Some(capture::CaptureDiagnosticV1 {
            stage: capture::DiagnosticStage::Cursor,
            message: "Position-only resource snapshot".into(),
        }),
        state: WindowsCaptureStateV3::Finalized {
            video_timing: WindowsVideoTimingV2 {
                endpoint: WindowsSavedEndpointV2 {
                    provenance: WindowsEndpointProvenance::SavedPacketFullGrid,
                    end_pts: 30,
                    time_base: capture::TimeBase {
                        numerator: 1,
                        denominator: 30,
                    },
                    interval: capture::Interval::HalfOpen,
                },
                clock: i.clock,
            },
            cursor_timing: ObservationPolicy::WindowsCurrentQueryBracket,
            initial_project: doc,
        },
    }
}
fn evidence() -> SavedCfrGrid {
    SavedCfrGrid {
        completed_frames: 30,
        video: ProbedVideoEvidence {
            video: ProbedVideo {
                width: 322,
                height: 272,
                duration_us: 1_000_000,
            },
            endpoint: PacketTime {
                ticks: 30,
                numerator: 1,
                denominator: 30,
            },
            first_packet_duration: PacketTime {
                ticks: 1,
                numerator: 1,
                denominator: 30,
            },
        },
    }
}
fn setup() -> (TestDir, std::path::PathBuf, ProjectStore) {
    let t = TestDir::new();
    let dir = t.path().join("fixture");
    fs::create_dir(&dir).unwrap();
    let store = ProjectStore::new(t.path()).unwrap();
    (t, dir, store)
}
fn save_data(
    dir: &Path,
    poses: &[WindowsCursorRowV2],
    inputs: &[WindowsInputRowV1],
    index: &CursorObservationIndexV2,
) {
    fs::create_dir_all(dir.join("cursor")).unwrap();
    fs::write(
        dir.join("cursor/index.json"),
        serde_json::to_vec(index).unwrap(),
    )
    .unwrap();
    for (path, rows) in [
        (
            OBSERVATIONS_PATH,
            poses
                .iter()
                .map(|v| serde_json::to_string(v).unwrap())
                .collect::<Vec<_>>(),
        ),
        (
            INPUT_OBSERVATIONS_PATH,
            inputs
                .iter()
                .map(|v| serde_json::to_string(v).unwrap())
                .collect::<Vec<_>>(),
        ),
    ] {
        fs::write(
            dir.join(path),
            if rows.is_empty() {
                String::new()
            } else {
                rows.join("\n") + "\n"
            },
        )
        .unwrap();
    }
    fs::write(dir.join("screen.mp4"), b"supplied packet-grid fixture").unwrap();
}
fn incomplete(m: &WindowsCaptureManifestV3) -> WindowsCaptureManifestV3 {
    let mut m = m.clone();
    m.state = WindowsCaptureStateV3::Incomplete {
        video_path: capture::VideoPath::Screen,
        video_cursor: capture::SeparateCursor::Separate,
        cursor_index_path: ObservationIndexPath::Index,
    };
    m
}
#[test]
fn strict_manifest_rejects_empty() {
    assert!(decode_windows_observations(b"{}", "session").is_err());
}
#[test]
fn strict_manifest_versions_missing_unknown_and_nested_duplicates() {
    let m = manifest();
    let v = serde_json::to_value(&m).unwrap();
    decode_windows_observations(&serde_json::to_vec(&v).unwrap(), "fixture").unwrap();
    assert!(capture::decode_windows(&serde_json::to_vec(&v).unwrap(), "fixture").is_err());
    assert!(capture::decode(&serde_json::to_vec(&v).unwrap(), "fixture").is_err());
    for key in v.as_object().unwrap().keys() {
        let mut bad = v.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(
            decode_windows_observations(&serde_json::to_vec(&bad).unwrap(), "fixture").is_err(),
            "{key}"
        );
    }
    for (path, bad) in [
        ("/schemaVersion", json!(2)),
        ("/source/pointerOrigin", json!([47, 40])),
        ("/interactionDefaults", json!([2, 0, 0, 0, 0, 0])),
        ("/interactionDefaults/scale", json!(9)),
        ("/interactionDefaults/settleUs", json!(999)),
        ("/state/initialProject/schemaVersion", json!(4)),
        ("/state/initialProject/video/cursor", json!("embedded")),
        ("/state/cursorTiming", Value::Null),
    ] {
        let mut b = v.clone();
        *b.pointer_mut(path).unwrap() = bad;
        assert!(
            decode_windows_observations(&serde_json::to_vec(&b).unwrap(), "fixture").is_err(),
            "{path}"
        );
    }
    let text = serde_json::to_string(&v).unwrap().replace(
        "\"pointerOrigin\":null",
        "\"pointerOrigin\":null,\"pointerOrigin\":null",
    );
    assert!(decode_windows_observations(text.as_bytes(), "fixture").is_err());
    let mut b = v.clone();
    b["source"]["extra"] = json!(1);
    assert!(decode_windows_observations(&serde_json::to_vec(&b).unwrap(), "fixture").is_err());
}
#[test]
fn source_join_requires_exact_raw_clock_and_grid() {
    let mut m = manifest();
    let data = ObservationData {
        index: support::index(),
        poses: vec![support::pose(0)],
        inputs: vec![],
    };
    bind_observations(&m, &data).unwrap();
    m.source.raw_size.width = 321;
    m.source.content_rect.width = 321;
    assert!(m.validate("fixture").is_ok());
    assert!(bind_observations(&m, &data).is_err());
    // Both 643 and 644 are provisionally compatible with encoded644; only frozen643 is authoritative.
    let mut m = manifest();
    m.source.raw_size.width = 643;
    m.source.encoded_size.width = 644;
    m.source.content_rect.width = 643;
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project.video.width = 644;
    }
    let mut data = data;
    data.index.width = 644;
    if let WindowsCursorRowV2::Observed { before, after, .. } = &mut data.poses[0] {
        before.dwm_rect[2] = 691;
        after.dwm_rect[2] = 691;
    }
    assert!(project_observation(&data.poses[0], &data.index)
        .unwrap()
        .is_some());
    assert!(bind_observations(&m, &data).is_err());
    let mut m = manifest();
    if let WindowsCaptureStateV3::Finalized { video_timing, .. } = &mut m.state {
        video_timing.clock.admission_qpc = "1".into();
    }
    assert!(bind_observations(&m, &data).is_err());
}
#[test]
fn publication_import_boundary_ignores_legacy_json_and_preserves_source() {
    let (_t, dir, store) = setup();
    let i = support::index();
    let (poses, inputs) = support::drag_inputs([true, true]);
    save_data(&dir, &poses, &inputs, &i);
    let result =
        sampled_interactions(&poses, &inputs, &i.terminal, &i, support::defaults()).unwrap();
    let mut m = manifest();
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project.zooms = result.project.zooms;
        initial_project.clicks = result.project.clicks;
        initial_project.warnings = vec![ProjectWarning::LegacyTimingUnknown];
        initial_project.warnings.extend(result.project.warnings);
        initial_project
            .warnings
            .push(ProjectWarning::CursorCaptureIncomplete);
    }
    let original = fs::read(dir.join(OBSERVATIONS_PATH)).unwrap();
    publish_windows_observations_incomplete(&dir, &incomplete(&m)).unwrap();
    assert!(store
        .open_or_import("fixture", Path::new("unused"), support::defaults())
        .is_err());
    let doc = validate_windows_observation_assets(&dir, &m, &evidence()).unwrap();
    assert_eq!(doc.clicks.len(), 2);
    assert!(!doc.zooms.is_empty());
    fs::write(dir.join("clicks.json"), b"forged").unwrap();
    fs::write(dir.join("drags.json"), b"forged").unwrap();
    fs::write(dir.join("timeline.json"), b"forged").unwrap();
    assert_eq!(
        validate_windows_observation_assets(&dir, &m, &evidence()).unwrap(),
        doc
    );
    let committed = publish_windows_observations_finalized_assets(&dir, &m, &evidence()).unwrap();
    assert!(publish_windows_observations_finalized_assets(&dir, &committed, &evidence()).is_err());
    assert_eq!(fs::read(dir.join(OBSERVATIONS_PATH)).unwrap(), original);
    let caps = store.capabilities(&doc);
    assert!(caps.cursor_observation_quality.is_some());
    assert!(caps.cursor_follow);
    assert!(!caps.cursor_native_style);
    let mut bad = m.clone();
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut bad.state
    {
        initial_project.clicks.clear();
    }
    assert!(validate_windows_observation_assets(&dir, &bad, &evidence()).is_err());
}
#[test]
fn markers_missing_index_and_private_evidence_never_fall_back() {
    let (_t, dir, store) = setup();
    let m = manifest();
    publish_windows_observations_incomplete(&dir, &incomplete(&m)).unwrap();
    let before = fs::read(dir.join("capture.json")).unwrap();
    fs::write(
        dir.join("capture.windows-v2"),
        b"focusflow-windows-capture-v2\n",
    )
    .unwrap();
    assert!(store
        .open_or_import("fixture", Path::new("unused"), support::defaults())
        .is_err());
    assert_eq!(fs::read(dir.join("capture.json")).unwrap(), before);
    fs::remove_file(dir.join("capture.windows-v2")).unwrap();
    fs::write(dir.join("capture.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    assert!(store
        .open_or_import("fixture", Path::new("unused"), support::defaults())
        .is_err());
    assert!(!dir.join("project.json").exists());
    fs::remove_file(dir.join("capture.json")).unwrap();
    assert!(store
        .open_or_import("fixture", Path::new("unused"), support::defaults())
        .is_err());
    fs::write(
        dir.join("cursor-proof.json"),
        b"{\"cursorProofPassed\":true,\"productionCursorQualified\":false}",
    )
    .unwrap();
    assert!(store
        .open_or_import("fixture", Path::new("unused"), support::defaults())
        .is_err());
}
#[test]
fn bounded_stream_reader_rejects_duplicate_rows_unsafe_refs_and_oversized_line() {
    let (_t, dir, _) = setup();
    let i = support::index();
    save_data(&dir, &[support::pose(0)], &[], &i);
    assert_eq!(read_observations(&dir).unwrap().poses.len(), 1);
    let before = fs::read(dir.join(OBSERVATIONS_PATH)).unwrap();
    let text = String::from_utf8(before.clone())
        .unwrap()
        .replace("\"q0\":\"0\"", "\"q0\":\"0\",\"q0\":\"0\"");
    fs::write(dir.join(OBSERVATIONS_PATH), text.as_bytes()).unwrap();
    assert!(read_observations(&dir).is_err());
    assert_eq!(
        fs::read(dir.join(OBSERVATIONS_PATH)).unwrap(),
        text.as_bytes()
    );
    fs::write(
        dir.join(OBSERVATIONS_PATH),
        vec![b' '; MAX_POSE_RECORD_BYTES + 1],
    )
    .unwrap();
    assert!(read_observations(&dir).is_err());
    fs::write(dir.join(OBSERVATIONS_PATH), before).unwrap();
    let mut i = i;
    i.observations_path = "../escape".into();
    fs::write(
        dir.join("cursor/index.json"),
        serde_json::to_vec(&i).unwrap(),
    )
    .unwrap();
    assert!(read_observations(&dir).is_err());
}
#[test]
fn source_join_owner_mismatch_and_unbound_index_capabilities_fail_closed() {
    let (_t, dir, store) = setup();
    let i = support::index();
    save_data(&dir, &[support::pose(0)], &[], &i);
    let m = manifest();
    let data = read_observations(&dir).unwrap();
    let mut other = m.clone();
    other.source.window_id = "999".into();
    assert!(bind_observations(&other, &data).is_err());
    assert!(
        !store
            .capabilities(m.finalized_project().unwrap())
            .cursor_follow
    );
    assert!(store
        .capabilities(m.finalized_project().unwrap())
        .cursor_observation_quality
        .is_none());
}
#[cfg(unix)]
#[test]
fn contained_symlink_observations_are_rejected_before_read() {
    let (_t, dir, _) = setup();
    save_data(&dir, &[support::pose(0)], &[], &support::index());
    fs::rename(
        dir.join(OBSERVATIONS_PATH),
        dir.join("cursor/original.ndjson"),
    )
    .unwrap();
    std::os::unix::fs::symlink("original.ndjson", dir.join(OBSERVATIONS_PATH)).unwrap();
    assert!(read_observations(&dir).is_err());
}
#[test]
fn absent_without_index_preserves_video_but_retained_malformed_index_cannot_hide() {
    let (_t, dir, _) = setup();
    fs::write(dir.join("screen.mp4"), b"supplied grid").unwrap();
    let mut m = manifest();
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project.video.cursor = CursorMode::Absent;
        initial_project.cursor = None;
    }
    publish_windows_observations_incomplete(&dir, &incomplete(&m)).unwrap();
    validate_windows_observation_assets(&dir, &m, &evidence()).unwrap();
    let mut nonempty = m.clone();
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut nonempty.state
    {
        initial_project.clicks.push(Click {
            at_us: 10,
            x: 0.5,
            y: 0.5,
        });
    }
    assert!(validate_windows_observation_assets(&dir, &nonempty, &evidence()).is_err());
    fs::create_dir(dir.join("cursor")).unwrap();
    fs::write(dir.join("cursor/index.json"), b"bad index").unwrap();
    assert!(validate_windows_observation_assets(&dir, &m, &evidence()).is_err());
    fs::remove_file(dir.join("cursor/index.json")).unwrap();
    publish_windows_observations_finalized_assets(&dir, &m, &evidence()).unwrap();
}
#[test]
fn shared_start_snapshot_uses_frozen_defaults_and_canonical_drags() {
    let m = manifest();
    let (poses, inputs) = support::drag_inputs([true, true]);
    let data = ObservationData {
        index: support::index(),
        poses,
        inputs,
    };
    let WindowsCaptureStateV3::Finalized { video_timing, .. } = &m.state else {
        unreachable!()
    };
    let (doc, result) = incomplete(&m)
        .build_initial_snapshot(video_timing, Some(&data), None, CursorSettings::default())
        .unwrap();
    assert_eq!(doc.clicks, result.project.clicks);
    assert_eq!(doc.zooms, result.project.zooms);
    assert!(!doc.zooms.is_empty());
    assert_eq!(doc.camera, m.initial_defaults.camera);
    assert_eq!(doc.schema_version, 3);
    assert_eq!(doc.revision, 0);
}
#[test]
fn native_resource_assets_validate_without_promoting_animation_phase() {
    let (_t, dir, _) = setup();
    let mut i = support::index();
    i.shapes.push(focusflow_core::cursor::CursorShape {
        id: "shape1".into(),
        path: "cursor/shapes/shape1.pam".into(),
        width: 1,
        height: 1,
        hotspot_x: 0,
        hotspot_y: 0,
    });
    let mut pose = support::pose(0);
    if let WindowsCursorRowV2::Observed { shape_id, .. } = &mut pose {
        *shape_id = Some("shape1".into());
    }
    save_data(&dir, &[pose], &[], &i);
    fs::create_dir(dir.join("cursor/shapes")).unwrap();
    let path = dir.join("cursor/shapes/shape1.pam");
    fs::write(
        &path,
        b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\xff\xff\xff\xff",
    )
    .unwrap();
    let mut m = manifest();
    m.diagnostic = None;
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project
            .warnings
            .retain(|w| *w != ProjectWarning::CursorCaptureIncomplete);
        initial_project.cursor_settings.enabled = true;
    }
    validate_windows_observation_assets(&dir, &m, &evidence()).unwrap();
    fs::write(&path, b"corrupt resource").unwrap();
    assert!(validate_windows_observation_assets(&dir, &m, &evidence()).is_err());
}
#[test]
fn recovery_marker_cannot_be_replaced_by_an_older_publisher() {
    let (_t, dir, _) = setup();
    publish_windows_observations_incomplete(&dir, &incomplete(&manifest())).unwrap();
    fs::remove_file(dir.join("capture.json")).unwrap();
    let old = capture::decode_windows(
        include_bytes!("fixtures/windows-incomplete.json"),
        "fixture",
    )
    .unwrap();
    assert!(capture::publish_windows_incomplete(&dir, &old).is_err());
    assert!(!dir.join("capture.windows-v2").exists());
    assert!(!dir.join("capture.json").exists());
    let linux=capture::decode(br#"{"schemaVersion":1,"captureId":"fixture","source":{"backend":"linuxX11","kind":"region","initialBounds":{"x":0,"y":0,"width":100,"height":50},"windowId":null},"diagnostic":null,"state":{"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"}}"#,"fixture").unwrap();
    assert!(capture::publish(&dir, &linux).is_err());
    assert!(!dir.join("capture.json").exists());
}
#[test]
fn required_nullable_input_and_pose_fields_are_not_defaulted() {
    let (_t, dir, _) = setup();
    let i = support::index();
    let pose = support::pose(0);
    let input = support::input(0, 0, [10, 10], Some(false), Some(false), 0);
    save_data(&dir, &[pose.clone()], &[input.clone()], &i);
    let pv = serde_json::to_value(pose).unwrap();
    for key in [
        "shapeId",
        "before",
        "after",
        "pointerBefore",
        "pointerAfter",
        "barrierRevision",
        "q0",
        "q1",
    ] {
        let mut bad = pv.clone();
        bad.as_object_mut().unwrap().remove(key);
        fs::write(dir.join(OBSERVATIONS_PATH), bad.to_string() + "\n").unwrap();
        assert!(read_observations(&dir).is_err(), "pose {key}");
    }
    fs::write(dir.join(OBSERVATIONS_PATH), pv.to_string() + "\n").unwrap();
    let iv = serde_json::to_value(input).unwrap();
    for key in [
        "left",
        "right",
        "basisPoseRow",
        "buttonsSwapped",
        "barrierRevisionBefore",
        "barrierRevisionAfter",
    ] {
        let mut bad = iv.clone();
        bad.as_object_mut().unwrap().remove(key);
        fs::write(dir.join(INPUT_OBSERVATIONS_PATH), bad.to_string() + "\n").unwrap();
        assert!(read_observations(&dir).is_err(), "input {key}");
    }
    let mut nullable = iv;
    nullable["left"] = Value::Null;
    nullable["right"] = Value::Null;
    nullable["basisPoseRow"] = Value::Null;
    fs::write(
        dir.join(INPUT_OBSERVATIONS_PATH),
        nullable.to_string() + "\n",
    )
    .unwrap();
    assert!(read_observations(&dir).is_ok());
}
#[test]
fn real_saved_cfr_grid_opens_v3_through_shared_store_without_source_writes() {
    let (_t, dir, store) = setup();
    let i = support::index();
    save_data(&dir, &[support::pose(0)], &[], &i);
    assert!(std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=322x272:r=30",
            "-frames:v",
            "30",
            "-c:v",
            "libx264",
            "-bf",
            "0",
            "-pix_fmt",
            "yuv420p"
        ])
        .arg(dir.join("screen.mp4"))
        .status()
        .unwrap()
        .success());
    let m = manifest();
    publish_windows_observations_incomplete(&dir, &incomplete(&m)).unwrap();
    publish_windows_observations_finalized_assets(&dir, &m, &evidence()).unwrap();
    let sources = [
        "screen.mp4",
        "capture.json",
        "cursor/index.json",
        OBSERVATIONS_PATH,
        INPUT_OBSERVATIONS_PATH,
    ];
    let bytes = sources
        .iter()
        .map(|p| fs::read(dir.join(p)).unwrap())
        .collect::<Vec<_>>();
    let doc = store
        .open_or_import("fixture", Path::new("ffmpeg"), support::defaults())
        .unwrap();
    assert_eq!(doc.schema_version, 3);
    assert_eq!(doc.revision, 0);
    assert_eq!(doc.video.cursor, CursorMode::Separate);
    assert!(store
        .capabilities(&doc)
        .cursor_observation_quality
        .is_some());
    for (p, b) in sources.iter().zip(bytes) {
        assert_eq!(fs::read(dir.join(p)).unwrap(), b, "{p}");
    }
    assert_eq!(
        store
            .open_or_import("fixture", Path::new("unused"), support::defaults())
            .unwrap(),
        doc
    );
}
#[test]
fn retained_pre_epoch_and_post_cutoff_rows_do_not_create_initial_cursor_capability() {
    let (_t, dir, store) = setup();
    let mut i = support::index();
    i.clock.qpc_frequency = "3000000".into();
    i.clock.epoch_100ns = "10000000".into();
    i.clock.admission_qpc = "2999000".into();
    i.clock.cutoff_qpc = "6000000".into();
    i.terminal.source_cutoff_qpc = 6000000;
    save_data(
        &dir,
        &[support::pose(2999999), support::pose(6000000)],
        &[],
        &i,
    );
    let original = fs::read(dir.join(OBSERVATIONS_PATH)).unwrap();
    let mut m = manifest();
    if let WindowsCaptureStateV3::Finalized {
        video_timing,
        initial_project,
        ..
    } = &mut m.state
    {
        video_timing.clock = i.clock;
        initial_project.video.cursor = CursorMode::Absent;
        initial_project.cursor = None;
    }
    let doc = validate_windows_observation_assets(&dir, &m, &evidence()).unwrap();
    assert!(!store.capabilities(&doc).cursor_follow);
    assert!(store
        .capabilities(&doc)
        .cursor_observation_quality
        .is_none());
    assert_eq!(fs::read(dir.join(OBSERVATIONS_PATH)).unwrap(), original);
}

#[test]
fn initial_capability_requires_a_stream_valid_drawable_pose_interval() {
    for cause in ["terminal", "sameTickBarrier", "sameTickHidden", "overlap"] {
        let (_t, dir, store) = setup();
        let mut index = support::index();
        let poses = match cause {
            "terminal" => {
                index.terminal.pose_barrier = Some(QpcBarrier {
                    q0: "0".into(),
                    q1: "0".into(),
                    reason: "pose sampler failed".into(),
                });
                index.terminal.stop_kind = StopKind::SamplerFailure;
                vec![support::pose(100)]
            }
            "sameTickBarrier" => vec![
                WindowsCursorRowV2::Barrier {
                    q0: "0".into(),
                    q1: "0".into(),
                    reason: "hidden".into(),
                },
                support::pose(0),
            ],
            "sameTickHidden" => {
                let mut hidden = support::pose(0);
                if let WindowsCursorRowV2::Observed {
                    pointer_before,
                    pointer_after,
                    ..
                } = &mut hidden
                {
                    pointer_before.flags = 0;
                    pointer_after.flags = 0;
                }
                vec![hidden, support::pose(0)]
            }
            "overlap" => {
                let mut pose = support::pose(0);
                if let WindowsCursorRowV2::Observed { q1, .. } = &mut pose {
                    *q1 = "10".into();
                }
                vec![
                    pose,
                    WindowsCursorRowV2::Barrier {
                        q0: "5".into(),
                        q1: "10".into(),
                        reason: "invalidated before query completed".into(),
                    },
                ]
            }
            _ => unreachable!(),
        };
        save_data(&dir, &poses, &[], &index);
        let data = read_observations(&dir).unwrap();
        let mut m = manifest();
        let WindowsCaptureStateV3::Finalized { video_timing, .. } = &m.state else {
            unreachable!()
        };
        let (doc, result) = incomplete(&m)
            .build_initial_snapshot(video_timing, Some(&data), None, CursorSettings::default())
            .unwrap();
        assert_eq!(doc.video.cursor, CursorMode::Absent, "{cause}");
        assert!(doc.cursor.is_none());
        assert!(result.project.clicks.is_empty());
        if let WindowsCaptureStateV3::Finalized {
            initial_project, ..
        } = &mut m.state
        {
            *initial_project = doc.clone();
        }
        checked_interactions(&m, &data).unwrap();
        assert_eq!(
            validate_windows_observation_assets(&dir, &m, &evidence()).unwrap(),
            doc
        );
        assert!(!store.capabilities(&doc).cursor_follow);
        assert!(store
            .capabilities(&doc)
            .cursor_observation_quality
            .is_none());
        if let WindowsCaptureStateV3::Finalized {
            initial_project, ..
        } = &mut m.state
        {
            initial_project.video.cursor = CursorMode::Separate;
            initial_project.cursor = Some(CursorAsset {
                index_path: "cursor/index.json".into(),
            });
        }
        assert!(
            validate_windows_observation_assets(&dir, &m, &evidence()).is_err(),
            "{cause}"
        );
    }
}

#[test]
fn sampler_failure_requires_disabled_declared_initial_degradation_but_preserves_prefix_edits() {
    let (_t, dir, store) = setup();
    let mut index = support::index();
    index.terminal.stop_kind = StopKind::SamplerFailure;
    index.terminal.input_barrier = Some(QpcBarrier {
        q0: "100000".into(),
        q1: "100001".into(),
        reason: "input sampler failed".into(),
    });
    index.terminal.pose_barrier = Some(QpcBarrier {
        q0: "100000".into(),
        q1: "100001".into(),
        reason: "pose sampler failed".into(),
    });
    index.shapes.push(focusflow_core::cursor::CursorShape {
        id: "shape1".into(),
        path: "cursor/shapes/shape1.pam".into(),
        width: 1,
        height: 1,
        hotspot_x: 0,
        hotspot_y: 0,
    });
    let mut pose = support::pose(0);
    if let WindowsCursorRowV2::Observed { shape_id, .. } = &mut pose {
        *shape_id = Some("shape1".into());
    }
    save_data(&dir, &[pose], &[], &index);
    fs::create_dir(dir.join("cursor/shapes")).unwrap();
    fs::write(
        dir.join("cursor/shapes/shape1.pam"),
        b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\xff\xff\xff\xff",
    )
    .unwrap();
    let mut m = manifest();
    m.diagnostic = None;
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project.cursor_settings.enabled = true;
        initial_project
            .warnings
            .retain(|w| *w != ProjectWarning::CursorCaptureIncomplete);
    }
    assert!(validate_windows_observation_assets(&dir, &m, &evidence()).is_err());
    m.diagnostic = Some(capture::CaptureDiagnosticV1 {
        stage: capture::DiagnosticStage::Cursor,
        message: "sampler failed; retained prefix only".into(),
    });
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project
            .warnings
            .push(ProjectWarning::CursorCaptureIncomplete);
    }
    assert!(validate_windows_observation_assets(&dir, &m, &evidence()).is_err());
    if let WindowsCaptureStateV3::Finalized {
        initial_project, ..
    } = &mut m.state
    {
        initial_project.cursor_settings.enabled = false;
    }
    let mut doc = validate_windows_observation_assets(&dir, &m, &evidence()).unwrap();
    fs::write(dir.join("capture.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    fs::write(
        dir.join("capture.windows-v3"),
        b"focusflow-windows-capture-v3\n",
    )
    .unwrap();
    doc.cursor_settings.enabled = true;
    doc.revision = 1;
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(dir.join(OBSERVATIONS_PATH))
            .unwrap(),
        "{}",
        serde_json::to_string(&support::pose(200000)).unwrap()
    )
    .unwrap();
    let caps = store.capabilities(&doc);
    assert!(caps.cursor_follow);
    assert!(caps.cursor_native_style);
    assert!(caps.cursor_observation_quality.is_some());
    assert!(!m.finalized_project().unwrap().cursor_settings.enabled);
}

#[test]
fn producer_memory_cap_reserves_both_terminal_rows_at_small_limits() {
    let pose_cost = std::mem::size_of::<WindowsCursorRowV2>() + 2 * MAX_POSE_RECORD_BYTES;
    let input_cost = std::mem::size_of::<WindowsInputRowV1>() + 2 * MAX_INPUT_RECORD_BYTES;
    let base = focusflow_core::cursor::MAX_INDEX_BYTES as usize + MAX_INTERACTION_BYTES + 8192;
    let mut tiny = ObservationBudget::with_limits(65536, 100, base + pose_cost);
    assert!(!tiny.admit(100, pose_cost));
    let mut exact = ObservationBudget::with_limits(65536, 100, base + 2 * pose_cost + input_cost);
    assert!(exact.admit(100, pose_cost));
    assert!(!exact.admit(100, pose_cost));
    assert!(exact.admit_terminal(100));
    assert!(exact.admit_terminal(100));
    assert!(!exact.admit_terminal(100));
}

#[test]
fn default_producer_memory_boundary_is_accepted_by_the_actual_canonical_reader() {
    use std::io::Write;
    let (_t, dir, _) = setup();
    let pose_cost = std::mem::size_of::<WindowsCursorRowV2>() + 2 * MAX_POSE_RECORD_BYTES;
    fs::create_dir(dir.join("cursor")).unwrap();
    let mut index = support::index();
    let barrier = QpcBarrier {
        q0: "0".into(),
        q1: "0".into(),
        reason: "budget".into(),
    };
    index.terminal.pose_barrier = Some(barrier.clone());
    index.terminal.input_barrier = Some(barrier);
    index.terminal.stop_kind = StopKind::Budget;
    fs::write(
        dir.join("cursor/index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    let pose_line = serde_json::to_string(&support::pose(0)).unwrap() + "\n";
    let barrier_line = b"{\"type\":\"barrier\",\"q0\":\"0\",\"q1\":\"0\",\"reason\":\"budget\"}\n";
    let mut budget = ObservationBudget::default();
    let mut output =
        std::io::BufWriter::new(fs::File::create(dir.join(OBSERVATIONS_PATH)).unwrap());
    let mut count = 0;
    while budget.admit(pose_line.len(), pose_cost) {
        output.write_all(pose_line.as_bytes()).unwrap();
        count += 1;
    }
    assert!(count > 27000);
    assert!(budget.admit_terminal(barrier_line.len()));
    assert!(budget.admit_terminal(barrier_line.len()));
    output.write_all(barrier_line).unwrap();
    output.flush().unwrap();
    fs::write(dir.join(INPUT_OBSERVATIONS_PATH), barrier_line).unwrap();
    let data = read_observations(&dir).unwrap();
    assert_eq!(data.poses.len(), count + 1);
    assert_eq!(data.inputs.len(), 1);
}
