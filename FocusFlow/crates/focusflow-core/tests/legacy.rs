mod common;
use common::{fixture_project, write_legacy, TestDir};
use focusflow_core::{audio::is_usable_wav, ffmpeg::{probe_video, ProbedVideo}, legacy::{convert_interactions, read_legacy, seconds_to_us, validate_defaults, LegacyDefaults}, project::*, ErrorCode};
use std::{collections::BTreeMap, hash::{Hash, Hasher}, path::Path};

fn defaults() -> LegacyDefaults { LegacyDefaults { scale: 2.0, intro_us: 200_000, hold_us: 300_000, outro_us: 200_000, pan_transition_us: 200_000, settle_us: 100_000 } }
fn video() -> ProbedVideo { ProbedVideo { width: 100, height: 50, duration_us: 5_000_000 } }
fn import(dir: &Path) -> focusflow_core::CoreResult<ProjectDocument> { read_legacy(dir, "project-1", video(), defaults()) }
fn hashes(dir: &Path) -> BTreeMap<String, u64> {
    std::fs::read_dir(dir).unwrap().map(|e| { let e=e.unwrap(); let mut h=std::collections::hash_map::DefaultHasher::new(); std::fs::read(e.path()).unwrap().hash(&mut h); (e.file_name().to_string_lossy().into_owned(), h.finish()) }).collect()
}
fn write_json(dir: &Path, name: &str, value: serde_json::Value) { std::fs::write(dir.join(name), serde_json::to_vec(&value).unwrap()).unwrap(); }

#[test]
fn legacy_import_retains_bytes_and_timing_unknown() {
    let dir=TestDir::new(); write_legacy(dir.path()); let before=hashes(dir.path());
    let doc=import(dir.path()).expect("valid legacy session imports");
    assert_eq!(hashes(dir.path()), before);
    assert_eq!(doc.video.duration_us, 5_000_000); // session.json zero is not media duration.
    assert_eq!(doc.video.path,"screen.mp4"); assert_eq!(doc.schema_version,3); assert_eq!(doc.revision,0);
    assert_eq!(doc.timing.provenance,TimingProvenance::LegacyUnknown); assert_eq!(doc.timing.audio_offset_us,None);
    assert!(doc.warnings.contains(&ProjectWarning::LegacyTimingUnknown)); assert!(doc.warnings.contains(&ProjectWarning::EmbeddedCursor));
    assert!(doc.warnings.contains(&ProjectWarning::LegacyScaleOverridden));
    assert!(!doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio));
    assert_eq!(doc.clips[0].id,"clip-0"); assert_eq!(doc.zooms[0].id,"zoom-0"); assert_eq!(doc.zooms[0].clip_id,"clip-0");
    assert_eq!(doc.zooms[0].mode,ZoomMode::Fixed);
    assert_eq!(doc.zooms[0].scale,2.0); assert_eq!(doc.zooms[0].start_us,1_000_000); assert_eq!(doc.zooms[0].end_us,2_000_000);
    assert_eq!(doc.zooms[0].hold_us,300_000); assert_eq!(doc.camera.settle_us,100_000);
    validate(&doc).unwrap();
    let json=serde_json::to_value(&doc).unwrap(); assert_eq!(json["schemaVersion"],3); assert_eq!(json["timing"]["provenance"],"legacyUnknown");
}
#[test]
fn normalizes_capture_coordinates_once() {
    let dir=TestDir::new(); write_legacy(dir.path()); let doc=import(dir.path()).unwrap();
    assert_eq!((doc.clicks[0].x,doc.clicks[0].y),(0.5,0.5)); assert_eq!((doc.zooms[0].x,doc.zooms[0].y),(0.5,0.5));
}
#[test]
fn rejects_bad_time_or_schema() {
    assert_eq!(seconds_to_us(0.0000014).unwrap(),1);
    for bad in [-0.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 86400.000001, u64::MAX as f64] { assert!(seconds_to_us(bad).is_err(),"{bad}"); }
    assert_eq!(seconds_to_us(86400.0).unwrap(),86_400_000_000);
    let good=fixture_project(); validate(&good).unwrap();
    let mut doc=good.clone(); doc.schema_version=5; assert_eq!(validate(&doc).unwrap_err().code,ErrorCode::UnsupportedSchema);
    let mut doc=good.clone(); doc.clips.clear(); assert_eq!(validate(&doc).unwrap_err().field.as_deref(),Some("clips"));
    let mut doc=good.clone(); doc.timing.audio_offset_us=Some(0); assert!(validate(&doc).is_err());
    let mut doc=good.clone(); doc.clips[0].out_us=6_000_000; assert_eq!(validate(&doc).unwrap_err().field.as_deref(),Some("clips[0].outUs"));
    let mut doc=good.clone(); doc.clips.push(doc.clips[0].clone()); assert!(validate(&doc).is_err());
    let mut doc=good.clone(); doc.video.path="../screen.mp4".into(); assert!(validate(&doc).is_err());
    let mut json=serde_json::to_value(&good).unwrap(); json["inventedHover"]=true.into(); assert!(serde_json::from_value::<ProjectDocument>(json).is_err());
}
#[test]
fn rejects_indexed_invalid_interactions_and_present_metadata() {
    let dir=TestDir::new(); write_legacy(dir.path());
    write_json(dir.path(),"clicks.json",serde_json::json!([{"timestamp":1,"x":50,"y":25,"button":"left"},{"timestamp":-1,"x":50,"y":25,"button":"left"}]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(),Some("clicks[1].timestamp"));
    write_json(dir.path(),"clicks.json",serde_json::json!([{"timestamp":1,"x":101,"y":25,"button":"left"}]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(),Some("clicks[0].x"));
    write_json(dir.path(),"clicks.json",serde_json::json!([{"timestamp":"bad","x":50,"y":25,"button":"left"}]));
    assert!(import(dir.path()).unwrap_err().field.unwrap().starts_with("clicks[0]"));
    write_legacy(dir.path()); write_json(dir.path(),"drags.json",serde_json::json!([{"start":1,"end":2,"button":"left","points":[{"timestamp":-1,"x":50,"y":25}]}]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(),Some("drags[0].points[0].timestamp"));
    write_legacy(dir.path()); write_json(dir.path(),"metadata.json",serde_json::json!({"version":2}));
    assert_eq!(import(dir.path()).unwrap_err().code,ErrorCode::UnsupportedSchema);
    write_json(dir.path(),"metadata.json",serde_json::json!({"version":1,"captureBounds":"bad"})); assert!(import(dir.path()).is_err());
}
#[test]
fn missing_older_metadata_and_empty_interactions_are_supported() {
    let dir=TestDir::new(); write_legacy(dir.path());
    std::fs::remove_file(dir.path().join("metadata.json")).unwrap(); std::fs::remove_file(dir.path().join("session.json")).unwrap(); std::fs::remove_file(dir.path().join("drags.json")).unwrap();
    write_json(dir.path(),"clicks.json",serde_json::json!([])); write_json(dir.path(),"timeline.json",serde_json::json!([]));
    let doc=import(dir.path()).unwrap(); assert!(doc.clicks.is_empty()); assert!(doc.zooms.is_empty()); assert!(doc.warnings.contains(&ProjectWarning::LegacyMetadataMissing));
}
#[test]
fn optional_audio_uses_real_wav_and_warns_only_when_invalid() {
    let dir=TestDir::new(); write_legacy(dir.path());
    let path=dir.path().join("mic.wav");
    let mut meta:serde_json::Value=serde_json::from_slice(&std::fs::read(dir.path().join("metadata.json")).unwrap()).unwrap(); meta["hasMicAudio"]=true.into(); write_json(dir.path(),"metadata.json",meta);
    for contents in [None,Some(Vec::new()),Some(b"corrupt".to_vec())] {
        if let Some(bytes)=contents { std::fs::write(&path,bytes).unwrap(); }
        assert!(!is_usable_wav(&path)); let doc=import(dir.path()).unwrap(); assert!(doc.microphone.is_none()); assert!(doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio));
    }
    let spec=hound::WavSpec { channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int };
    hound::WavWriter::create(&path,spec).unwrap().finalize().unwrap(); assert!(!is_usable_wav(&path));
    let mut writer=hound::WavWriter::create(&path,spec).unwrap(); for _ in 0..100 { writer.write_sample(100_i16).unwrap(); } writer.finalize().unwrap();
    assert!(is_usable_wav(&path)); let doc=import(dir.path()).unwrap(); assert_eq!(doc.microphone.unwrap().path,"mic.wav"); assert!(!doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio));
    let len=std::fs::metadata(&path).unwrap().len(); std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(len-10).unwrap(); assert!(!is_usable_wav(&path));
}
#[test]
fn bounded_required_records_fail_instead_of_being_dropped() {
    let dir=TestDir::new(); write_legacy(dir.path());
    write_json(dir.path(),"timeline.json",serde_json::json!([{"start":2,"end":1,"x":50,"y":25,"scale":3}]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(),Some("timeline[0].end"));
    let file=std::fs::File::create(dir.path().join("timeline.json")).unwrap(); file.set_len(MAX_JSON_BYTES+1).unwrap(); assert_eq!(import(dir.path()).unwrap_err().field.as_deref(),Some("timeline"));
    write_legacy(dir.path()); write_json(dir.path(),"session.json",serde_json::json!({"sessionId":"old","createdAt":"date","durationSeconds":0,"recordingSource":"screen","exported":false})); import(dir.path()).unwrap();
    write_json(dir.path(),"session.json",serde_json::json!({"sessionId":"old","durationSeconds":"bad"})); assert!(import(dir.path()).is_err());
}
#[test]
fn ffmpeg_probe_reports_media_facts_and_bad_input() {
    let dir=TestDir::new(); let path=dir.path().join("screen.mp4");
    let status=std::process::Command::new("ffmpeg").args(["-v","error","-f","lavfi","-i","color=c=black:s=100x50:r=30","-t","1.1","-c:v","libx264","-threads","1","-y"]).arg(&path).status().expect("ffmpeg installed for core media checks"); assert!(status.success());
    let info=probe_video(Path::new("ffmpeg"),&path).unwrap(); assert_eq!((info.width,info.height),(100,50)); assert_eq!(info.duration_us,1_100_000);
    std::fs::write(&path,b"not media").unwrap(); assert_eq!(probe_video(Path::new("ffmpeg"),&path).unwrap_err().code,ErrorCode::EncoderFailed);
}

#[test]
fn document_bounds_reject_invalid_geometry_and_checked_ranges() {
    let good=fixture_project();
    for (field, mutate) in [
        ("video.width", (|d:&mut ProjectDocument| d.video.width=0) as fn(&mut ProjectDocument)),
        ("video.height", |d:&mut ProjectDocument| d.video.height=16385),
        ("video.durationUs", |d:&mut ProjectDocument| d.video.duration_us=MAX_DURATION_US+1),
        ("revision", |d:&mut ProjectDocument| d.revision=MAX_REVISION+1),
        ("projectId", |d:&mut ProjectDocument| d.project_id="../escape".into()),
        ("camera.settleUs", |d:&mut ProjectDocument| d.camera.settle_us=u64::MAX),
        ("clips", |d:&mut ProjectDocument| d.clips=(0..4097).map(|i| Clip{id:format!("clip-{i}"),in_us:0,out_us:1}).collect()),
        ("zooms", |d:&mut ProjectDocument| d.zooms=vec![ZoomElement{mode:ZoomMode::Fixed,id:"z".into(),clip_id:"clip-0".into(),start_us:1,end_us:2,x:0.5,y:0.5,scale:2.0,intro_us:1,hold_us:1,outro_us:1};16385]),
    ] {
        let mut doc=good.clone(); mutate(&mut doc); assert_eq!(validate(&doc).unwrap_err().field.as_deref(),Some(field));
    }
    let mut doc=good.clone(); doc.clips=vec![Clip{id:"a".into(),in_us:0,out_us:5_000_000},Clip{id:"b".into(),in_us:0,out_us:5_000_000}]; validate(&doc).unwrap();
    let mut doc=good.clone(); doc.video.duration_us=MAX_DURATION_US; doc.clips=vec![Clip{id:"a".into(),in_us:0,out_us:MAX_DURATION_US},Clip{id:"b".into(),in_us:0,out_us:1}]; assert_eq!(validate(&doc).unwrap_err().field.as_deref(),Some("clips"));
    let mut doc=good.clone(); doc.zooms.push(ZoomElement{mode:ZoomMode::Fixed,id:"z".into(),clip_id:"clip-0".into(),start_us:1,end_us:2,x:0.5,y:0.5,scale:2.0,intro_us:1,hold_us:1,outro_us:1}); validate(&doc).unwrap();
    for (field, mutate) in [
        ("zooms[0].clipId", (|z:&mut ZoomElement| z.clip_id="missing".into()) as fn(&mut ZoomElement)),
        ("zooms[0].scale", |z:&mut ZoomElement| z.scale=f64::INFINITY),
        ("zooms[0].x", |z:&mut ZoomElement| z.x=f64::NAN),
        ("zooms[0].y", |z:&mut ZoomElement| z.y=-0.1),
        ("zooms[0].endUs", |z:&mut ZoomElement| z.end_us=6_000_000),
        ("zooms[0].introUs", |z:&mut ZoomElement| z.intro_us=u64::MAX),
    ] { let mut doc=doc.clone(); mutate(&mut doc.zooms[0]); assert_eq!(validate(&doc).unwrap_err().field.as_deref(),Some(field)); }
}

#[cfg(unix)]
#[test]
fn dangling_optional_records_are_not_silently_treated_as_absent() {
    let dir=TestDir::new(); write_legacy(dir.path());
    std::fs::remove_file(dir.path().join("metadata.json")).unwrap();
    std::os::unix::fs::symlink("missing",dir.path().join("metadata.json")).unwrap();
    assert!(import(dir.path()).is_err(),"a present dangling metadata link must fail");
    std::fs::remove_file(dir.path().join("metadata.json")).unwrap();
    std::os::unix::fs::symlink("missing",dir.path().join("mic.wav")).unwrap();
    assert!(import(dir.path()).unwrap().warnings.contains(&ProjectWarning::InvalidOptionalAudio));
}

#[test]
fn near_eof_click_and_drag_idle_tails_are_clipped_without_mutating_raw_files() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    write_json(dir.path(), "timeline.json", serde_json::json!([
        {"start":4.7,"end":5.7,"x":50,"y":25,"scale":2},
        {"start":4.9,"end":5.9,"x":60,"y":30,"scale":2}
    ]));
    write_json(dir.path(), "clicks.json", serde_json::json!([
        {"timestamp":4.8,"x":50,"y":25,"button":"left"}
    ]));
    write_json(dir.path(), "drags.json", serde_json::json!([
        {"start":4.9,"end":5.1,"button":"left","points":[
            {"timestamp":4.9,"x":50,"y":25},
            {"timestamp":5.1,"x":60,"y":30}
        ]}
    ]));
    let before = hashes(dir.path());
    let doc = import(dir.path()).expect("generated idle tails must remain readable");
    assert_eq!(hashes(dir.path()), before);
    assert_eq!(doc.zooms.len(), 2);
    assert!(doc.warnings.contains(&ProjectWarning::LegacyZoomOverlapNormalized));
    assert_eq!((doc.zooms[0].start_us, doc.zooms[0].end_us), (4_700_000, 4_900_000));
    assert_eq!((doc.zooms[1].start_us, doc.zooms[1].end_us), (4_900_000, 5_000_000));
    assert_eq!(doc.clicks[0].at_us, 4_800_000);
    assert_eq!(doc.warnings.iter().filter(|w| **w == ProjectWarning::LegacyInteractionClipped).count(), 1);
    assert_eq!(doc.timing.provenance, TimingProvenance::LegacyUnknown);
    assert_eq!(doc.timing.audio_offset_us, None);
    validate(&doc).unwrap();
    assert_eq!(serde_json::to_value(ProjectWarning::LegacyInteractionClipped).unwrap(), "legacyInteractionClipped");
}

#[test]
fn legacy_eof_boundary_is_half_open_and_outside_records_are_omitted() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    write_json(dir.path(), "timeline.json", serde_json::json!([
        {"start":4.0,"end":5.0,"x":50,"y":25,"scale":2},
        {"start":5.0,"end":5.5,"x":50,"y":25,"scale":2},
        {"start":5.2,"end":5.7,"x":50,"y":25,"scale":2}
    ]));
    write_json(dir.path(), "clicks.json", serde_json::json!([
        {"timestamp":4.999999,"x":50,"y":25,"button":"left"},
        {"timestamp":5.0,"x":50,"y":25,"button":"left"},
        {"timestamp":5.1,"x":50,"y":25,"button":"left"}
    ]));
    write_json(dir.path(), "drags.json", serde_json::json!([
        {"start":5.1,"end":5.2,"button":"left","points":[
            {"timestamp":5.1,"x":50,"y":25},
            {"timestamp":5.2,"x":60,"y":30}
        ]}
    ]));
    let before = hashes(dir.path());
    let doc = import(dir.path()).unwrap();
    assert_eq!(hashes(dir.path()), before);
    assert_eq!(doc.zooms.len(), 1);
    assert_eq!((doc.zooms[0].id.as_str(), doc.zooms[0].end_us), ("zoom-0", 5_000_000));
    assert_eq!(doc.clicks.len(), 1);
    assert_eq!(doc.clicks[0].at_us, 4_999_999);
    assert!(doc.warnings.contains(&ProjectWarning::LegacyInteractionClipped));
    validate(&doc).unwrap();
    let mut strict = doc.clone();
    strict.zooms[0].end_us = 5_000_001;
    assert_eq!(validate(&strict).unwrap_err().field.as_deref(), Some("zooms[0].endUs"));

    write_json(dir.path(), "timeline.json", serde_json::json!([
        {"start":4.0,"end":5.0,"x":50,"y":25,"scale":2}
    ]));
    write_json(dir.path(), "clicks.json", serde_json::json!([]));
    write_json(dir.path(), "drags.json", serde_json::json!([]));
    let exact_end = import(dir.path()).unwrap();
    assert!(!exact_end.warnings.contains(&ProjectWarning::LegacyInteractionClipped));
}

#[test]
fn legacy_clipping_does_not_hide_malformed_outside_records() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    for (segment, field) in [
        (serde_json::json!({"start":6,"end":5,"x":50,"y":25,"scale":2}), "timeline[0].end"),
        (serde_json::json!({"start":6,"end":7,"x":100,"y":25,"scale":2}), "timeline[0].x"),
        (serde_json::json!({"start":6,"end":86401,"x":50,"y":25,"scale":2}), "timeline[0].end"),
        (serde_json::json!({"start":-1,"end":7,"x":50,"y":25,"scale":2}), "timeline[0].start"),
    ] {
        write_json(dir.path(), "timeline.json", serde_json::json!([segment]));
        assert_eq!(import(dir.path()).unwrap_err().field.as_deref(), Some(field));
    }
    write_json(dir.path(), "timeline.json", serde_json::json!([]));
    write_json(dir.path(), "clicks.json", serde_json::json!([
        {"timestamp":6,"x":100,"y":25,"button":"left"}
    ]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(), Some("clicks[0].x"));
    write_json(dir.path(), "clicks.json", serde_json::json!([]));
    write_json(dir.path(), "drags.json", serde_json::json!([
        {"start":5.1,"end":5.4,"button":"left","points":[
            {"timestamp":5.3,"x":50,"y":25},
            {"timestamp":5.2,"x":60,"y":30}
        ]}
    ]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(), Some("drags[0].points[1].timestamp"));
}

#[test]
fn legacy_drag_order_is_validated_before_rounding() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    write_json(dir.path(), "drags.json", serde_json::json!([
        {"start":5.1,"end":5.4,"button":"left","points":[
            {"timestamp":5.3000004,"x":50,"y":25},
            {"timestamp":5.3000001,"x":60,"y":30}
        ]}
    ]));
    assert_eq!(import(dir.path()).unwrap_err().field.as_deref(), Some("drags[0].points[1].timestamp"));
}

#[test]
fn legacy_overlapping_targets_normalize_in_stable_start_order() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    write_json(dir.path(), "timeline.json", serde_json::json!([
        {"start":3.0,"end":4.0,"x":30,"y":15,"scale":2},
        {"start":1.0,"end":4.5,"x":10,"y":5,"scale":2},
        {"start":1.0,"end":2.5,"x":20,"y":10,"scale":2},
        {"start":2.0,"end":3.5,"x":40,"y":20,"scale":2}
    ]));
    let before = hashes(dir.path());
    let doc = import(dir.path()).unwrap();
    assert_eq!(hashes(dir.path()), before);
    assert_eq!(doc.zooms.iter().map(|z| (z.id.as_str(), z.start_us, z.end_us)).collect::<Vec<_>>(),
        vec![("zoom-2", 1_000_000, 2_000_000), ("zoom-3", 2_000_000, 3_000_000), ("zoom-0", 3_000_000, 4_500_000)]);
    assert_eq!(doc.zooms[0].x, 0.2);
    assert_eq!((doc.zooms[0].intro_us, doc.zooms[0].hold_us, doc.zooms[0].outro_us), (200_000, 300_000, 200_000));
    assert_eq!(doc.warnings.iter().filter(|w| **w == ProjectWarning::LegacyZoomOverlapNormalized).count(), 1);
    validate(&doc).unwrap();
}

#[test]
fn legacy_nested_overlap_carries_the_sequence_tail_to_the_later_target() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    write_json(dir.path(), "timeline.json", serde_json::json!([
        {"start":1.0,"end":5.0,"x":10,"y":5,"scale":2},
        {"start":2.0,"end":3.0,"x":20,"y":10,"scale":2}
    ]));
    let before = hashes(dir.path());
    let doc = import(dir.path()).unwrap();
    assert_eq!(hashes(dir.path()), before);
    assert_eq!(doc.zooms.iter().map(|z| (z.id.as_str(), z.start_us, z.end_us)).collect::<Vec<_>>(),
        vec![("zoom-0", 1_000_000, 2_000_000), ("zoom-1", 2_000_000, 5_000_000)]);
    assert_eq!(doc.zooms[1].x, 0.2);
    assert!(doc.warnings.contains(&ProjectWarning::LegacyZoomOverlapNormalized));
    validate(&doc).unwrap();
}

#[test]
fn unsupported_saved_codec_retains_source_and_leaves_project_absent() {
    let root=TestDir::new(); let dir=root.path().join("legacy"); std::fs::create_dir(&dir).unwrap();
    let source=dir.join("screen.mp4");
    let status=std::process::Command::new("ffmpeg").args(["-v","error","-f","lavfi","-i","color=c=black:s=100x50:r=30","-frames:v","3","-c:v","mpeg4","-y"]).arg(&source).status().unwrap();
    assert!(status.success()); let before=hashes(&dir);
    let error=focusflow_core::store::ProjectStore::new(root.path()).unwrap().open_or_import("legacy",Path::new("ffmpeg"),defaults()).unwrap_err();
    assert_eq!(error.code,ErrorCode::EncoderFailed);
    assert!(error.message.contains("H.264") && error.message.contains("retained"),"{}",error.message);
    assert!(!dir.join("project.json").exists()); assert_eq!(hashes(&dir),before);
}

#[test]
fn pure_interactions_match_import_with_frozen_custom_defaults_without_filesystem() {
    let dir = TestDir::new();
    write_legacy(dir.path());
    let timeline = serde_json::json!([
        {"start":1,"end":6,"x":10,"y":5,"scale":2},
        {"start":1,"end":2,"x":20,"y":10,"scale":2},
        {"start":2,"end":3,"x":40,"y":20,"scale":2}
    ]);
    let clicks = serde_json::json!([
        {"timestamp":4.9999994,"x":50,"y":25,"button":"left"},
        {"timestamp":4.9999996,"x":60,"y":30,"button":"right"}
    ]);
    write_json(dir.path(), "timeline.json", timeline.clone());
    write_json(dir.path(), "clicks.json", clicks.clone());
    std::fs::remove_file(dir.path().join("drags.json")).unwrap();
    let custom = LegacyDefaults { scale: 3.5, intro_us: 111_000, hold_us: 222_000,
        outro_us: 333_000, pan_transition_us: 444_000, settle_us: 555_000 };
    let before = hashes(dir.path());
    let doc = read_legacy(dir.path(), "project-1", video(), custom).unwrap();
    assert_eq!(hashes(dir.path()), before);
    assert_eq!((doc.camera.pan_transition_us, doc.camera.settle_us), (444_000, 555_000));
    assert_eq!(doc.zooms.iter().map(|z| (z.id.as_str(), z.start_us, z.end_us)).collect::<Vec<_>>(),
        vec![("zoom-1", 1_000_000, 2_000_000), ("zoom-2", 2_000_000, 5_000_000)]);
    assert_eq!((doc.zooms[0].scale, doc.zooms[0].intro_us, doc.zooms[0].hold_us, doc.zooms[0].outro_us),
        (3.5, 111_000, 222_000, 333_000));
    assert_eq!(doc.clicks.len(), 1);
    assert_eq!((doc.clicks[0].at_us, doc.clicks[0].x, doc.clicks[0].y), (4_999_999, 0.5, 0.5));
    let expected_warnings = vec![ProjectWarning::LegacyScaleOverridden,
        ProjectWarning::LegacyZoomOverlapNormalized, ProjectWarning::LegacyInteractionClipped];
    assert_eq!(&doc.warnings[2..], expected_warnings.as_slice());
    // Stale optional files cannot replace the caller's already corrected in-memory events.
    write_json(dir.path(), "timeline.json", serde_json::json!({"stale": true}));
    write_json(dir.path(), "drags.json", serde_json::json!({"stale": true}));
    let stale = hashes(dir.path());
    let pure = convert_interactions(video(), custom, timeline.clone(), clicks.clone(), None).unwrap();
    assert_eq!(pure.zooms, doc.zooms);
    assert_eq!(pure.clicks, doc.clicks);
    assert_eq!(pure.warnings, expected_warnings);
    assert_eq!(hashes(dir.path()), stale);
    drop(dir);
    let pure = convert_interactions(video(), custom, timeline, clicks, None).unwrap();
    assert_eq!(pure.zooms, doc.zooms);
    assert_eq!(pure.clicks, doc.clicks);
    assert_eq!(pure.warnings, expected_warnings);
}

#[test]
fn shared_defaults_validate_inclusive_scale_and_all_time_boundaries() {
    for scale in [1.0, 8.0] {
        validate_defaults(LegacyDefaults { scale, ..defaults() }).unwrap();
    }
    for scale in [0.999, 8.001, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(validate_defaults(LegacyDefaults { scale, ..defaults() }).unwrap_err().field.as_deref(), Some("defaults.scale"));
    }
    for (field, set) in [
        ("defaults.introUs", (|d: &mut LegacyDefaults, v| d.intro_us = v) as fn(&mut LegacyDefaults, u64)),
        ("defaults.holdUs", |d: &mut LegacyDefaults, v| d.hold_us = v),
        ("defaults.outroUs", |d: &mut LegacyDefaults, v| d.outro_us = v),
        ("defaults.panTransitionUs", |d: &mut LegacyDefaults, v| d.pan_transition_us = v),
        ("defaults.settleUs", |d: &mut LegacyDefaults, v| d.settle_us = v),
    ] {
        let mut value = defaults();
        for time in [0, MAX_DURATION_US] { set(&mut value, time); validate_defaults(value).unwrap(); }
        set(&mut value, MAX_DURATION_US + 1);
        assert_eq!(validate_defaults(value).unwrap_err().field.as_deref(), Some(field));
        assert_eq!(convert_interactions(video(), value, serde_json::json!([]), serde_json::json!([]), None).unwrap_err().field.as_deref(), Some(field));
    }
}

#[test]
fn pure_interactions_reject_malformed_arrays_and_unknown_fields() {
    use serde_json::json;
    for (timeline, clicks, drags, field) in [
        (json!({}), json!([]), None, "timeline"),
        (json!([]), json!(null), None, "clicks"),
        (json!([]), json!([]), Some(json!({})), "drags"),
        (json!([{"start":6,"end":7,"x":50,"y":25,"scale":2,"unknown":true}]), json!([]), None, "timeline[0]"),
        (json!([]), json!([{"timestamp":6,"x":50,"y":25,"button":"left","unknown":true}]), None, "clicks[0]"),
        (json!([]), json!([]), Some(json!([{"start":6,"end":7,"button":"left","points":[],"unknown":true}])), "drags[0]"),
        (json!([]), json!([]), Some(json!([{"start":6,"end":7,"button":"left","points":[{"timestamp":6,"x":50,"y":25,"unknown":true}]}])), "drags[0]"),
        (json!([{"start":6,"end":7,"x":50,"y":25,"scale":0}]), json!([]), None, "timeline[0].scale"),
        (json!([]), json!([{"timestamp":86401,"x":50,"y":25,"button":"left"}]), None, "clicks[0].timestamp"),
        (json!([]), json!([]), Some(json!([{"start":6,"end":7,"button":"left","points":[{"timestamp":6,"x":100,"y":25}]}])), "drags[0].points[0].x"),
        (json!([]), json!([]), Some(json!([{"start":7,"end":6,"button":"left","points":[]}])), "drags[0].end"),
    ] {
        let error = convert_interactions(video(), defaults(), timeline, clicks, drags).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidDocument);
        assert_eq!(error.field.as_deref(), Some(field));
    }
}

#[test]
fn pure_interactions_enforce_timeline_count_before_clipping_or_normalizing() {
    let segment = serde_json::json!({"start":6,"end":7,"x":50,"y":25,"scale":2});
    let mut timeline = vec![segment.clone(); 16384];
    let pure = convert_interactions(video(), defaults(), timeline.clone().into(), serde_json::json!([]), None).unwrap();
    assert!(pure.zooms.is_empty());
    assert_eq!(pure.warnings, vec![ProjectWarning::LegacyInteractionClipped]);
    timeline.push(segment);
    let error = convert_interactions(video(), defaults(), timeline.into(), serde_json::json!([]), None).unwrap_err();
    assert_eq!(error.field.as_deref(), Some("timeline"));
}

#[test]
fn zoom_mode_old_project_versions_remain_closed_and_fixed() {
    for version in 1..=3 {
        let original = common::project_wire(version);
        let doc: ProjectDocument = serde_json::from_value(original.clone()).unwrap();
        validate(&doc).unwrap();
        assert_eq!(serde_json::to_value(&doc).unwrap(), original);
        // Edit/history output has an explicit normalized mode, even for an old source file.
        assert_eq!(serde_json::to_value(EditableState::from_document(&doc)).unwrap()["zooms"][0]["mode"], "fixed");
        for mode in ["fixed", "follow"] {
            let mut wire = original.clone();
            wire["zooms"][0]["mode"] = serde_json::json!(mode);
            assert!(serde_json::from_value::<ProjectDocument>(wire).is_err(), "schema {version} accepted {mode}");
        }
    }
}

#[test]
fn zoom_mode_schema_four_requires_an_explicit_known_mode() {
    let original = common::project_wire(4);
    let doc: ProjectDocument = serde_json::from_value(original.clone()).expect("schema 4 fixed document");
    validate(&doc).unwrap();
    assert_eq!(serde_json::to_value(&doc).unwrap(), original);
    for mode in [None, Some(serde_json::json!("other")), Some(serde_json::Value::Null)] {
        let mut wire = original.clone();
        match mode { Some(mode) => {wire["zooms"][0]["mode"] = mode;}, None => {wire["zooms"][0].as_object_mut().unwrap().remove("mode");} }
        assert!(serde_json::from_value::<ProjectDocument>(wire).is_err());
    }
}

#[test]
fn zoom_mode_follow_domain_requires_current_schema_and_separate_cursor() {
    let mut wire = common::project_wire(4);
    wire["zooms"][0]["mode"] = serde_json::json!("follow");
    let follow: ProjectDocument = serde_json::from_value(wire).unwrap();
    for cursor in [CursorMode::Embedded, CursorMode::Absent] {
        let mut doc = follow.clone(); doc.video.cursor = cursor;
        let error = validate(&doc).expect_err("follow needs separate cursor provenance");
        assert_eq!(error.field.as_deref(), Some("zooms[0].mode"));
    }
    let mut doc = follow;
    doc.video.cursor = CursorMode::Separate;
    doc.cursor = Some(CursorAsset {index_path:"cursor/index.json".into()});
    validate(&doc).unwrap();
    for version in 1..=3 {
        let mut old = doc.clone(); old.schema_version = version;
        assert!(validate(&old).is_err(), "follow accepted in schema {version}");
    }
    doc.cursor = None;
    assert!(validate(&doc).is_err());
}

#[test]
fn zoom_mode_serializing_old_domain_cannot_silently_erase_follow() {
    let mut wire = common::project_wire(4);
    wire["zooms"][0]["mode"] = serde_json::json!("follow");
    let mut doc: ProjectDocument = serde_json::from_value(wire).unwrap();
    for version in 1..=3 {
        doc.schema_version = version;
        assert!(serde_json::to_vec(&doc).is_err(), "serializing schema {version} erased follow mode");
    }
}
