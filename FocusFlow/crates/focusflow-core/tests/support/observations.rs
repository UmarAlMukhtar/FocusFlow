#![allow(dead_code)]
use focusflow_core::{capture::windows::*, cursor_observation::*, legacy::LegacyDefaults};
pub fn defaults() -> LegacyDefaults {
    LegacyDefaults {
        scale: 2.0,
        intro_us: 300_000,
        hold_us: 1_000_000,
        outro_us: 350_000,
        pan_transition_us: 200_000,
        settle_us: 100_000,
    }
}
pub fn index() -> CursorObservationIndexV2 {
    CursorObservationIndexV2 {
        schema_version: 2,
        width: 322,
        height: 272,
        duration_us: 1_000_000,
        observations_path: OBSERVATIONS_PATH.into(),
        input_observations_path: INPUT_OBSERVATIONS_PATH.into(),
        shapes: vec![],
        clock: WindowsOwnedClockV1 {
            provenance: WindowsClockProvenance::OwnedWgcCausalCfrV1,
            qpc_frequency: "1000000".into(),
            admission_qpc: "0".into(),
            epoch_100ns: "0".into(),
            cutoff_qpc: "1000000".into(),
            cutoff_reason: WindowsCutoffReason::Stop,
            completed_frames: 30,
            worker_exit_code: 0,
            encoder_exit_code: 0,
            interactions: Unmeasured::Unmeasured,
            audio: Unmeasured::Unmeasured,
        },
        model: GeometryModel::DwmExtendedFrameTranslationV1,
        profile: ModelProfile::Decorated,
        observation: ObservationPolicy::WindowsCurrentQueryBracket,
        reconstruction: ReconstructionPolicy::CausalAfterBracket,
        max_hold_us: 250_000,
        spatial_error_px: NullOnly,
        frame_alignment_error_us: NullOnly,
        animation_phase: AnimationPhase::Unknown,
        button_coverage: ButtonCoverage::SampledEdges,
        drag_coverage: DragCoverage::SampledPath,
        terminal: ObservationTerminal {
            pose_barrier: None,
            input_barrier: None,
            source_cutoff_qpc: 1_000_000,
            stop_kind: StopKind::Stop,
        },
    }
}
pub fn window() -> WindowObservation {
    WindowObservation {
        hwnd: "123".into(),
        pid: 10,
        tid: 11,
        client_origin: [47, 40],
        client_size: [322, 272],
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
pub fn pose(q: i64) -> WindowsCursorRowV2 {
    let p = PointerObservation {
        physical_position: [56, 79],
        cursor_position: [56, 79],
        flags: 1,
        handle: "77".into(),
        hit_hwnd: "123".into(),
        root_hwnd: "123".into(),
        hit_pid: 10,
        hit_tid: 11,
    };
    WindowsCursorRowV2::Observed {
        q0: q.to_string(),
        q1: q.to_string(),
        before: window(),
        after: window(),
        pointer_before: p.clone(),
        pointer_after: p,
        shape_id: None,
        barrier_revision: 0,
    }
}
pub fn input(
    q0: i64,
    q1: i64,
    p: [i32; 2],
    left: Option<bool>,
    right: Option<bool>,
    basis: u32,
) -> WindowsInputRowV1 {
    WindowsInputRowV1::Observed {
        q0: q0.to_string(),
        q1: q1.to_string(),
        physical_position: [47 + p[0], 40 + p[1]],
        hit_hwnd: "123".into(),
        root_hwnd: "123".into(),
        hit_pid: 10,
        hit_tid: 11,
        buttons_swapped: false,
        left,
        right,
        basis_pose_row: Some(basis),
        barrier_revision_before: 0,
        barrier_revision_after: 0,
    }
}
pub fn drag_inputs(buttons: [bool; 2]) -> (Vec<WindowsCursorRowV2>, Vec<WindowsInputRowV1>) {
    let poses = [0, 16667, 33334, 50001, 66668, 83335, 100002, 116669]
        .into_iter()
        .map(pose)
        .collect();
    let inputs = [
        (0, [10, 10], false, 0),
        (20000, [10, 10], true, 1),
        (40000, [14, 10], true, 2),
        (80000, [14, 10], true, 4),
        (100000, [26, 10], true, 5),
        (120000, [27, 10], false, 7),
    ]
    .into_iter()
    .map(|(q, p, down, b)| {
        input(
            q,
            q,
            p,
            Some(down && buttons[0]),
            Some(down && buttons[1]),
            b,
        )
    })
    .collect();
    (poses, inputs)
}

/// Test-owned frozen V3 source; expected positions/pixels belong in each consumer test.
pub fn write_project(
    root: &std::path::Path,
    index: CursorObservationIndexV2,
    poses: Vec<WindowsCursorRowV2>,
) -> focusflow_core::project::ProjectDocument {
    use focusflow_core::{
        capture::{self, windows_observations::*},
        cursor_observation::ObservationData,
    };
    use std::fs;
    fs::create_dir_all(root.join("cursor/shapes")).unwrap();
    fs::write(
        root.join("cursor/index.json"),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join(OBSERVATIONS_PATH),
        poses
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    fs::write(root.join(INPUT_OBSERVATIONS_PATH), b"").unwrap();
    let timing = WindowsVideoTimingV2 {
        endpoint: WindowsSavedEndpointV2 {
            provenance: WindowsEndpointProvenance::SavedPacketFullGrid,
            end_pts: index.clock.completed_frames,
            time_base: capture::TimeBase {
                numerator: 1,
                denominator: 30,
            },
            interval: capture::Interval::HalfOpen,
        },
        clock: index.clock.clone(),
    };
    let mut manifest: WindowsCaptureManifestV3 = serde_json::from_value(serde_json::json!({
        "schemaVersion":3,"captureId":"fixture", "source":{
            "backend":"windowsWgcOwned", "kind":"window", "windowId":"123", "observedWindowBounds":null,
            "rawSize":{"width":index.width,"height":index.height}, "encodedSize":{"width":index.width,"height":index.height},
            "contentRect":{"x":0,"y":0,"width":index.width,"height":index.height},"pointerOrigin":null
        },
        "initialDefaults":{"frameStyle":focusflow_core::project::FrameStyle::default(),"camera":{"panTransitionUs":200000,"settleUs":100000}},
        "interactionDefaults": {"scale":2.0,"introUs":300000,"holdUs":1000000,"outroUs":350000,"panTransitionUs":200000,"settleUs":100000},
        "diagnostic":{"stage":"cursor","message":"Synthetic sampled observations"},
        "state":{"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","cursorIndexPath":"cursor/index.json"}
    })).unwrap();
    let data = ObservationData {
        index,
        poses,
        inputs: vec![],
    };
    // Start with a real typed incomplete authority rather than inventing a source project.
    let (doc, _) = manifest
        .build_initial_snapshot(&timing, Some(&data), None, Default::default())
        .unwrap();
    manifest.state = WindowsCaptureStateV3::Finalized {
        video_timing: timing,
        cursor_timing: ObservationPolicy::WindowsCurrentQueryBracket,
        initial_project: doc.clone(),
    };
    fs::write(
        root.join("capture.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("capture.windows-v3"),
        b"focusflow-windows-capture-v3\n",
    )
    .unwrap();
    doc
}
