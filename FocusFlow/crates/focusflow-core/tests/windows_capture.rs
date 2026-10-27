mod common;
use common::TestDir;
use focusflow_core::{capture::{self, windows::*}, ffmpeg::{SavedCfrGrid, ProbedVideoEvidence, ProbedVideo, PacketTime}, legacy::LegacyDefaults, project::*, store::ProjectStore};
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}, process::Command};
const INCOMPLETE: &str = include_str!("fixtures/windows-incomplete.json");
const FINAL: &str = include_str!("fixtures/windows-finalized.json");
const MARKER: &[u8] = b"focusflow-windows-capture-v2\n";
fn decode(value: &Value) -> focusflow_core::CoreResult<WindowsCaptureManifestV2> { capture::decode_windows(&serde_json::to_vec(value).unwrap(),"fixture") }
fn final_value() -> Value { serde_json::from_str(FINAL).unwrap() }
fn defaults() -> LegacyDefaults { LegacyDefaults { scale:3.0,intro_us:123,hold_us:456,outro_us:789,pan_transition_us:999,settle_us:888 } }
fn setup(media: bool) -> (TestDir,PathBuf,ProjectStore) {
    let temp=TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();
    if media { assert!(Command::new("ffmpeg").args(["-v","error","-f","lavfi","-i","color=c=blue:s=642x360:r=30","-frames:v","30","-c:v","libx264","-bf","0","-pix_fmt","yuv420p"]).arg(dir.join("screen.mp4")).status().unwrap().success()); }
    let store=ProjectStore::new(temp.path()).unwrap();(temp,dir,store)
}
fn save(dir:&Path,value:&Value) { fs::write(dir.join("capture.json"),value.to_string()).unwrap(); }
fn mark(dir:&Path) {fs::write(dir.join("capture.windows-v2"),MARKER).unwrap();}
fn evidence() -> SavedCfrGrid { SavedCfrGrid {completed_frames:30,video:ProbedVideoEvidence {video:ProbedVideo {width:642,height:360,duration_us:1_000_000},endpoint:PacketTime {ticks:30000,numerator:1,denominator:30000},first_packet_duration:PacketTime {ticks:1000,numerator:1,denominator:30000}}} }
fn reject(dir:&Path,store:&ProjectStore) -> focusflow_core::CoreError {
    let before=fs::read(dir.join("screen.mp4")).ok();
    let error=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap_err();assert!(!dir.join("project.json").exists());
    assert_eq!(fs::read(dir.join("screen.mp4")).ok(),before);error
}
#[test]
fn exact_literal_examples_roundtrip_and_v1_decoder_remains_strict() {
    for text in [INCOMPLETE,FINAL] {
        let manifest=capture::decode_windows(text.as_bytes(),"fixture").unwrap();
        assert_eq!(serde_json::to_value(manifest).unwrap(),serde_json::from_str::<Value>(text).unwrap());
        assert!(capture::decode(text.as_bytes(),"fixture").is_err());
        assert!(capture::decode_windows(text.as_bytes(),"other").is_err());
    }
}
fn objects(value:&Value,path:String,out:&mut Vec<String>) {
    match value {Value::Object(fields)=>{out.push(path.clone());for (key,v) in fields {objects(v,format!("{path}/{key}"),out);}},Value::Array(values)=>for (i,v) in values.iter().enumerate(){objects(v,format!("{path}/{i}"),out);},_=>{}}
}
fn duplicate(value:&Value,path:&str)->String {
    if path.is_empty() {let (key,v)=value.as_object().unwrap().iter().next().unwrap();let mut text=value.to_string();text.pop();return format!("{text},{}:{v}}}",json!(key));}
    let (part,rest)=path[1..].split_once('/').map(|(a,b)|(a,format!("/{b}"))).unwrap_or((&path[1..],String::new()));
    match value {Value::Object(fields)=>format!("{{{}}}",fields.iter().map(|(k,v)|format!("{}:{}",json!(k),if k==part {duplicate(v,&rest)}else{v.to_string()})).collect::<Vec<_>>().join(",")),Value::Array(vs)=>format!("[{}]",vs.iter().enumerate().map(|(i,v)|if i.to_string()==part{duplicate(v,&rest)}else{v.to_string()}).collect::<Vec<_>>().join(",")),_=>panic!("object path")}
}
#[test]
fn deep_objects_require_every_field_and_reject_duplicates_arrays_and_unknowns() {
    let mut extra=final_value();extra["diagnostic"]=json!({"stage":"publishing","message":"Retained"});extra["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});
    for value in [serde_json::from_str(INCOMPLETE).unwrap(),final_value(),extra] {
        decode(&value).unwrap();let mut paths=vec![];objects(&value,String::new(),&mut paths);
        for path in paths {
            let fields=value.pointer(&path).unwrap().as_object().unwrap();
            for key in fields.keys() {let mut bad=value.clone();bad.pointer_mut(&path).unwrap().as_object_mut().unwrap().remove(key);assert!(decode(&bad).is_err(),"missing {path}/{key}");}
            let mut bad=value.clone();bad.pointer_mut(&path).unwrap()["unknown"]=json!(true);assert!(decode(&bad).is_err(),"unknown {path}");
            let mut bad=value.clone();*bad.pointer_mut(&path).unwrap()=Value::Array(fields.values().cloned().collect());assert!(decode(&bad).is_err(),"array {path}");
            assert!(capture::decode_windows(duplicate(&value,&path).as_bytes(),"fixture").is_err(),"duplicate {path}");
        }
    }
}
#[test]
fn strict_literals_explicit_nulls_versions_and_bounded_json() {
    for path in ["/source/backend","/source/kind","/state/status","/state/videoTiming/endpoint/provenance","/state/videoTiming/endpoint/interval","/state/videoTiming/clock/provenance","/state/videoTiming/clock/cutoffReason","/state/videoTiming/clock/audio","/state/videoTiming/clock/interactions","/initialDefaults/frameStyle/canvas","/initialDefaults/frameStyle/background/type","/state/initialProject/video/cursor","/state/initialProject/cursorSettings/style","/state/initialProject/timing/provenance"] {
        for replacement in [json!({final_value().pointer(path).unwrap().as_str().unwrap():null}),json!(12),Value::Null,json!("other")] {let mut value=final_value();*value.pointer_mut(path).unwrap()=replacement;assert!(decode(&value).is_err(),"{path}");}
    }
    for path in ["/source/pointerOrigin","/state/cursorTiming"] {for replacement in [json!({}),json!([]),json!(0),json!("null"),json!(false)] {let mut value=final_value();*value.pointer_mut(path).unwrap()=replacement;assert!(decode(&value).is_err(),"{path}");}}
    for version in [json!(1),json!(3),json!(2.0),json!("2"),json!(-2),Value::Null] {let mut value=final_value();value["schemaVersion"]=version;assert!(decode(&value).is_err());}
    let mut bytes=FINAL.as_bytes().to_vec();bytes.resize(MAX_JSON_BYTES as usize,b' ');capture::decode_windows(&bytes,"fixture").unwrap();bytes.push(b' ');assert!(capture::decode_windows(&bytes,"fixture").is_err());
}
#[test]
fn canonical_handles_qpc_and_geometry_use_exact_bounded_integers() {
    for handle in ["1","4294967297","18446744073709551615"] {let mut value=final_value();value["source"]["windowId"]=json!(handle);decode(&value).unwrap();}
    for field in ["/source/windowId","/state/videoTiming/clock/qpcFrequency","/state/videoTiming/clock/admissionQpc","/state/videoTiming/clock/epoch100ns","/state/videoTiming/clock/cutoffQpc"] {
        for invalid in [json!(""),json!("+1"),json!("-0"),json!("-1"),json!("01"),json!(" 1"),json!("1e6"),json!(1),json!("18446744073709551616")] {let mut value=final_value();*value.pointer_mut(field).unwrap()=invalid;assert!(decode(&value).is_err(),"{field}");}
    }
    for (path,bad) in [("/source/windowId",json!("0")),("/state/videoTiming/clock/qpcFrequency",json!("0")),("/state/videoTiming/clock/epoch100ns",json!("9223372036854775808")),("/source/rawSize/width",json!(1)),("/source/rawSize/height",json!(16385)),("/source/encodedSize/width",json!(644)),("/source/contentRect/x",json!(1)),("/source/contentRect/width",json!(642)),("/source/observedWindowBounds/x",json!(2147483647)),("/source/observedWindowBounds/width",json!(2147483648u64)),("/source/observedWindowBounds/height",json!(0))] {let mut value=final_value();*value.pointer_mut(path).unwrap()=bad;assert!(decode(&value).is_err(),"{path}");}
    let mut value=final_value();value["source"]["observedWindowBounds"]=Value::Null;decode(&value).unwrap();
    for message in ["".to_owned(),"é".repeat(513)] {value["diagnostic"]=json!({"stage":"clock","message":message});assert!(decode(&value).is_err());}
}
#[test]
fn checked_clock_cells_counts_exits_and_endpoint_bounds() {
    // F=3MHz: one complete extra cell is exactly 100000 QPC ticks.
    for (cutoff,ok) in [(5_999_999,false),(6_000_000,true),(6_000_001,true),(6_099_999,true),(6_100_000,false)] {let mut value=final_value();value["state"]["videoTiming"]["clock"]["cutoffQpc"]=json!(cutoff.to_string());assert_eq!(decode(&value).is_ok(),ok,"{cutoff}");}
    for (path,bad) in [("admissionQpc",json!("3000001")),("cutoffQpc",json!("2999999")),("completedFrames",json!(0)),("completedFrames",json!(31)),("completedFrames",json!(2592001)),("workerExitCode",json!(-1)),("encoderExitCode",json!(1))] {let mut value=final_value();value["state"]["videoTiming"]["clock"][path]=bad;assert!(decode(&value).is_err(),"{path}");}
    let mut value=final_value();value["state"]["videoTiming"]["clock"]["cutoffReason"]=json!("sourceClosed");decode(&value).unwrap();
    for (field,bad) in [("endPts",json!(0)),("endPts",json!(30001)),("endPts",json!(9223372036854775807u64))] {let mut value=final_value();value["state"]["videoTiming"]["endpoint"][field]=bad;assert!(decode(&value).is_err());}
    for n in [0u64,2147483648] {let mut value=final_value();value["state"]["videoTiming"]["endpoint"]["timeBase"]["numerator"]=json!(n);assert!(decode(&value).is_err());}
    let mut value=final_value();for field in ["qpcFrequency","epoch100ns","admissionQpc","cutoffQpc"] {value["state"]["videoTiming"]["clock"][field]=json!(i64::MAX.to_string());}assert!(decode(&value).is_err());
    // Zero admission/epoch and raw 100ns units, with a different clock frequency.
    let mut value=final_value();let c=&mut value["state"]["videoTiming"]["clock"];c["qpcFrequency"]=json!("1000");c["admissionQpc"]=json!("0");c["epoch100ns"]=json!("0");c["cutoffQpc"]=json!("1000");decode(&value).unwrap();
}
#[test]
fn one_frame_and_full_day_endpoints_floor_exactly_without_rounded_cutoff_math() {
    for (n,cutoff,duration) in [(1u64,3_100_000u64,33_333u64),(2_592_000,259_203_000_000,86_400_000_000)] {
        let mut value=final_value();value["state"]["videoTiming"]["clock"]["completedFrames"]=json!(n);value["state"]["videoTiming"]["clock"]["cutoffQpc"]=json!(cutoff.to_string());value["state"]["videoTiming"]["endpoint"]["endPts"]=json!(n*1000);
        value["state"]["initialProject"]["video"]["durationUs"]=json!(duration);value["state"]["initialProject"]["clips"][0]["outUs"]=json!(duration);decode(&value).unwrap();
        value["state"]["videoTiming"]["clock"]["cutoffQpc"]=json!((cutoff-1).to_string());assert!(decode(&value).is_err());
        value["state"]["videoTiming"]["clock"]["cutoffQpc"]=json!((cutoff+1).to_string());decode(&value).unwrap();
    }
}
#[test]
fn incomplete_accepts_even_extreme_geometry_and_freezes_nondefault_start_settings() {
    for (raw,encoded) in [(2,2),(16383,16384),(16384,16384)] {
        let mut value:Value=serde_json::from_str(INCOMPLETE).unwrap();value["source"]["rawSize"]=json!({"width":raw,"height":raw});value["source"]["encodedSize"]=json!({"width":encoded,"height":encoded});value["source"]["contentRect"]=json!({"x":0,"y":0,"width":raw,"height":raw});value["source"]["observedWindowBounds"]=json!({"x":-2147483648i64,"y":-100,"width":2147483647,"height":100});decode(&value).unwrap();
    }
    let (_temp,dir,_store)=setup(false);let mut value=final_value();value["initialDefaults"]["frameStyle"]=json!({"canvas":"landscape","background":{"type":"gradient","start":"#123456","end":"#654321"},"paddingPx":20,"cornerRadiusPx":10,"shadow":true});value["initialDefaults"]["camera"]=json!({"panTransitionUs":345678,"settleUs":234567});value["state"]["initialProject"]["frameStyle"]=value["initialDefaults"]["frameStyle"].clone();value["state"]["initialProject"]["camera"]=value["initialDefaults"]["camera"].clone();let final_manifest=decode(&value).unwrap();
    value["state"]=json!({"status":"incomplete","videoPath":"screen.mp4","videoCursor":"embedded"});capture::publish_windows_incomplete(&dir,&decode(&value).unwrap()).unwrap();fs::write(dir.join("screen.mp4"),b"producer evidence").unwrap();
    let mut changed=final_manifest.clone();changed.source.window_id="2".into();assert!(capture::publish_windows_finalized_assets(&dir,&changed,&evidence()).is_err());
    capture::publish_windows_finalized_assets(&dir,&final_manifest,&evidence()).unwrap();
}

#[test]
fn frozen_defaults_and_trusted_snapshot_cannot_manufacture_interactions() {
    for (path,bad) in [("/initialDefaults/camera/settleUs",json!(86400000001u64)),("/initialDefaults/frameStyle/paddingPx",json!(99999)),("/state/initialProject/revision",json!(1)),("/state/initialProject/schemaVersion",json!(4)),("/state/initialProject/projectId",json!("other")),("/state/initialProject/camera/settleUs",json!(100001)),("/state/initialProject/frameStyle/shadow",json!(true)),("/state/initialProject/video/path",json!("other.mp4")),("/state/initialProject/video/cursor",json!("absent")),("/state/initialProject/video/width",json!(640)),("/state/initialProject/cursorSettings/scale",json!(2.0)),("/state/initialProject/cursorSettings/enabled",json!(true)),("/state/initialProject/clips/0/inUs",json!(1)),("/state/initialProject/clips/0/id",json!("other")),("/state/initialProject/clicks",json!([{"atUs":100,"x":0.5,"y":0.5}])),("/state/initialProject/microphone",json!({"path":"other.wav"})),("/state/initialProject/timing/audioOffsetUs",json!(0))] {let mut value=final_value();*value.pointer_mut(path).unwrap()=bad;assert!(decode(&value).is_err(),"{path}");}
    let mut value=final_value();value["state"]["initialProject"]["zooms"]=json!([{"id":"z","clipId":"clip-0","startUs":0,"endUs":1000000,"x":0.5,"y":0.5,"scale":2.0,"introUs":0,"holdUs":0,"outroUs":0}]);assert!(decode(&value).is_err());
    for warnings in [json!([]),json!(["embeddedCursor"]),json!(["embeddedCursor","legacyTimingUnknown","embeddedCursor"]),json!(["embeddedCursor","legacyTimingUnknown","cursorCaptureIncomplete"])] {let mut value=final_value();value["state"]["initialProject"]["warnings"]=warnings;assert!(decode(&value).is_err());}
    let mut value=final_value();value["state"]["initialProject"]["warnings"]=json!(["invalidOptionalAudio","legacyTimingUnknown","embeddedCursor"]);decode(&value).unwrap();
}
#[test]
fn store_import_uses_full_grid_snapshot_and_ignores_legacy_settings_and_diagnostics() {
    let (_temp,dir,store)=setup(true);mark(&dir);save(&dir,&final_value());
    for path in ["clicks.json","drags.json","timeline.json","metadata.json","session.json"] {fs::write(dir.join(path),b"corrupt and deliberately ignored").unwrap();}
    let before=fs::read(dir.join("screen.mp4")).unwrap();let authority=fs::read(dir.join("capture.json")).unwrap();
    let page=store.list_projects(None,100).unwrap();assert!(page.issues.is_empty());assert!(page.projects[0].needs_import);
    let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(serde_json::to_value(&doc).unwrap(),final_value()["state"]["initialProject"]);
    assert!(!store.capabilities(&doc).cursor_replacement);assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),before);assert_eq!(fs::read(dir.join("capture.json")).unwrap(),authority);
    // Existing saved edits win even if capture authority later goes missing or malformed.
    fs::write(dir.join("capture.json"),b"broken").unwrap();fs::write(dir.join("capture.windows-v2"),b"partial").unwrap();
    assert_eq!(store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).unwrap(),doc);assert!(store.list_projects(None,100).unwrap().issues.is_empty());
}
#[test]
fn intent_marker_missing_partial_oversized_conflicting_and_nonregular_are_recovery() {
    for marker in [None,Some(vec![]),Some(b"focusflow-windows".to_vec()),Some([MARKER,b"x"].concat()),Some(vec![b'x';100000])] {let (_temp,dir,store)=setup(true);save(&dir,&final_value());if let Some(bytes)=marker {fs::write(dir.join("capture.windows-v2"),bytes).unwrap();}reject(&dir,&store);assert_eq!(store.list_projects(None,100).unwrap().issues.len(),1);}
    for json in [None,Some(b"".as_slice()),Some(b"{".as_slice()),Some(br#"{"schemaVersion":1}"#.as_slice()),Some(br#"{"schemaVersion":99}"#.as_slice()),Some(br#"{"schemaVersion":2}"#.as_slice())] {let (_temp,dir,store)=setup(true);mark(&dir);if let Some(bytes)=json {fs::write(dir.join("capture.json"),bytes).unwrap();}reject(&dir,&store);assert_eq!(store.list_projects(None,100).unwrap().issues.len(),1);}
    for path in ["capture.json","capture.windows-v2"] {let (_temp,dir,store)=setup(true);mark(&dir);save(&dir,&final_value());fs::remove_file(dir.join(path)).unwrap();fs::create_dir(dir.join(path)).unwrap();reject(&dir,&store);assert_eq!(store.list_projects(None,100).unwrap().issues.len(),1);}
    let (_temp,dir,store)=setup(true);mark(&dir);fs::write(dir.join("capture.json"),vec![b' ';MAX_JSON_BYTES as usize+1]).unwrap();reject(&dir,&store);assert_eq!(store.list_projects(None,100).unwrap().issues.len(),1);
}
#[test]
fn publication_freezes_start_then_commits_projected_assets_and_never_project_json() {
    let (_temp,dir,store)=setup(false);let start=capture::decode_windows(INCOMPLETE.as_bytes(),"fixture").unwrap();let final_doc=decode(&final_value()).unwrap();
    assert!(capture::publish_windows_finalized_assets(&dir,&final_doc,&evidence()).is_err());assert!(!dir.join("capture.windows-v2").exists());
    capture::publish_windows_incomplete(&dir,&start).unwrap();assert_eq!(fs::read(dir.join("capture.windows-v2")).unwrap(),MARKER);assert!(!dir.join("screen.mp4").exists());reject(&dir,&store);
    let before=fs::read(dir.join("capture.json")).unwrap();let mut changed=start.clone();changed.initial_defaults.camera.settle_us+=1;assert!(capture::publish_windows_incomplete(&dir,&changed).is_err());assert_eq!(fs::read(dir.join("capture.json")).unwrap(),before);
    fs::write(dir.join("screen.mp4"),b"trusted producer supplied evidence").unwrap();let committed=capture::publish_windows_finalized_assets(&dir,&final_doc,&evidence()).unwrap();assert_eq!(committed,final_doc);
    assert!(capture::publish_windows_finalized_assets(&dir,&final_doc,&evidence()).is_err());assert!(capture::publish_windows_incomplete(&dir,&start).is_err());assert!(!dir.join("project.json").exists());assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),b"trusted producer supplied evidence");
}
#[test]
fn public_validation_checks_full_grid_count_dimensions_duration_and_exact_cadence() {
    let (_temp,dir,_)=setup(false);fs::write(dir.join("screen.mp4"),b"producer evidence").unwrap();let manifest=decode(&final_value()).unwrap();
    capture::validate_windows_finalized_assets(&dir,&manifest,&evidence()).unwrap();
    for change in 0..5 {let mut bad=evidence();match change {0=>bad.completed_frames=29,1=>bad.video.endpoint.ticks+=1,2=>bad.video.first_packet_duration.ticks+=1,3=>bad.video.video.duration_us+=1,_=>bad.video.video.width+=2};assert!(capture::validate_windows_finalized_assets(&dir,&manifest,&bad).is_err());}
    for first in [false,true] {let mut bad=evidence();let packet=if first {&mut bad.video.first_packet_duration}else{&mut bad.video.endpoint};packet.numerator=0;packet.denominator=0;assert!(capture::validate_windows_finalized_assets(&dir,&manifest,&bad).is_err(),"zero-over-zero packet rational");}
    let mut equal=evidence();equal.video.endpoint.numerator=2;equal.video.endpoint.denominator=60000;capture::validate_windows_finalized_assets(&dir,&manifest,&equal).unwrap();
}
#[test]
fn optional_mic_projection_is_identical_for_publication_and_shared_import() {
    for kind in ["valid","missing","empty","corrupt"] {
        let (_temp,dir,store)=setup(true);let start=capture::decode_windows(INCOMPLETE.as_bytes(),"fixture").unwrap();capture::publish_windows_incomplete(&dir,&start).unwrap();
        match kind {"valid"=>{let mut w=hound::WavWriter::create(dir.join("mic.wav"),hound::WavSpec {channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();w.write_sample(42i16).unwrap();w.finalize().unwrap();},"empty"=>fs::write(dir.join("mic.wav"),b"").unwrap(),"corrupt"=>fs::write(dir.join("mic.wav"),b"not a wav").unwrap(),_=>{}}
        let before=fs::read(dir.join("mic.wav")).ok();let mut value=final_value();value["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});let candidate=decode(&value).unwrap();
        let projected=capture::validate_windows_finalized_assets(&dir,&candidate,&evidence()).unwrap();
        // Import must independently project unusable optional audio, without rewriting authority.
        save(&dir,&value);let imported=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(projected,imported);fs::remove_file(dir.join("project.json")).unwrap();save(&dir,&serde_json::to_value(&start).unwrap());
        let committed=capture::publish_windows_finalized_assets(&dir,&candidate,&evidence()).unwrap();assert_eq!(committed.finalized_project().unwrap(),&projected);
        assert_eq!(projected.microphone.is_some(),kind=="valid");assert_eq!(projected.warnings.contains(&ProjectWarning::InvalidOptionalAudio),kind!="valid");assert_eq!(fs::read(dir.join("mic.wav")).ok(),before);
    }
}
#[cfg(unix)]
#[test]
fn aliases_and_nonregular_assets_never_escape_or_block_import() {
    use std::os::unix::fs::symlink;
    for name in ["capture.json","capture.windows-v2","screen.mp4","mic.wav"] {
        for dangling in [false,true] {let (temp,dir,store)=setup(true);let mut value=final_value();value["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});mark(&dir);save(&dir,&value);let target=temp.path().join("external");if !dangling {fs::write(&target,b"foreign bytes").unwrap();}let path=dir.join(name);if path.exists(){fs::remove_file(&path).unwrap();}symlink(&target,&path).unwrap();assert_eq!(reject(&dir,&store).code,focusflow_core::ErrorCode::InvalidPath);if !dangling {assert_eq!(fs::read(target).unwrap(),b"foreign bytes");}}
    }
    for name in ["capture.json","capture.windows-v2","screen.mp4","mic.wav"] {let (_temp,dir,store)=setup(true);mark(&dir);let mut value=final_value();value["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});save(&dir,&value);let path=dir.join(name);if path.exists(){fs::remove_file(&path).unwrap();}assert!(Command::new("mkfifo").arg(path).status().unwrap().success());assert!(store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).is_err());assert!(!dir.join("project.json").exists());}
}
#[test]
fn marker_retry_and_cross_version_conflict_preserve_authority() {
    let (_temp,dir,_store)=setup(false);let start=capture::decode_windows(INCOMPLETE.as_bytes(),"fixture").unwrap();
    mark(&dir);capture::publish_windows_incomplete(&dir,&start).unwrap();
    let linux: capture::CaptureManifestV1=serde_json::from_value(json!({"schemaVersion":1,"captureId":"fixture","source":{"backend":"linuxX11","kind":"region","initialBounds":{"x":0,"y":0,"width":100,"height":50},"windowId":null},"diagnostic":null,"state":{"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"}})).unwrap();
    assert!(capture::publish(&dir,&linux).is_err());let before=fs::read(dir.join("capture.json")).unwrap();capture::publish_windows_incomplete(&dir,&start).unwrap();assert_eq!(fs::read(dir.join("capture.json")).unwrap(),before);
    for bytes in [serde_json::to_vec(&linux).unwrap(),br#"{"schemaVersion":99}"#.to_vec(),b"broken".to_vec()] {fs::write(dir.join("capture.json"),&bytes).unwrap();assert!(capture::publish_windows_incomplete(&dir,&start).is_err());assert_eq!(fs::read(dir.join("capture.json")).unwrap(),bytes);}
}
#[test]
fn shared_import_rejects_real_additional_stream_and_preserves_media() {
    let (_temp,dir,store)=setup(true);let extra=dir.join("extra.mp4");assert!(Command::new("ffmpeg").args(["-v","error","-i"]).arg(dir.join("screen.mp4")).args(["-f","lavfi","-i","anullsrc=r=48000:cl=mono","-map","0:v","-map","1:a","-c:v","copy","-c:a","aac","-t","1"]).arg(&extra).status().unwrap().success());fs::rename(extra,dir.join("screen.mp4")).unwrap();mark(&dir);save(&dir,&final_value());
    let before=fs::read(dir.join("screen.mp4")).unwrap();assert!(store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).is_err());assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),before);assert!(!dir.join("project.json").exists());
}
#[cfg(target_os="linux")]
#[test]
fn shared_import_rejects_bad_interior_without_first_tail_fallback() {
    use std::os::unix::fs::PermissionsExt;
    let (temp,dir,store)=setup(false);mark(&dir);save(&dir,&final_value());fs::write(dir.join("screen.mp4"),b"immutable media").unwrap();
    let header="#format: frame checksums\n#version: 2\n#hash: adler32\n#software: Lavf62.3.100\n#tb 0: 1/30000\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 642x360\n#sar 0: 1/1\n#stream#, dts, pts, duration, size, hash\n";
    let tool=temp.path().join("fake-ffmpeg");fs::write(&tool,b"#!/bin/sh\ncat \"$0.output\"\n").unwrap();fs::set_permissions(&tool,fs::Permissions::from_mode(0o700)).unwrap();
    for bad in [true,false] {let rows=(0..30).map(|n|{let t=n*1000+u32::from(bad&&n==15);format!("0, {t}, {t}, 1000, 123, abcdef01\n")}).collect::<String>();fs::write(temp.path().join("fake-ffmpeg.output"),format!("{header}{rows}")).unwrap();let result=store.open_or_import("fixture",&tool,defaults());assert_eq!(result.is_err(),bad);assert_eq!(dir.join("project.json").exists(),!bad);assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),b"immutable media");}
}
