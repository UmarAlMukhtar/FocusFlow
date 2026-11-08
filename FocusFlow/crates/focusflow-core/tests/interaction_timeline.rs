use focusflow_core::interaction_timeline::*;
#[test]
fn click_golden() {
    let t = build_timeline_segments(
        vec![ClickEvent {
            timestamp: 1.0,
            x: 10,
            y: 20,
            button: ClickButton::Left,
        }],
        vec![],
    );
    assert_eq!(
        serde_json::to_value(t).unwrap(),
        serde_json::json!([{ "start":0.7,"end":2.0,"x":10,"y":20,"scale":2.0}])
    );
}

#[test]
fn legacy_drag_golden() {
    let drag = DragInteraction {
        start: 1.0,
        end: 1.2,
        button: ClickButton::Left,
        points: vec![
            DragPoint {
                timestamp: 1.0,
                x: 10,
                y: 20,
            },
            DragPoint {
                timestamp: 1.1,
                x: 14,
                y: 20,
            },
            DragPoint {
                timestamp: 1.1,
                x: 14,
                y: 20,
            },
            DragPoint {
                timestamp: 1.2,
                x: 26,
                y: 20,
            },
        ],
    };
    assert_eq!(
        serde_json::to_value(build_timeline_segments(vec![], vec![drag.clone()])).unwrap(),
        serde_json::json!([{ "start":0.7,"end":1.1,"x":10,"y":20,"scale":2.0},{"start":1.1,"end":1.2,"x":14,"y":20,"scale":2.0},{"start":1.2,"end":2.2,"x":26,"y":20,"scale":2.0}])
    );
    let mut right = drag.clone();
    right.button = ClickButton::Right;
    assert!(build_timeline_segments(vec![], vec![right]).is_empty());
    assert_eq!(
        serde_json::to_value(build_timeline_segments(
            vec![ClickEvent {
                timestamp: 1.0,
                x: 10,
                y: 20,
                button: ClickButton::Left
            }],
            vec![drag]
        ))
        .unwrap(),
        serde_json::json!([{ "start":0.7,"end":2.0,"x":10,"y":20,"scale":2.0},{"start":2.0,"end":2.2,"x":26,"y":20,"scale":2.0}])
    );
}
#[test]
fn literal_frozen_defaults_and_idle_tail_clipping() {
    let timeline = build_timeline_segments(
        vec![ClickEvent {
            timestamp: 1.0,
            x: 10,
            y: 20,
            button: ClickButton::Left,
        }],
        vec![],
    );
    let converted = focusflow_core::legacy::convert_interactions(
        focusflow_core::ffmpeg::ProbedVideo {
            width: 322,
            height: 272,
            duration_us: 1_500_000,
        },
        focusflow_core::legacy::LegacyDefaults {
            scale: 3.0,
            intro_us: 123,
            hold_us: 456,
            outro_us: 789,
            pan_transition_us: 999,
            settle_us: 888,
        },
        serde_json::to_value(timeline).unwrap(),
        serde_json::json!([{ "timestamp":1.0,"x":10,"y":20,"button":"left"}]),
        Some(serde_json::json!([])),
    )
    .unwrap();
    assert_eq!(converted.zooms.len(), 1);
    let z = &converted.zooms[0];
    assert_eq!(
        (z.start_us, z.end_us, z.intro_us, z.hold_us, z.outro_us),
        (700000, 1500000, 123, 456, 789)
    );
    assert_eq!((z.x, z.y, z.scale), (10.0 / 322.0, 20.0 / 272.0, 3.0));
    assert_eq!(
        converted.warnings,
        vec![
            focusflow_core::project::ProjectWarning::LegacyScaleOverridden,
            focusflow_core::project::ProjectWarning::LegacyInteractionClipped
        ]
    );
}
