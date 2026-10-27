#[allow(dead_code)]
mod common;
use common::fixture_project;
use focusflow_core::{edits::{self, Edit, MappedClip}, project::*};

fn zoom(id: &str, clip_id: &str, start_us: u64, end_us: u64) -> ZoomElement {
    ZoomElement { mode: focusflow_core::project::ZoomMode::Fixed, id: id.into(), clip_id: clip_id.into(), start_us, end_us,
        x: 0.25, y: 0.75, scale: 2.0, intro_us: 200_000, hold_us: 300_000, outro_us: 400_000 }
}

#[test]
fn trim_split_remove_share_half_open_mapping() {
    let mut original = fixture_project();
    original.revision = 7;
    original.clicks = [0, 999_999, 1_000_000, 2_000_000, 2_999_999, 3_000_000]
        .into_iter().map(|at_us| Click { at_us, x: 0.5, y: 0.5 }).collect();
    let result = edits::apply(&original, &[
        Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1_000_000, right_clip_id: "middle".into() },
        Edit::SplitClip { clip_id: "middle".into(), at_us: 2_000_000, right_clip_id: "right".into() },
        Edit::RemoveClip { clip_id: "middle".into() },
        Edit::TrimClip { clip_id: "right".into(), in_us: 2_000_000, out_us: 3_000_000 },
    ]).unwrap();
    assert_eq!(result.clips, vec![
        Clip { id: "clip-0".into(), in_us: 0, out_us: 1_000_000 },
        Clip { id: "right".into(), in_us: 2_000_000, out_us: 3_000_000 },
    ]);
    let mapped = edits::map_clips(&result).unwrap();
    assert_eq!(mapped, vec![
        MappedClip { clip_id: "clip-0".into(), source_start_us: 0, source_end_us: 1_000_000, output_start_us: 0 },
        MappedClip { clip_id: "right".into(), source_start_us: 2_000_000, source_end_us: 3_000_000, output_start_us: 1_000_000 },
    ]);
    assert_eq!(mapped.last().map(|c| c.output_start_us + c.source_end_us - c.source_start_us), Some(2_000_000));
    let mapped_events: Vec<_> = mapped.iter().flat_map(|clip| result.clicks.iter()
        .filter(|click| (clip.source_start_us..clip.source_end_us).contains(&click.at_us))
        .map(move |click| (clip.clip_id.as_str(), clip.output_start_us + click.at_us - clip.source_start_us))).collect();
    assert_eq!(mapped_events, vec![("clip-0", 0), ("clip-0", 999_999), ("right", 1_000_000), ("right", 1_999_999)]);
    assert_eq!(result.video, original.video);
    assert_eq!(result.clicks, original.clicks);
    assert_eq!(result.revision, 7);
    assert_eq!(original.clips.len(), 1);
}

#[test]
fn split_retains_stable_ids_and_effect_intersections() {
    let mut original = fixture_project();
    original.zooms = vec![zoom("left", "clip-0", 100_000, 1_000_000),
        zoom("cross", "clip-0", 1_000_000, 3_000_000), zoom("right", "clip-0", 3_000_000, 4_000_000)];
    original.clicks = vec![Click { at_us: 2_000_000, x: 0.2, y: 0.3 }];
    let result = edits::apply(&original, &[Edit::SplitClip { clip_id: "clip-0".into(), at_us: 2_000_000, right_clip_id: "clip-1".into() }]).unwrap();
    assert_eq!(result.clips[0], Clip { id: "clip-0".into(), in_us: 0, out_us: 2_000_000 });
    assert_eq!(result.clips[1], Clip { id: "clip-1".into(), in_us: 2_000_000, out_us: 5_000_000 });
    assert_eq!(result.zooms, vec![zoom("left", "clip-0", 100_000, 1_000_000),
        zoom("cross", "clip-0", 1_000_000, 2_000_000), zoom("cross__clip-1", "clip-1", 2_000_000, 3_000_000),
        zoom("right", "clip-1", 3_000_000, 4_000_000)]);
    let mapped = edits::map_clips(&result).unwrap();
    let containing: Vec<_> = mapped.iter().filter(|c| c.source_start_us <= 2_000_000 && 2_000_000 < c.source_end_us).collect();
    assert_eq!(containing.len(), 1);
    assert_eq!(containing[0].clip_id, "clip-1");
    let trimmed = edits::apply(&result, &[
        Edit::TrimClip { clip_id: "clip-1".into(), in_us: 2_500_000, out_us: 3_000_000 },
        Edit::RemoveClip { clip_id: "clip-0".into() },
    ]).unwrap();
    assert_eq!(trimmed.zooms, vec![zoom("cross__clip-1", "clip-1", 2_500_000, 3_000_000)]);
    assert_eq!(trimmed.clicks, original.clicks);
}

#[test]
fn batch_is_all_or_nothing() {
    let original = fixture_project();
    let before = serde_json::to_vec(&original).unwrap();
    for invalid in [
        Edit::RemoveClip { clip_id: "clip-0".into() },
        Edit::RemoveClip { clip_id: "missing".into() },
        Edit::RemoveZoom { zoom_id: "missing".into() },
        Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1_000_000, right_clip_id: "clip-0".into() },
        Edit::SplitClip { clip_id: "clip-0".into(), at_us: 0, right_clip_id: "right".into() },
        Edit::TrimClip { clip_id: "clip-0".into(), in_us: 2_000_000, out_us: 1_000_000 },
        Edit::TrimClip { clip_id: "clip-0".into(), in_us: 0, out_us: u64::MAX },
        Edit::PutZoom { zoom: ZoomElement { scale: f64::INFINITY, ..zoom("bad", "clip-0", 1, 2) } },
        Edit::PutZoom { zoom: ZoomElement { x: f64::NAN, ..zoom("bad", "clip-0", 1, 2) } },
        Edit::PutZoom { zoom: ZoomElement { hold_us: u64::MAX, ..zoom("bad", "clip-0", 1, 2) } },
    ] {
        assert!(edits::apply(&original, &[
            Edit::TrimClip { clip_id: "clip-0".into(), in_us: 0, out_us: 4_000_000 }, invalid.clone()
        ]).is_err(), "{invalid:?}");
        assert_eq!(serde_json::to_vec(&original).unwrap(), before);
    }
    assert!(edits::apply(&original, &[]).is_err());
    assert!(edits::apply(&original, &vec![Edit::TrimClip { clip_id: "clip-0".into(), in_us: 0, out_us: 4_000_000 }; 257]).is_err());
    assert!(edits::apply(&original, &vec![Edit::TrimClip { clip_id: "clip-0".into(), in_us: 0, out_us: 4_000_000 }; 256]).is_ok());
}

#[test]
fn put_zoom_upserts_and_remove_zoom_leaves_source_events() {
    let mut original = fixture_project();
    original.clicks = vec![Click { at_us: 2, x: 0.5, y: 0.5 }];
    let first = zoom("target", "clip-0", 1, 2);
    let replacement = zoom("target", "clip-0", 3, 4);
    let result = edits::apply(&original, &[
        Edit::PutZoom { zoom: first }, Edit::PutZoom { zoom: replacement.clone() },
    ]).unwrap();
    assert_eq!(result.zooms, vec![replacement]);
    let removed = edits::apply(&result, &[Edit::RemoveZoom { zoom_id: "target".into() }]).unwrap();
    assert!(removed.zooms.is_empty());
    assert_eq!(removed.clicks, original.clicks);
    assert_eq!(removed.clips, original.clips);
}

#[test]
fn split_derived_zoom_collisions_and_long_ids_reject_the_batch() {
    for zoom_id in ["cross".to_owned(), "z".repeat(120)] {
        let mut original = fixture_project();
        original.zooms = vec![zoom(&zoom_id, "clip-0", 1_000_000, 3_000_000)];
        if zoom_id == "cross" {
            original.zooms.push(zoom("cross__right-clip", "clip-0", 3_000_000, 4_000_000));
        }
        assert!(edits::apply(&original, &[Edit::SplitClip {
            clip_id: "clip-0".into(), at_us: 2_000_000, right_clip_id: "right-clip".into(),
        }]).is_err());
        assert_eq!(original.clips, fixture_project().clips);
        assert_eq!(original.zooms[0].end_us, 3_000_000);
        assert_eq!(original.revision, 0);
    }
}

#[test]
fn mapping_respects_playback_order_and_repeated_source_ranges() {
    let mut doc = fixture_project();
    doc.clips = vec![Clip { id: "later".into(), in_us: 2_000_000, out_us: 3_000_000 },
        Clip { id: "earlier".into(), in_us: 0, out_us: 1_000_000 },
        Clip { id: "repeat".into(), in_us: 2_000_000, out_us: 3_000_000 }];
    assert_eq!(edits::map_clips(&doc).unwrap(), vec![
        MappedClip { clip_id: "later".into(), source_start_us: 2_000_000, source_end_us: 3_000_000, output_start_us: 0 },
        MappedClip { clip_id: "earlier".into(), source_start_us: 0, source_end_us: 1_000_000, output_start_us: 1_000_000 },
        MappedClip { clip_id: "repeat".into(), source_start_us: 2_000_000, source_end_us: 3_000_000, output_start_us: 2_000_000 },
    ]);
    doc.clips[0].in_us = u64::MAX;
    assert!(edits::map_clips(&doc).is_err());
    assert!(edits::apply(&doc, &[Edit::RemoveClip { clip_id: "later".into() }]).is_err());
    doc.video.duration_us = MAX_DURATION_US;
    doc.clips = vec![Clip { id: "full".into(), in_us: 0, out_us: MAX_DURATION_US },
        Clip { id: "excess".into(), in_us: 0, out_us: 1 }];
    assert!(edits::map_clips(&doc).is_err());
}

#[test]
fn rejects_unknown_ids_and_invalid_split_or_trim_bounds() {
    let doc = fixture_project();
    for edit in [
        Edit::TrimClip { clip_id: "missing".into(), in_us: 0, out_us: 1 },
        Edit::SplitClip { clip_id: "missing".into(), at_us: 1, right_clip_id: "new".into() },
        Edit::SplitClip { clip_id: "clip-0".into(), at_us: 5_000_000, right_clip_id: "new".into() },
        Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1, right_clip_id: "../bad".into() },
        Edit::TrimClip { clip_id: "clip-0".into(), in_us: 1, out_us: 1 },
        Edit::PutZoom { zoom: zoom("unknown-clip", "missing", 1, 2) },
        Edit::PutZoom { zoom: zoom("../bad", "clip-0", 1, 2) },
        Edit::PutZoom { zoom: zoom("empty", "clip-0", 2, 2) },
        Edit::PutZoom { zoom: zoom("outside", "clip-0", 1, 5_000_001) },
        Edit::PutZoom { zoom: ZoomElement { y: 1.1, ..zoom("bad", "clip-0", 1, 2) } },
        Edit::PutZoom { zoom: ZoomElement { scale: 8.1, ..zoom("bad", "clip-0", 1, 2) } },
        Edit::PutZoom { zoom: ZoomElement { scale: 0.9, ..zoom("bad", "clip-0", 1, 2) } },
    ] {
        assert!(edits::apply(&doc, &[edit.clone()]).is_err(), "{edit:?}");
    }
}

#[test]
fn edit_json_is_camel_case_and_rejects_unsupported_operations() {
    let edit = Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1, right_clip_id: "right".into() };
    let json = serde_json::json!({"type":"splitClip", "clipId":"clip-0", "atUs":1, "rightClipId":"right"});
    assert_eq!(serde_json::to_value(&edit).unwrap(), json);
    assert_eq!(serde_json::from_value::<Edit>(json).unwrap(), edit);
    for bad in [
        serde_json::json!({"type":"splitClip","clip_id":"clip-0","atUs":1,"rightClipId":"r"}),
        serde_json::json!({"type":"trimClip","clipId":"clip-0","inUs":0,"outUs":1,"style":"smooth"}),
        serde_json::json!({"type":"removeClip","clipId":"clip-0","follow":true}),
        serde_json::json!({"type":"followCursor","clipId":"clip-0"}),
        serde_json::json!({"type":"setStyle","style":"smooth"}),
        serde_json::json!({"type":"trimClip","clipId":"clip-0","inUs":-1,"outUs":1}),
        serde_json::json!({"type":"splitClip","clipId":"clip-0","atUs":1.5,"rightClipId":"r"}),
        serde_json::json!({"type":"removeZoom"}),
        serde_json::json!({"type":"putZoom","zoom":{"id":"z","clipId":"clip-0","startUs":1,"endUs":2,"x":0.5,"y":0.5,"scale":2,"introUs":1,"holdUs":1,"outroUs":1,"style":"new"}}),
    ] {
        assert!(serde_json::from_value::<Edit>(bad.clone()).is_err(), "{bad}");
    }
    assert!(serde_json::from_str::<Edit>(r#"{"type":"trimClip","clipId":"clip-0","inUs":0,"inUs":1,"outUs":2}"#).is_err());
}

#[test]
fn trim_reextension_preserves_source_but_does_not_resurrect_zooms() {
    let mut original = fixture_project();
    original.zooms = vec![zoom("target", "clip-0", 1_000_000, 4_000_000), zoom("lost", "clip-0", 100_000, 200_000)];
    let result = edits::apply(&original, &[
        Edit::TrimClip { clip_id: "clip-0".into(), in_us: 2_000_000, out_us: 3_000_000 },
        Edit::TrimClip { clip_id: "clip-0".into(), in_us: 0, out_us: 5_000_000 },
    ]).unwrap();
    assert_eq!(result.clips, original.clips);
    assert_eq!(result.zooms, vec![zoom("target", "clip-0", 2_000_000, 3_000_000)]);
}

#[test]
fn invalid_put_zoom_cannot_be_hidden_by_a_later_removal() {
    let original = fixture_project();
    for bad in [zoom("bad", "missing", 1, 2), zoom("bad", "clip-0", 2, 1),
        ZoomElement { scale: f64::NAN, ..zoom("bad", "clip-0", 1, 2) }] {
        assert!(edits::apply(&original, &[
            Edit::PutZoom { zoom: bad }, Edit::RemoveZoom { zoom_id: "bad".into() },
        ]).is_err());
    }
}

#[test]
fn active_zoom_overlap_is_rejected_but_envelope_overlap_and_adjacency_are_allowed() {
    let mut original = fixture_project();
    original.zooms = vec![zoom("first", "clip-0", 1_000_000, 2_000_000)];
    let adjacent = edits::apply(&original, &[Edit::PutZoom { zoom: zoom("second", "clip-0", 2_000_000, 3_000_000) }]).unwrap();
    assert_eq!(adjacent.zooms[0].hold_us, 300_000);
    assert_eq!(adjacent.zooms[0].outro_us, 400_000);
    for (start, end) in [(1_999_999, 3_000_000), (1_500_000, 1_600_000), (500_000, 3_000_000)] {
        let mut invalid = original.clone();
        invalid.zooms.insert(0, zoom("overlap", "clip-0", start, end));
        assert!(validate(&invalid).is_err(), "direct legacyUnknown project overlap must fail");
        assert!(edits::apply(&original, &[Edit::PutZoom { zoom: zoom("overlap", "clip-0", start, end) }]).is_err());
    }
    let mut separate = original.clone();
    separate.clips.push(Clip { id: "repeat".into(), in_us: 0, out_us: 5_000_000 });
    separate.zooms.push(zoom("repeated", "repeat", 1_000_000, 2_000_000));
    validate(&separate).unwrap();
}

#[test]
fn edits_enforce_clip_zoom_and_output_limits_without_partial_changes() {
    let mut clips = fixture_project();
    clips.clips = (0..4096).map(|i| Clip { id: format!("clip-{i}"), in_us: 0, out_us: 2 }).collect();
    assert!(edits::apply(&clips, &[Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1, right_clip_id: "extra".into() }]).is_err());
    assert_eq!(clips.clips.len(), 4096);

    let mut zooms = fixture_project();
    zooms.zooms = (0..16384).map(|i| zoom(&format!("zoom-{i}"), "clip-0", i * 2, i * 2 + 2)).collect();
    assert!(edits::apply(&zooms, &[Edit::SplitClip { clip_id: "clip-0".into(), at_us: 1, right_clip_id: "right".into() }]).is_err());
    assert!(edits::apply(&zooms, &[Edit::PutZoom { zoom: zoom("extra", "clip-0", 40_000, 40_001) }]).is_err());
    assert!(edits::apply(&zooms, &[Edit::PutZoom { zoom: zoom("zoom-0", "clip-0", 0, 2) }]).is_ok());
    assert_eq!(zooms.zooms.len(), 16384);

    let mut duration = fixture_project();
    duration.video.duration_us = MAX_DURATION_US;
    duration.clips = vec![Clip { id: "clip-0".into(), in_us: 0, out_us: 1 }, Clip { id: "clip-1".into(), in_us: 1, out_us: 2 }];
    assert!(edits::apply(&duration, &[Edit::TrimClip { clip_id: "clip-0".into(), in_us: 0, out_us: MAX_DURATION_US }]).is_err());
    assert_eq!(duration.clips[0].out_us, 1);
}

#[test]
fn zoom_mode_omitted_edit_and_history_inputs_default_to_fixed_only() {
    let old_zoom = common::fixed_zoom_wire();
    let edit: Edit = serde_json::from_value(serde_json::json!({"type":"putZoom","zoom":old_zoom})).unwrap();
    let result = edits::apply(&fixture_project(), &[edit]).unwrap();
    assert_eq!(result.schema_version, 4);
    assert_eq!(result.revision, 0);
    let value = serde_json::to_value(&result).unwrap();
    assert_eq!(value["zooms"][0]["mode"], "fixed");
    let mut state = serde_json::to_value(EditableState::from_document(&result)).unwrap();
    state["zooms"][0].as_object_mut().unwrap().remove("mode");
    let restored: EditableState = serde_json::from_value(state.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap()["zooms"][0]["mode"], "fixed");
    for mode in [serde_json::json!("unknown"), serde_json::Value::Null] {
        let mut wire = old_zoom.clone(); wire["mode"] = mode.clone();
        assert!(serde_json::from_value::<Edit>(serde_json::json!({"type":"putZoom","zoom":wire})).is_err());
        state["zooms"][0]["mode"] = mode;
        assert!(serde_json::from_value::<EditableState>(state.clone()).is_err());
    }
}

#[test]
fn zoom_mode_follow_split_trim_and_remove_retain_mode_and_intervals() {
    let mut wire = common::project_wire(4);
    wire["video"]["cursor"] = serde_json::json!("separate");
    wire["cursor"] = serde_json::json!({"indexPath":"cursor/index.json"});
    wire["zooms"][0]["mode"] = serde_json::json!("follow");
    let original: ProjectDocument = serde_json::from_value(wire).unwrap();
    let split = edits::apply(&original, &[Edit::SplitClip {clip_id:"clip-0".into(), at_us:2_000_000, right_clip_id:"right".into()}]).unwrap();
    let value = serde_json::to_value(&split).unwrap();
    assert_eq!(value["zooms"][0]["mode"], "follow");
    assert_eq!(value["zooms"][1]["mode"], "follow");
    assert_eq!((split.zooms[0].start_us, split.zooms[0].end_us), (1_000_000, 2_000_000));
    assert_eq!((split.zooms[1].start_us, split.zooms[1].end_us), (2_000_000, 3_000_000));
    assert_eq!(split.zooms[1].id, "zoom-0__right");
    let trimmed = edits::apply(&split, &[Edit::TrimClip {clip_id:"right".into(), in_us:2_500_000, out_us:4_000_000}, Edit::RemoveClip {clip_id:"clip-0".into()}]).unwrap();
    assert_eq!(trimmed.zooms.len(), 1);
    assert_eq!((trimmed.zooms[0].start_us, trimmed.zooms[0].end_us), (2_500_000, 3_000_000));
    assert_eq!(serde_json::to_value(&trimmed).unwrap()["zooms"][0]["mode"], "follow");
    let removed = edits::apply(&trimmed, &[Edit::RemoveZoom {zoom_id:"zoom-0__right".into()}]).unwrap();
    assert!(removed.zooms.is_empty());
    assert_eq!(original.zooms[0].end_us, 3_000_000);
}

#[test]
fn zoom_mode_invalid_follow_put_cannot_be_hidden_by_later_removal() {
    let mut z = zoom("follow", "clip-0", 1_000_000, 2_000_000); z.mode = ZoomMode::Follow;
    let original = fixture_project();
    assert!(edits::apply(&original, &[Edit::PutZoom {zoom:z}, Edit::RemoveZoom {zoom_id:"follow".into()}]).is_err());
    assert_eq!(original.schema_version, 1);
    assert!(original.zooms.is_empty());
}
