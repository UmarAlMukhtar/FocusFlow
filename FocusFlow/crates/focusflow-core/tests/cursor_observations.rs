use focusflow_core::cursor_observation::*;
#[path = "support/observations.rs"]
mod support;
use support::*;
#[test]
fn strict_index_rejects_empty() {
    assert!(decode_observation_index(b"{}").is_err());
}
#[test]
fn strict_index_literals_nulls_and_duplicates() {
    let v = serde_json::to_value(index()).unwrap();
    let bytes = serde_json::to_vec(&v).unwrap();
    assert!(decode_observation_index(&bytes).is_ok());
    for field in [
        "spatialErrorPx",
        "frameAlignmentErrorUs",
        "terminal",
        "clock",
        "model",
        "observation",
    ] {
        let mut bad = v.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(
            decode_observation_index(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("spatialErrorPx", serde_json::json!(0)),
        ("model", serde_json::json!("measuredUnitTranslation")),
        ("animationPhase", serde_json::json!({"unknown":null})),
        ("observationsPath", serde_json::json!("../raw")),
        ("shapes", serde_json::json!([["id", "path", 1, 1, 0, 0]])),
    ] {
        let mut bad = v.clone();
        bad[field] = value;
        assert!(
            decode_observation_index(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "{field}"
        );
    }
    let text = String::from_utf8(bytes.clone()).unwrap();
    let bad = text.replacen("{", "{\"schemaVersion\":2,", 1);
    assert!(decode_observation_index(bad.as_bytes()).is_err());
    assert_eq!(bytes, serde_json::to_vec(&v).unwrap());
    for q in ["01", "+1", "-1", "1.0", "9223372036854775808", ""] {
        assert!(qpc(q).is_err(), "{q}");
    }
}
#[test]
fn literal_dwm_projection_not_window_origin() {
    let i = index();
    let p = pose(0);
    assert_eq!(
        project_observation(&p, &i).unwrap(),
        Some([9.0 / 322.0, 39.0 / 272.0])
    );
    let mut p = p.clone();
    if let WindowsCursorRowV2::Observed { after, .. } = &mut p {
        after.dwm_rect[0] += 1;
    }
    assert_eq!(project_observation(&p, &i).unwrap(), None);
    let mut p = pose(0);
    if let WindowsCursorRowV2::Observed { before, after, .. } = &mut p {
        before.target_pmv2 = false;
        after.target_pmv2 = false;
    }
    assert!(project_observation(&p, &i).unwrap().is_none());
}
#[test]
fn qpc_rational_admission_no_clip_or_float() {
    let mut i = index();
    i.clock.qpc_frequency = "3000000".into();
    i.clock.epoch_100ns = "10000000".into();
    i.clock.admission_qpc = "2999000".into();
    i.clock.cutoff_qpc = "6000000".into();
    i.terminal.source_cutoff_qpc = 6000000;
    i.validate().unwrap();
    let mut p = pose(3000000);
    if let WindowsCursorRowV2::Observed { q1, .. } = &mut p {
        *q1 = "3000006".into();
    }
    assert_eq!(i.ceil_us(3000006).unwrap(), 2);
    assert!(project_observation(&p, &i).unwrap().is_some());
    assert!(project_observation(&pose(2999999), &i).unwrap().is_none());
    assert!(project_observation(&pose(6000000), &i).unwrap().is_none());
}
#[test]
fn sampled_edges_retain_occurrence_and_input_upper_endpoint() {
    let i = index();
    let poses = vec![pose(0)];
    let inputs = vec![
        input(10, 12, [10, 10], Some(false), Some(false), 0),
        input(20, 22, [11, 10], Some(true), Some(false), 0),
    ];
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.project.clicks[0].at_us, 22);
    assert_eq!(r.click_evidence[0].occurrence_qpc, [10, 22]);
    let inputs = vec![
        input(0, 0, [10, 10], Some(false), Some(false), 0),
        input(7999, 8000, [11, 10], Some(true), Some(false), 0),
    ];
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.project.clicks[0].at_us, 8000);
    for inputs in [
        vec![input(20, 22, [10, 10], Some(true), Some(false), 0)],
        vec![
            input(10, 12, [10, 10], None, Some(false), 0),
            input(20, 22, [10, 10], Some(true), Some(false), 0),
        ],
        vec![
            input(10, 12, [10, 10], Some(false), Some(false), 0),
            input(20, 22, [10, 10], Some(false), Some(false), 0),
        ],
    ] {
        assert!(
            sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults())
                .unwrap()
                .project
                .clicks
                .is_empty()
        );
    }
}
#[test]
fn literal_both_button_drags_and_initial_zoom_targets() {
    for buttons in [[true, false], [false, true], [true, true]] {
        let mut i = index();
        i.duration_us = 2_000_000;
        i.clock.completed_frames = 60;
        i.clock.cutoff_qpc = "2000000".into();
        i.terminal.source_cutoff_qpc = 2000000;
        let (poses, inputs) = drag_inputs(buttons);
        let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
        let n = buttons.into_iter().filter(|v| *v).count();
        assert_eq!(r.click_evidence.len(), n);
        assert_eq!(r.drag_evidence.len(), n);
        for (d, e) in r.compatibility.drags.iter().zip(&r.drag_evidence) {
            assert_eq!((d.start, d.end), (0.02, 0.12));
            assert_eq!(
                d.points
                    .iter()
                    .map(|p| (p.timestamp, p.x, p.y))
                    .collect::<Vec<_>>(),
                vec![
                    (0.02, 10, 10),
                    (0.08, 14, 10),
                    (0.1, 26, 10),
                    (0.12, 27, 10)
                ]
            );
            assert_eq!(e.input_point_rows, vec![1, 3, 4, 5]);
            assert_eq!(e.start_edge.occurrence_qpc, [0, 20000]);
            assert_eq!(e.release_bracket, Some([100000, 120000]));
            assert_eq!(e.end_kind, DragEndKind::ObservedRelease);
        }
        assert_eq!(r.project.zooms.is_empty(), !buttons[0]);
        if buttons[0] {
            assert!(r.project.zooms.iter().any(|z| z.x == 27.0 / 322.0));
        }
    }
}
#[test]
fn stale_future_revision_and_barrier_do_not_bridge() {
    let i = index();
    let (poses, mut inputs) = drag_inputs([true, false]);
    for row in [
        input(34000, 34000, [10, 10], Some(true), Some(false), 0),
        input(100, 100, [10, 10], Some(true), Some(false), 1),
    ] {
        let r = sampled_interactions(
            &poses,
            &[input(0, 0, [10, 10], Some(false), Some(false), 0), row],
            &i.terminal,
            &i,
            defaults(),
        )
        .unwrap();
        assert!(r.project.clicks.is_empty());
    }
    inputs[4] = WindowsInputRowV1::Barrier {
        q0: "100000".into(),
        q1: "100000".into(),
        reason: "hidden".into(),
    };
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.drag_evidence.len(), 1);
    assert_eq!(r.drag_evidence[0].end_kind, DragEndKind::Interrupted);
    assert_eq!(r.drag_evidence[0].release_bracket, None);
    assert_eq!(r.drag_evidence[0].input_point_rows, vec![1, 3]);
    assert_eq!(r.compatibility.drags[0].end, 0.08);
    let mut bad = input(20, 22, [10, 10], Some(true), Some(false), 0);
    if let WindowsInputRowV1::Observed {
        barrier_revision_after,
        ..
    } = &mut bad
    {
        *barrier_revision_after = 1;
    }
    assert!(sampled_interactions(
        &poses,
        &[input(10, 12, [10, 10], Some(false), Some(false), 0), bad],
        &i.terminal,
        &i,
        defaults()
    )
    .unwrap()
    .project
    .clicks
    .is_empty());
}
#[test]
fn stop_held_and_small_caps_keep_measured_prefixes() {
    let i = index();
    let (poses, mut inputs) = drag_inputs([true, true]);
    inputs.pop();
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.drag_evidence.len(), 2);
    for e in r.drag_evidence {
        assert_eq!(e.end_kind, DragEndKind::Interrupted);
        assert!(e.release_bracket.is_none());
        assert_eq!(e.input_point_rows, vec![1, 3, 4]);
    }
    let r = sampled_interactions_with_limits(
        &poses,
        &inputs,
        &i.terminal,
        &i,
        defaults(),
        InteractionLimits {
            clicks: 2,
            drags: 2,
            points: 6,
            bytes: 8 * 1024 * 1024,
        },
    )
    .unwrap();
    assert!(r.truncated);
    assert_eq!(r.compatibility.clicks.len(), 2);
    let mut budget = ObservationBudget::with_limits(4096, 5, 16 * 1024 * 1024);
    assert!(budget.admit(1024, 1024));
    assert!(!budget.admit(1, 1));
    assert!(budget.admit_terminal(2048));
    assert!(budget.admit_terminal(1024));
    assert!(!budget.admit_terminal(1));
}
#[test]
fn all_declared_barriers_interrupt_at_last_real_input() {
    for cause in [
        "hidden",
        "unknownButton",
        "owner",
        "origin",
        "dpi",
        "overwide",
        "stale",
        "stop",
    ] {
        let mut i = index();
        let (mut poses, mut inputs) = drag_inputs([true, false]);
        inputs.pop();
        match cause {
            "hidden" => {
                if let WindowsCursorRowV2::Observed {
                    pointer_before,
                    pointer_after,
                    ..
                } = &mut poses[5]
                {
                    pointer_before.flags = 0;
                    pointer_after.flags = 0;
                }
            }
            "unknownButton" => {
                if let WindowsInputRowV1::Observed { left, .. } = &mut inputs[4] {
                    *left = None;
                }
            }
            "owner" => {
                if let WindowsCursorRowV2::Observed { before, after, .. } = &mut poses[5] {
                    before.pid = 20;
                    after.pid = 20;
                }
            }
            "origin" => {
                if let WindowsCursorRowV2::Observed { before, after, .. } = &mut poses[5] {
                    before.dwm_rect[0] += 1;
                    before.dwm_rect[2] += 1;
                    after.dwm_rect = before.dwm_rect;
                }
            }
            "dpi" => {
                if let WindowsCursorRowV2::Observed { before, after, .. } = &mut poses[5] {
                    before.dpi = 120;
                    after.dpi = 120;
                }
            }
            "overwide" => {
                if let WindowsInputRowV1::Observed { q1, .. } = &mut inputs[4] {
                    *q1 = "140000".into();
                }
            }
            "stale" => {
                if let WindowsInputRowV1::Observed { basis_pose_row, .. } = &mut inputs[4] {
                    *basis_pose_row = Some(0);
                }
            }
            "stop" => {
                i.terminal.input_barrier = Some(QpcBarrier {
                    q0: "99000".into(),
                    q1: "99001".into(),
                    reason: "sampler stopped".into(),
                });
                i.terminal.stop_kind = StopKind::SamplerFailure;
            }
            _ => unreachable!(),
        }
        let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
        assert_eq!(r.drag_evidence.len(), 1, "{cause}");
        assert_eq!(
            r.drag_evidence[0].end_kind,
            DragEndKind::Interrupted,
            "{cause}"
        );
        assert!(r.drag_evidence[0].release_bracket.is_none());
        assert_eq!(r.drag_evidence[0].input_point_rows, vec![1, 3], "{cause}");
    }
}
#[test]
fn stationary_release_selects_real_row_and_short_excursion_is_suppressed() {
    let i = index();
    let (poses, mut inputs) = drag_inputs([true, false]);
    if let WindowsInputRowV1::Observed {
        physical_position, ..
    } = &mut inputs[5]
    {
        *physical_position = [47 + 26, 40 + 10];
    }
    let before = serde_json::to_vec(&inputs).unwrap();
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.drag_evidence[0].input_point_rows, vec![1, 3, 5]);
    assert_eq!(r.compatibility.drags[0].points[2].timestamp, 0.12);
    assert_eq!(serde_json::to_vec(&inputs).unwrap(), before);
    for row in inputs.iter_mut().skip(2) {
        if let WindowsInputRowV1::Observed {
            physical_position, ..
        } = row
        {
            *physical_position = [60, 50];
        }
    }
    assert!(
        sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults())
            .unwrap()
            .drag_evidence
            .is_empty()
    );
}
#[test]
fn dangling_basis_errors_and_post_cutoff_cannot_create_capability() {
    let i = index();
    assert!(sampled_interactions(
        &[pose(0)],
        &[input(1, 1, [10, 10], Some(true), Some(false), 9)],
        &i.terminal,
        &i,
        defaults()
    )
    .is_err());
    assert!(project_observation(&pose(1_000_000), &i).unwrap().is_none());
    let mut i = index();
    i.clock.qpc_frequency = "3000000".into();
    i.clock.admission_qpc = "0".into();
    i.clock.epoch_100ns = "0".into();
    i.clock.completed_frames = 1;
    i.clock.cutoff_qpc = "100000".into();
    i.duration_us = 33333;
    i.terminal.source_cutoff_qpc = 100000;
    let inputs = [
        input(99990, 99990, [10, 10], Some(false), Some(false), 0),
        input(99998, 99999, [10, 10], Some(true), Some(false), 0),
    ];
    let poses = [pose(99980)];
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert!(r.project.clicks.is_empty());
}
#[test]
fn caps_are_deterministic_and_keep_simultaneous_endpoint_reserves() {
    let i = index();
    let (poses, inputs) = drag_inputs([true, true]);
    for limits in [
        InteractionLimits {
            clicks: 1,
            drags: 4096,
            points: 8192,
            bytes: 8 * 1024 * 1024,
        },
        InteractionLimits {
            clicks: 4096,
            drags: 1,
            points: 8192,
            bytes: 8 * 1024 * 1024,
        },
        InteractionLimits {
            clicks: 4096,
            drags: 4096,
            points: 6,
            bytes: 8 * 1024 * 1024,
        },
        InteractionLimits {
            clicks: 4096,
            drags: 4096,
            points: 8192,
            bytes: 24576,
        },
    ] {
        let a =
            sampled_interactions_with_limits(&poses, &inputs, &i.terminal, &i, defaults(), limits)
                .unwrap();
        let b =
            sampled_interactions_with_limits(&poses, &inputs, &i.terminal, &i, defaults(), limits)
                .unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert!(a.truncated);
        assert!(a.click_evidence.len() <= limits.clicks);
        assert!(a.drag_evidence.len() <= limits.drags);
        assert!(
            a.drag_evidence
                .iter()
                .map(|e| e.input_point_rows.len())
                .sum::<usize>()
                <= limits.points
        );
        assert!(serde_json::to_vec(&a).unwrap().len() <= limits.bytes);
        if limits.points == 6 {
            assert_eq!(a.drag_evidence.len(), 2);
            for e in a.drag_evidence {
                assert_eq!(e.input_point_rows, vec![1, 3, 4]);
            }
        }
    }
}
#[test]
fn physical_button_swap_is_explicit_and_mapping_change_is_barrier() {
    let i = index();
    let (poses, mut inputs) = drag_inputs([true, false]);
    for row in &mut inputs {
        if let WindowsInputRowV1::Observed {
            buttons_swapped, ..
        } = row
        {
            *buttons_swapped = true;
        }
    }
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(
        r.click_evidence[0].button,
        focusflow_core::interaction_timeline::ClickButton::Right
    );
    assert_eq!(
        r.drag_evidence[0].button,
        focusflow_core::interaction_timeline::ClickButton::Right
    );
    assert!(r.project.zooms.is_empty());
    if let WindowsInputRowV1::Observed {
        buttons_swapped, ..
    } = &mut inputs[4]
    {
        *buttons_swapped = false;
    }
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.drag_evidence[0].end_kind, DragEndKind::Interrupted);
    assert_eq!(r.drag_evidence[0].input_point_rows, vec![1, 3]);
}
#[test]
fn same_tick_barriers_dominate_and_cutoff_keeps_its_actual_end_cause() {
    let i = index();
    let poses = vec![
        WindowsCursorRowV2::Barrier {
            q0: "0".into(),
            q1: "0".into(),
            reason: "hidden".into(),
        },
        pose(0),
    ];
    let inputs = vec![
        input(0, 0, [10, 10], Some(false), Some(false), 1),
        input(8000, 8000, [10, 10], Some(true), Some(false), 1),
    ];
    assert!(
        sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults())
            .unwrap()
            .click_evidence
            .is_empty()
    );
    let poses = vec![pose(0)];
    let inputs = vec![
        input(0, 0, [10, 10], Some(false), Some(false), 0),
        input(8000, 8000, [10, 10], Some(true), Some(false), 0),
        WindowsInputRowV1::Barrier {
            q0: "8000".into(),
            q1: "8000".into(),
            reason: "query failed".into(),
        },
    ];
    assert!(
        sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults())
            .unwrap()
            .click_evidence
            .is_empty()
    );
    let mut i = index();
    i.terminal.stop_kind = StopKind::SourceEnd;
    let (poses, mut inputs) = drag_inputs([true, false]);
    inputs.push(input(
        1_000_000,
        1_000_000,
        [30, 10],
        Some(true),
        Some(false),
        7,
    ));
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.drag_evidence[0].end_reason, DragEndReason::Release);
    let (poses, mut inputs) = drag_inputs([true, false]);
    inputs.pop();
    inputs.push(input(
        1_000_000,
        1_000_000,
        [30, 10],
        Some(true),
        Some(false),
        7,
    ));
    let r = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert_eq!(r.drag_evidence[0].end_reason, DragEndReason::SourceCutoff);
    assert!(r.drag_evidence[0].end_bracket.is_none());
    assert_eq!(r.drag_evidence[0].input_point_rows, vec![1, 3, 4]);
}

mod common;
use focusflow_core::{cursor, project::CursorStyle};
use std::sync::atomic::AtomicBool;
fn literal_clock() -> CursorObservationIndexV2 {
    let mut i = index();
    i.clock.qpc_frequency = "3000000".into();
    i.clock.epoch_100ns = "10000000".into();
    i.clock.admission_qpc = "2999000".into();
    i.clock.cutoff_qpc = "6000000".into();
    i.terminal.source_cutoff_qpc = 6000000;
    i
}
fn ranged_pose(q0: i64, q1: i64, x: i32) -> WindowsCursorRowV2 {
    let mut p = pose(q0);
    if let WindowsCursorRowV2::Observed {
        q1: q,
        pointer_before,
        pointer_after,
        ..
    } = &mut p
    {
        *q = q1.to_string();
        pointer_before.physical_position[0] = 47 + x;
        pointer_before.cursor_position[0] = 47 + x;
        *pointer_after = pointer_before.clone();
    }
    p
}
fn prepared(
    i: CursorObservationIndexV2,
    poses: Vec<WindowsCursorRowV2>,
    tau: u64,
) -> (common::TestDir, std::sync::Arc<cursor::PreparedCursor>) {
    let t = common::TestDir::new();
    let mut doc = write_project(t.path(), i, poses);
    doc.cursor_settings.enabled = true;
    doc.cursor_settings.style = CursorStyle::Arrow;
    doc.cursor_settings.smoothing_us = tau;
    let c = cursor::prepare_cursor(t.path(), &doc, &AtomicBool::new(false))
        .expect("checked index2 must prepare")
        .unwrap();
    (t, c)
}
#[test]
fn prepared_observations_use_upper_endpoint_lower_hold_and_exact_rational_clock() {
    let (t, c) = prepared(
        literal_clock(),
        vec![
            pose(2999999),
            ranged_pose(3000000, 3000006, 9),
            pose(6000000),
        ],
        0,
    );
    assert_eq!(c.position_at(1, 1), None);
    assert_eq!(c.position_at(3999, 2000), None);
    assert_eq!(c.position_at(2, 1), Some([9.0 / 322.0, 39.0 / 272.0]));
    assert_eq!(c.position_at(499999, 2), Some([9.0 / 322.0, 39.0 / 272.0]));
    assert_eq!(c.position_at(250000, 1), None);
    assert_eq!(c.position_at(1_000_000, 1), None);
    assert_eq!(c.follow_focus_at(1, 1, 0, [0.4, 0.5]), [0.4, 0.5]);
    assert_eq!(
        c.follow_focus_at(300000, 1, 0, [0.4, 0.5]),
        [9.0 / 322.0, 39.0 / 272.0]
    );
    assert_eq!(c.follow_focus_at(300000, 1, 250000, [0.4, 0.5]), [0.4, 0.5]);
    std::fs::remove_dir_all(t.path().join("cursor")).unwrap();
    assert_eq!(c.position_at(2, 1), Some([9.0 / 322.0, 39.0 / 272.0]));
}
#[test]
fn prepared_barrier_cuts_at_lower_endpoint_and_new_segment_snaps() {
    let (_, c) = prepared(
        literal_clock(),
        vec![
            pose(3000000),
            WindowsCursorRowV2::Barrier {
                q0: "3000030".into(),
                q1: "3000060".into(),
                reason: "hidden".into(),
            },
            ranged_pose(3000090, 3000090, 109),
        ],
        100000,
    );
    assert_eq!(c.position_at(19, 2), Some([9.0 / 322.0, 39.0 / 272.0]));
    assert_eq!(c.position_at(10, 1), None);
    assert_eq!(c.position_at(29, 1), None);
    assert_eq!(c.position_at(30, 1), Some([109.0 / 322.0, 39.0 / 272.0]));
    assert_eq!(
        c.follow_focus_at(20, 1, 0, [0.4, 0.5]),
        [9.0 / 322.0, 39.0 / 272.0]
    );
    let (_, c) = prepared(
        literal_clock(),
        vec![
            pose(3000000),
            WindowsCursorRowV2::Barrier {
                q0: "3000030".into(),
                q1: "3000060".into(),
                reason: "hidden".into(),
            },
            ranged_pose(3000030, 3000060, 109),
        ],
        0,
    );
    assert_eq!(
        c.position_at(20, 1),
        None,
        "same tick barrier dominates later row"
    );
}
#[test]
fn prepared_smoothing_is_causal_at_common_frame_instants_and_hold_resets() {
    let (_, c) = prepared(
        literal_clock(),
        vec![
            pose(3000000),
            ranged_pose(3300000, 3300000, 109),
            ranged_pose(4500000, 4500000, 209),
        ],
        100000,
    );
    assert_eq!(c.position_at(100000, 1), Some([9.0 / 322.0, 39.0 / 272.0]));
    let p = c.position_at(200000, 1).unwrap();
    assert!((p[0] - 0.2242610431144589f64).abs() < 1e-12);
    assert_eq!(c.position_at(350000, 1), None);
    assert_eq!(
        c.position_at(500000, 1),
        Some([209.0 / 322.0, 39.0 / 272.0])
    );
    for k in 0..30 {
        assert_eq!(
            c.position_at(k * 1_000_000, 30),
            c.position_at(k * 2_000_000, 60)
        );
    }
    assert!(c.buffer_bytes() <= 128 * 1024 * 1024 + 32 * 1024 * 1024);
}
#[test]
fn observation_hotspot_must_be_inside_raw_content_and_showing_owned_creator() {
    let i = index();
    let p = pose(0);
    for kind in 0..5 {
        let mut p = p.clone();
        if let WindowsCursorRowV2::Observed {
            before,
            after,
            pointer_before,
            pointer_after,
            ..
        } = &mut p
        {
            match kind {
                0 => {
                    before.dwm_rect[2] -= 1;
                    *after = before.clone();
                    pointer_before.physical_position[0] = 368;
                    pointer_before.cursor_position[0] = 368;
                }
                1 => pointer_before.flags = 3,
                2 => pointer_before.flags = 0,
                3 => pointer_before.root_hwnd = "124".into(),
                _ => pointer_before.hit_tid = 12,
            }
            *pointer_after = pointer_before.clone();
        }
        assert!(
            project_observation(&p, &i).unwrap().is_none(),
            "ownership case {kind}"
        );
    }
}
#[test]
fn prepared_same_upper_endpoint_hidden_barrier_precedes_later_visible_poll() {
    let (_, c) = prepared(
        literal_clock(),
        vec![
            pose(3000000),
            WindowsCursorRowV2::Barrier {
                q0: "3000030".into(),
                q1: "3000060".into(),
                reason: "hidden".into(),
            },
            ranged_pose(3000060, 3000060, 109),
            ranged_pose(3000090, 3000090, 209),
        ],
        0,
    );
    assert_eq!(
        c.position_at(20, 1),
        None,
        "shared upper tick cannot erase hidden evidence"
    );
    assert_eq!(c.position_at(30, 1), Some([209.0 / 322.0, 39.0 / 272.0]));
}
#[test]
fn prepared_fractional_tick_eligibility_and_hold_do_not_round_to_microseconds() {
    let (_, c) = prepared(literal_clock(), vec![ranged_pose(3000001, 3000002, 9)], 0);
    assert_eq!(c.position_at(1, 2), None);
    assert_eq!(c.position_at(2, 3), Some([9.0 / 322.0, 39.0 / 272.0]));
    assert!(c.position_at(250000, 1).is_some());
    assert_eq!(c.position_at(750001, 3), None);
}
#[test]
fn literal_out_of_epoch_rows_alone_create_no_draw_follow_or_interactions() {
    let i = literal_clock();
    let poses = vec![pose(2999999), pose(6000000)];
    let inputs = vec![
        input(2999998, 2999998, [9, 39], Some(false), Some(false), 0),
        input(2999999, 2999999, [9, 39], Some(true), Some(true), 0),
        input(6000000, 6000000, [109, 39], Some(true), Some(true), 1),
    ];
    let result = sampled_interactions(&poses, &inputs, &i.terminal, &i, defaults()).unwrap();
    assert!(result.project.clicks.is_empty());
    assert!(result.project.zooms.is_empty());
    assert!(result.compatibility.drags.is_empty());
    let t = common::TestDir::new();
    let doc = write_project(t.path(), i, poses);
    assert_eq!(
        doc.video.cursor,
        focusflow_core::project::CursorMode::Absent
    );
    assert!(doc.cursor.is_none());
    assert!(!doc.cursor_settings.enabled);
    assert!(
        cursor::prepare_cursor(t.path(), &doc, &AtomicBool::new(false))
            .unwrap()
            .is_none()
    );
    let caps = cursor::capabilities(&doc, &cursor::inspect_cursor(t.path(), &doc));
    assert!(!caps.cursor_follow);
    assert!(!caps.cursor_replacement);
}
#[test]
fn equal_geometry_polls_keep_empirical_unknown_quality_without_false_gap_warning() {
    let t = common::TestDir::new();
    let poses = (0..1_000_000).step_by(100000).map(pose).collect();
    let mut doc = write_project(t.path(), index(), poses);
    doc.cursor_settings.enabled = true;
    doc.cursor_settings.style = CursorStyle::Arrow;
    let report = cursor::inspect_cursor(t.path(), &doc);
    let cursor::CursorAssetReport::Available {
        quality: Some(q), ..
    } = report
    else {
        panic!("checked observation quality missing")
    };
    let mut quality = serde_json::to_value(q).unwrap();
    quality.as_object_mut().unwrap().remove("message");
    assert_eq!(
        quality,
        serde_json::json!({
            "observation":"sampledCurrent", "geometry":"empiricalDwmTranslation",
            "spatialErrorPx":null, "frameAlignmentErrorUs":null, "animationPhase":"unknown",
            "buttonCoverage":"sampledEdges", "dragCoverage":"sampledPath", "interactionTruncated":false
        })
    );
    let c = cursor::prepare_cursor(t.path(), &doc, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert!(!c.has_hidden_or_stale_gaps);
}
#[test]
fn prepared_extreme_valid_clock_and_future_follow_entry_are_safe() {
    let mut i = index();
    i.clock.qpc_frequency = i64::MAX.to_string();
    i.clock.cutoff_qpc = i64::MAX.to_string();
    i.terminal.source_cutoff_qpc = i64::MAX;
    let (_, c) = prepared(i, vec![pose(0)], 0);
    assert_eq!(
        c.follow_focus_at(300000, 1, u64::MAX, [0.4, 0.5]),
        [0.4, 0.5]
    );
    assert_eq!(c.position_at(u128::MAX, u32::MAX), None);
    assert_eq!(
        c.follow_focus_at(u128::MAX, u32::MAX, 0, [0.4, 0.5]),
        [0.4, 0.5]
    );
    assert_eq!(c.position_at(0, 0), None);
}
