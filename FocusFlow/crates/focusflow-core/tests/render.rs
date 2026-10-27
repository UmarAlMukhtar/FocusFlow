#[allow(dead_code)]
mod common;
use common::fixture_project;
use focusflow_core::{edits::{self, Edit}, project::*, render::{self, CameraPose, ExportPreset, RenderPlan}};

fn zoom(id: &str, start_us: u64, end_us: u64, scale: f64) -> ZoomElement {
    ZoomElement { mode: focusflow_core::project::ZoomMode::Fixed, id: id.into(), clip_id: "clip-0".into(), start_us, end_us,
        x: 0.25, y: 0.75, scale, intro_us: 180_000, hold_us: 150_000, outro_us: 220_000 }
}
fn pose(plan: &RenderPlan, at_us: u64) -> CameraPose {
    plan.camera.iter().find(|i| i.start_us <= at_us && at_us < i.end_us)
        .unwrap_or_else(|| plan.camera.last().unwrap()).pose_at(at_us)
}
fn near(actual: f64, expected: f64) { assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}"); }
fn center(actual: CameraPose) { assert_eq!(actual, CameraPose { x: 0.5, y: 0.5, scale: 1.0 }); }

#[test]
fn default_legacy_camera_is_characterized() {
    let mut doc = fixture_project();
    doc.camera = CameraDefaults { pan_transition_us: 180_000, settle_us: 120_000 };
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, 2.3), zoom("b", 2_000_000, 3_000_000, 2.3)];
    doc.zooms[1].x = 0.75;
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    center(pose(&plan, 1_000_000));
    near(pose(&plan, 1_090_000).scale, 1.65);
    assert_eq!(pose(&plan, 1_180_000), CameraPose { x: 0.25, y: 0.75, scale: 2.3 });
    near(pose(&plan, 2_000_000).x, 0.25);
    near(pose(&plan, 2_090_000).x, 0.5);
    near(pose(&plan, 2_180_000).x, 0.75);
    near(pose(&plan, 2_300_000).x, 0.75); // old settle boundary
    near(pose(&plan, 3_150_000).scale, 2.3);
    near(pose(&plan, 3_260_000).scale, 1.65);
    center(pose(&plan, 3_370_000));
    // Old desktop blends return across hold+outro; the core deliberately honors hold.
    let old_at_hold_end = 2.3 + (1.0 - 2.3) * (0.5 - 0.5 * (std::f64::consts::PI * 150.0 / 370.0).cos());
    assert!(old_at_hold_end < pose(&plan, 3_150_000).scale);
    doc.camera.pan_transition_us = 220_000; // backend default caps, never lengthens own intro
    let backend = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&backend, 2_180_000).x, 0.75);
}

#[test]
fn per_element_scale_changes_camera() {
    let mut doc = fixture_project();
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, 2.0), zoom("b", 3_000_000, 4_000_000, 3.0)];
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&plan, 1_180_000).scale, 2.0);
    center(pose(&plan, 2_370_000));
    near(pose(&plan, 3_180_000).scale, 3.0);
}

#[test]
fn adjacent_zooms_pan_without_center_reset() {
    let mut doc = fixture_project();
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, 2.0), zoom("b", 2_000_000, 3_000_000, 3.0)];
    doc.zooms[1].x = 0.75;
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&plan, 1_180_000).scale, 2.0);
    near(pose(&plan, 2_000_000).scale, 2.0);
    near(pose(&plan, 2_090_000).scale, 2.5);
    near(pose(&plan, 2_090_000).x, 0.5);
    near(pose(&plan, 2_180_000).scale, 3.0);
    assert!(plan.camera.iter().filter(|i| i.start_us >= 1_180_000 && i.end_us <= 3_000_000)
        .all(|i| i.from.scale >= 2.0 && i.to.scale >= 2.0));
}

#[test]
fn removed_ranges_remove_clicks() {
    let mut doc = fixture_project();
    doc.clicks = [0, 999_999, 1_000_000, 1_999_999, 2_000_000, 2_999_999, 3_000_000, 5_000_000]
        .into_iter().map(|at_us| Click { at_us, x: 0.25, y: 0.75 }).collect();
    let edited = edits::apply(&doc, &[
        Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1_000_000, right_clip_id: "middle".into() },
        Edit::SplitClip { clip_id: "middle".into(), at_us: 2_000_000, right_clip_id: "right".into() },
        Edit::RemoveClip { clip_id: "middle".into() },
        Edit::TrimClip { clip_id: "right".into(), in_us: 2_000_000, out_us: 3_000_000 },
    ]).unwrap();
    let plan = render::compile(&edited, ExportPreset::Balanced).unwrap();
    assert_eq!(plan.clicks.iter().map(|c| c.at_us).collect::<Vec<_>>(), [0, 999_999, 1_000_000, 1_999_999]);
    assert_eq!(plan.duration_us, 2_000_000);
    assert!(plan.clicks.iter().all(|c| c.x == 0.25 && c.y == 0.75));
}

#[test]
fn clicks_repeat_once_per_retained_occurrence_and_sort_stably() {
    let mut doc = fixture_project();
    doc.clips = vec![Clip { id: "late".into(), in_us: 2_000_001, out_us: 3_000_002 },
        Clip { id: "first".into(), in_us: 0, out_us: 1_000_001 },
        Clip { id: "repeat".into(), in_us: 2_000_001, out_us: 3_000_002 }];
    doc.clicks = vec![Click { at_us: 3_000_001, x: 0.1, y: 0.2 }, Click { at_us: 2_000_001, x: 0.3, y: 0.4 },
        Click { at_us: 0, x: 0.5, y: 0.6 }, Click { at_us: 3_000_002, x: 0.7, y: 0.8 }];
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    assert_eq!(plan.clicks.iter().map(|c| c.at_us).collect::<Vec<_>>(), [0, 1_000_000, 1_000_001, 2_000_002, 3_000_002]);
    near(plan.clicks[0].x, 0.3);
    assert_eq!(plan.duration_us, 3_000_003);
}

#[test]
fn envelope_overlap_continues_from_evaluated_outro_pose() {
    let mut doc = fixture_project();
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, 2.0), zoom("b", 2_260_000, 3_000_000, 3.0)];
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&plan, 2_205_000).scale, 1.75); // retimed truncated outro midpoint
    near(pose(&plan, 2_260_000).scale, 1.5); // halfway through original 220ms outro
    near(pose(&plan, 2_350_000).scale, 2.25);
    near(pose(&plan, 2_440_000).scale, 3.0);
    assert!(!plan.camera.iter().any(|i| i.start_us == 2_260_000 && i.from.scale == 1.0));
}

#[test]
fn element_timing_and_pan_cap_are_both_honored() {
    let mut doc = fixture_project();
    doc.camera.pan_transition_us = 100_000;
    doc.camera.settle_us = 120_000;
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, 2.0), zoom("b", 2_000_000, 3_000_000, 3.0)];
    doc.zooms[0].intro_us = 400_000; // isolated intro ignores pan cap
    doc.zooms[1].intro_us = 300_000; // chained intro is capped by pan
    doc.zooms[1].hold_us = 400_000;
    doc.zooms[1].outro_us = 600_000;
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&plan, 1_200_000).scale, 1.5);
    near(pose(&plan, 2_050_000).scale, 2.5);
    near(pose(&plan, 2_100_000).scale, 3.0);
    near(pose(&plan, 3_400_000).scale, 3.0);
    near(pose(&plan, 3_700_000).scale, 2.0);
    center(pose(&plan, 4_000_000));
    doc.zooms[1].intro_us = 40_000;
    let short = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&short, 2_020_000).scale, 2.5);
    near(pose(&short, 2_040_000).scale, 3.0);
}

#[test]
fn cuts_reset_camera_without_interpolation_through_removed_media() {
    let mut doc = fixture_project();
    doc.clips = vec![Clip { id: "clip-0".into(), in_us: 0, out_us: 2_000_000 },
        Clip { id: "right".into(), in_us: 3_000_000, out_us: 4_000_000 }];
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, 2.0), ZoomElement {
        clip_id: "right".into(), ..zoom("b", 3_000_000, 3_500_000, 3.0) }];
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    near(pose(&plan, 1_999_999).scale, 2.0);
    center(pose(&plan, 2_000_000));
    near(pose(&plan, 2_090_000).scale, 2.0);
    assert!(plan.camera.iter().all(|i| !(i.start_us < 2_000_000 && i.end_us > 2_000_000)));
    near(plan.camera.iter().find(|i| i.end_us == 2_000_000).unwrap().to.scale, 2.0);
}

#[test]
fn zero_envelopes_are_instant_without_zero_length_intervals() {
    let mut doc = fixture_project();
    doc.camera.pan_transition_us = 0;
    doc.zooms = vec![ZoomElement { intro_us: 0, hold_us: 0, outro_us: 0, ..zoom("a", 1_000_000, 2_000_000, 2.0) },
        ZoomElement { hold_us: 0, outro_us: 0, ..zoom("b", 2_000_000, 3_000_000, 3.0) }];
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    center(pose(&plan, 999_999));
    near(pose(&plan, 1_000_000).scale, 2.0);
    near(pose(&plan, 2_000_000).scale, 3.0);
    center(pose(&plan, 3_000_000));
    assert!(plan.camera.iter().all(|i| i.start_us < i.end_us));
}

#[test]
fn deterministic_sorted_plan_pins_frame_boundary_camera_poses_and_presets() {
    let mut doc = fixture_project();
    doc.revision = 9;
    doc.zooms = vec![ZoomElement { intro_us: 200_000, ..zoom("a", 1_000_000, 2_000_000, 2.0) }];
    for (preset, crf, fps) in [(ExportPreset::SmallFile, 28, 30), (ExportPreset::Balanced, 25, 30), (ExportPreset::HighQuality, 22, 60)] {
        assert_eq!(preset.crf(), crf);
        assert_eq!(preset.fps(), fps);
        let plan = render::compile(&doc, preset).unwrap();
        assert_eq!(plan, render::compile(&doc, preset).unwrap());
        assert_eq!((plan.project_id.as_str(), plan.revision, plan.duration_us, plan.preset), ("fixture", 9, 5_000_000, preset));
        assert_eq!(plan.clips, edits::map_clips(&doc).unwrap());
        assert_eq!(plan.camera.first().unwrap().start_us, 0);
        assert_eq!(plan.camera.last().unwrap().end_us, plan.duration_us);
        assert!(plan.camera.iter().all(|i| i.start_us < i.end_us));
        assert!(plan.camera.windows(2).all(|w| w[0].end_us == w[1].start_us));
        // 1.1s and 1.2s land on frames at both 30fps and 60fps.
        near(pose(&plan, 1_100_000).scale, 1.5);
        near(pose(&plan, 1_200_000).scale, 2.0);
    }
}

#[test]
fn compiler_rejects_invalid_documents() {
    let mut doc = fixture_project();
    doc.zooms = vec![zoom("a", 1_000_000, 2_000_000, f64::NAN)];
    assert!(render::compile(&doc, ExportPreset::Balanced).is_err());
    doc.zooms.clear();
    doc.clips[0].out_us = u64::MAX;
    assert!(render::compile(&doc, ExportPreset::Balanced).is_err());
}

#[test]
fn clip_eof_preserves_evaluated_partial_outro_without_forced_center() {
    let mut doc = fixture_project();
    doc.zooms = vec![zoom("a", 4_000_000, 4_800_000, 2.0)];
    let plan = render::compile(&doc, ExportPreset::Balanced).unwrap();
    let expected = 2.0 - (0.5 - 0.5 * (std::f64::consts::PI * 50.0 / 220.0).cos());
    near(pose(&plan, 5_000_000).scale, expected);
    assert!(plan.camera.iter().all(|i| i.end_us <= 5_000_000));
    doc.zooms.clear();
    let empty = render::compile(&doc, ExportPreset::Balanced).unwrap();
    assert_eq!(empty.camera.len(), 1);
    center(pose(&empty, 0));
    center(pose(&empty, 5_000_000));
}

#[test]
fn microsecond_targets_reach_own_scale_and_do_not_create_tiny_interval_gaps() {
    let mut doc = fixture_project();
    doc.zooms = vec![zoom("a", 1_000_001, 1_000_002, 2.0), zoom("b", 1_000_002, 1_000_003, 3.0)];
    let plan = render::compile(&doc, ExportPreset::HighQuality).unwrap();
    near(pose(&plan, 1_000_001).scale, 1.0);
    near(pose(&plan, 1_000_002).scale, 2.0);
    near(pose(&plan, 1_000_003).scale, 3.0);
    assert!(plan.camera.windows(2).all(|w| w[0].end_us == w[1].start_us));
    assert!(plan.camera.iter().all(|i| i.start_us < i.end_us));
    assert_eq!(plan.camera.iter().find(|i| i.start_us == 1_000_001).unwrap().end_us, 1_000_002);
}


fn repeated_click_project(clips: usize, clicks: usize) -> ProjectDocument {
    let mut doc = fixture_project();
    doc.clips = (0..clips).map(|i| Clip { id: format!("repeat-{i}"), in_us: 0, out_us: 1_000_000 }).collect();
    doc.clicks = (0..clicks).map(|i| Click { at_us: (i % 1_000_000) as u64, x: 0.25, y: 0.75 }).collect();
    doc
}

#[test]
fn derived_click_budget_rejects_fanout_without_truncating_boundary() {
    let boundary = repeated_click_project(64, 128);
    let plan = render::compile(&boundary, ExportPreset::Balanced).unwrap();
    assert_eq!(plan.clicks.len(), 8192);
    let error = render::compile(&repeated_click_project(64, 129), ExportPreset::Balanced).unwrap_err();
    assert_eq!(error.code, focusflow_core::ErrorCode::InvalidDocument);
    assert_eq!(error.field.as_deref(), Some("render.clicks"));
    assert!(error.message.contains("8192"));
    // The 4096 x 100000 attack is ~3 MiB on disk but would expand to 409.6 million clicks.
    let attack = repeated_click_project(4096, 100_000);
    assert!(serde_json::to_vec(&attack).unwrap().len() < MAX_JSON_BYTES as usize);
    assert_eq!(render::compile(&attack, ExportPreset::Balanced).unwrap_err().field.as_deref(), Some("render.clicks"));
}

#[test]
fn render_source_click_index_is_bounded_for_in_memory_callers() {
    let mut doc = fixture_project();
    // All clicks are trimmed away: the source index itself still needs a bound.
    doc.clips[0].in_us = 1_000_000;
    doc.clicks = vec![Click { at_us: 0, x: 0.0, y: 0.0 }; MAX_JSON_BYTES as usize / 20 + 1];
    assert_eq!(render::compile(&doc, ExportPreset::Balanced).unwrap_err().field.as_deref(), Some("render.sourceClicks"));
}

#[test]
fn render_graph_and_camera_estimates_reject_before_expansion() {
    let mut doc = fixture_project();
    // 1 clip + (7 * 1170 + 1) estimated intervals fits 32 MiB; one more does not.
    doc.zooms = (0..1170).map(|i| zoom(&format!("z-{i}"), i * 1000, i * 1000 + 500, 2.0)).collect();
    assert!(render::compile(&doc, ExportPreset::Balanced).is_ok());
    doc.zooms.push(zoom("over", 1_170_000, 1_170_500, 2.0));
    assert_eq!(render::compile(&doc, ExportPreset::Balanced).unwrap_err().field.as_deref(), Some("render.graphBytes"));
    doc.zooms = (0..9363).map(|i| zoom(&format!("z-{i}"), i * 400, i * 400 + 200, 2.0)).collect();
    assert_eq!(render::compile(&doc, ExportPreset::Balanced).unwrap_err().field.as_deref(), Some("render.camera"));
}

#[test]
fn zoom_mode_follow_compile_has_a_symbolic_plan() {
    let mut doc = fixture_project(); doc.schema_version = 4;
    doc.video.cursor = CursorMode::Separate;
    doc.cursor = Some(CursorAsset {index_path:"cursor/index.json".into()});
    doc.zooms = vec![zoom("follow", 1_000_000, 2_000_000, 2.0)];
    doc.zooms[0].mode = ZoomMode::Follow;
    validate(&doc).unwrap();
    assert!(render::compile(&doc, ExportPreset::Balanced).is_ok(), "follow must compile through the shared camera schedule");
}
