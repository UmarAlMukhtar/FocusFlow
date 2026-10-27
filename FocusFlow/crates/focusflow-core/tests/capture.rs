mod common;
use common::{fixture_project, TestDir};
use focusflow_core::{legacy::LegacyDefaults, project::*, store::ProjectStore};
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}, process::Command};
fn defaults() -> LegacyDefaults { LegacyDefaults {scale:2.0,intro_us:0,hold_us:0,outro_us:0,pan_transition_us:0,settle_us:0} }
fn manifest() -> Value {
 let mut doc=fixture_project();doc.schema_version=3;doc.video.cursor=CursorMode::Separate;doc.video.duration_us=1_000_000;doc.clips[0].out_us=1_000_000;
 doc.cursor=Some(CursorAsset{index_path:"cursor/index.json".into()});doc.cursor_settings.enabled=true;doc.warnings=vec![ProjectWarning::LegacyTimingUnknown];
 json!({"schemaVersion":1,"captureId":"fixture","source":{"backend":"linuxX11","kind":"region","initialBounds":{"x":40,"y":60,"width":100,"height":50},"windowId":null},"diagnostic":null,
 "state":{"status":"finalized","videoTiming":{"endpoint":{"provenance":"savedPacketFirstAndTail","endPts":15360,"timeBase":{"numerator":1,"denominator":15360},"interval":"halfOpen"},"clock":{"sourceClock":"unixMicrosecondsPreImageCopyRequest","epoch":"captureLaunchMonotonicNs","anchor":{"lowNs":"149900000","highExclusiveNs":"150100000"},"completedFrames":30,"cadence":{"numerator":1,"denominator":30},"provenance":"completedEncoderGridAndSavedPacketCadence","interactions":"deliveryTimeRebasedOnce","audio":"unmeasured"}},"cursorTiming":{"clock":"videoFrameZeroRelativeUs","observation":"x11RequestBracketMidpoint","initialState":"boundedPreRollHold","visibility":"sameScreenNativeImageOrArrow1xIntersectsCrop","serverVisibility":"unknown"},"initialProject":doc}})
}
fn setup() -> (TestDir, PathBuf, ProjectStore, Value) {
 let temp=TestDir::new();let dir=temp.path().join("fixture");fs::create_dir_all(dir.join("cursor")).unwrap();
 let result=Command::new("ffmpeg").args(["-hide_banner","-loglevel","error","-y","-f","lavfi","-i","color=c=black:s=100x50:r=30:d=1","-c:v","libx264","-pix_fmt","yuv420p"]).arg(dir.join("screen.mp4")).status().unwrap();assert!(result.success());
 fs::write(dir.join("cursor/index.json"),json!({"schemaVersion":1,"width":100,"height":50,"durationUs":1_000_000,"samplesPath":"cursor/samples.ndjson","shapes":[{"id":"a","path":"cursor/a.pam","width":2,"height":2,"hotspotX":1,"hotspotY":1}]}).to_string()).unwrap();
 let samples=(0..30).map(|i|json!({"atUs":i*33333,"x":0.5,"y":0.5,"visible":true,"shapeId":"a","discontinuity":false}).to_string()+"\n").collect::<String>();fs::write(dir.join("cursor/samples.ndjson"),samples).unwrap();
 let mut pam=b"P7\nWIDTH 2\nHEIGHT 2\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();pam.extend([255;16]);fs::write(dir.join("cursor/a.pam"),pam).unwrap();
 let value=manifest();save(&dir,&value);let store=ProjectStore::new(temp.path()).unwrap();(temp,dir,store,value)
}
fn save(dir:&Path,value:&Value) {fs::write(dir.join("capture.json"),serde_json::to_vec(value).unwrap()).unwrap();}
fn saved_evidence() -> focusflow_core::ffmpeg::ProbedVideoEvidence {
 use focusflow_core::ffmpeg::{ProbedVideo,ProbedVideoEvidence,PacketTime};
 ProbedVideoEvidence{video:ProbedVideo{width:100,height:50,duration_us:1_000_000},endpoint:PacketTime{ticks:15360,numerator:1,denominator:15360},first_packet_duration:PacketTime{ticks:512,numerator:1,denominator:15360}}
}
fn protected_assets(dir:&Path)->std::collections::BTreeMap<PathBuf,Vec<u8>> {
 let mut files=std::collections::BTreeMap::new();
 for entry in fs::read_dir(dir).unwrap() {
  let entry=entry.unwrap();let kind=entry.file_type().unwrap();
  if kind.is_dir() {files.extend(protected_assets(&entry.path()));}
  else if kind.is_file() {files.insert(entry.path(),fs::read(entry.path()).unwrap());}
 }
 files
}
fn validated(dir:&Path,value:&Value)->focusflow_core::CoreResult<ProjectDocument> {
 let before=protected_assets(dir);
 let result=focusflow_core::capture::validate_finalized_assets(dir,&decode(value)?,&saved_evidence());
 assert_eq!(protected_assets(dir),before,"shared validation changed assets");result
}
#[test]
fn shared_validator_checks_structure_and_exact_media_without_mutation() {
 let (_temp,dir,_store,value)=setup();
 let paths=["capture.json","screen.mp4","cursor/index.json","cursor/samples.ndjson","cursor/a.pam"];
 let before:Vec<_>=paths.iter().map(|path|fs::read(dir.join(path)).unwrap()).collect();
 let manifest=decode(&value).unwrap();
 let mut malformed=manifest.clone();malformed.schema_version=2;
 assert!(focusflow_core::capture::validate_finalized_assets(&dir,&malformed,&saved_evidence()).is_err());
 let mut incomplete=value.clone();incomplete["state"]=json!({"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"});
 assert!(validated(&dir,&incomplete).is_err());
 let mut equivalent=saved_evidence();equivalent.endpoint.numerator=2;equivalent.endpoint.denominator=30720;
 assert_eq!(serde_json::to_value(focusflow_core::capture::validate_finalized_assets(&dir,&manifest,&equivalent).unwrap()).unwrap(),value["state"]["initialProject"]);
 let mut unequal=saved_evidence();unequal.endpoint=focusflow_core::ffmpeg::PacketTime{ticks:2000001,numerator:1,denominator:2000000};
 assert!(focusflow_core::capture::validate_finalized_assets(&dir,&manifest,&unequal).is_err());
 let mut wrong_size=saved_evidence();wrong_size.video.width=102;
 assert!(focusflow_core::capture::validate_finalized_assets(&dir,&manifest,&wrong_size).is_err());
 for (path,bytes) in paths.iter().zip(before) {assert_eq!(fs::read(dir.join(path)).unwrap(),bytes,"mutated {path}");}
 assert!(!dir.join("project.json").exists());
}
#[cfg(unix)]
#[test]
fn shared_validator_rejects_native_asset_aliases() {
 use std::os::unix::fs::symlink;
 for relative in ["screen.mp4","cursor/index.json","cursor/samples.ndjson","cursor/a.pam","mic.wav"] {
  let (_temp,dir,_store,mut value)=setup();value["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});
  if relative=="mic.wav" {fs::write(dir.join(relative),b"invalid").unwrap();}
  let original=dir.join(relative);let target=dir.join("target");fs::rename(&original,&target).unwrap();symlink(&target,&original).unwrap();
  let before=fs::read(&target).unwrap();assert!(validated(&dir,&value).is_err(),"{relative}");assert_eq!(fs::read(&target).unwrap(),before);assert!(fs::symlink_metadata(&original).unwrap().file_type().is_symlink());
 }
}
#[test]
fn production_import_accepts_complete_native_snapshot() {
 let (_temp,dir,store,value)=setup();let original=fs::read(dir.join("screen.mp4")).unwrap();let authority=fs::read(dir.join("capture.json")).unwrap();
 let direct=validated(&dir,&value).unwrap();
 let result=store.open_or_import("fixture",Path::new("ffmpeg"),defaults());assert!(result.is_ok(),"{result:?}");let doc=result.unwrap();
 assert_eq!(serde_json::to_value(&direct).unwrap(),serde_json::to_value(&doc).unwrap());
 assert_eq!(serde_json::to_value(&doc).unwrap(),value["state"]["initialProject"]);assert!(store.capabilities(&doc).cursor_native_style);
 assert_eq!(fs::read(dir.join("capture.json")).unwrap(),authority);assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),original);
}

fn degrade(value:&mut Value) {
 value["diagnostic"]=json!({"stage":"cursor","message":"Partial cursor observations; saved video is complete."});
 value["state"]["initialProject"]["cursorSettings"]["enabled"]=json!(false);
 value["state"]["initialProject"]["warnings"].as_array_mut().unwrap().push(json!("cursorCaptureIncomplete"));
}
fn absent(value:&mut Value,clock:bool) {
 degrade(value);value["state"]["cursorTiming"]=Value::Null;
 value["state"]["initialProject"]["video"]["cursor"]=json!("absent");value["state"]["initialProject"]["cursor"]=Value::Null;
 if !clock {value["state"]["videoTiming"]["clock"]=Value::Null;}
}
fn decode(value:&Value)->focusflow_core::CoreResult<focusflow_core::capture::CaptureManifestV1> {focusflow_core::capture::decode(&serde_json::to_vec(value).unwrap(),"fixture")}
fn rejected(store:&ProjectStore,dir:&Path,value:&Value) {
 save(dir,value);let paths=["capture.json","screen.mp4","cursor/index.json","cursor/samples.ndjson"];
 let before:Vec<_>=paths.iter().map(|path|fs::read(dir.join(path)).unwrap()).collect();
 if decode(value).is_ok() {assert!(validated(dir,value).is_err(),"shared validator accepted {value}");}
 assert!(store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).is_err(),"accepted {value}");assert!(!dir.join("project.json").exists());
 for (path,bytes) in paths.iter().zip(before) {assert_eq!(fs::read(dir.join(path)).unwrap(),bytes,"mutated {path}");}
}
#[test]
fn degradation_requires_valid_positions_and_explicit_disabled_policy() {
 for loss in ["missing-image","corrupt-image","position-only","partial"] {
  let (_temp,dir,store,mut value)=setup();degrade(&mut value);
  match loss {
   "missing-image"=>fs::remove_file(dir.join("cursor/a.pam")).unwrap(),
   "corrupt-image"=>fs::write(dir.join("cursor/a.pam"),b"corrupt").unwrap(),
   "position-only"=>{let bytes=fs::read_to_string(dir.join("cursor/samples.ndjson")).unwrap().replace("\"a\"","null");fs::write(dir.join("cursor/samples.ndjson"),bytes).unwrap();},
   _=>{let rows=json!({"atUs":0,"x":0.5,"y":0.5,"visible":true,"shapeId":"a","discontinuity":false}).to_string()+"\n"+&json!({"atUs":100000,"x":0.5,"y":0.5,"visible":false,"shapeId":null,"discontinuity":true}).to_string()+"\n";fs::write(dir.join("cursor/samples.ndjson"),rows).unwrap();}
  }
  let mut enabled=value.clone();enabled["state"]["initialProject"]["cursorSettings"]["enabled"]=json!(true);rejected(&store,&dir,&enabled);
  let mut undeclared=value.clone();undeclared["diagnostic"]=Value::Null;undeclared["state"]["initialProject"]["warnings"]=json!(["legacyTimingUnknown"]);
  if loss!="partial" {rejected(&store,&dir,&undeclared);}
  save(&dir,&value);let direct=validated(&dir,&value).unwrap();let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(serde_json::to_value(direct).unwrap(),serde_json::to_value(&doc).unwrap());assert!(!doc.cursor_settings.enabled);assert!(doc.warnings.contains(&ProjectWarning::CursorCaptureIncomplete));
  assert!(store.capabilities(&doc).cursor_replacement);
  fs::remove_file(dir.join("project.json")).unwrap();fs::write(dir.join("cursor/samples.ndjson"),b"broken\n").unwrap();rejected(&store,&dir,&value);
 }
}
#[test]
fn absent_cursor_accepts_null_or_independently_valid_clock_without_reading_raw_samples() {
 for clock in [false,true] {
  let (_temp,dir,store,mut value)=setup();absent(&mut value,clock);fs::remove_dir_all(dir.join("cursor")).unwrap();save(&dir,&value);
  let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(doc.video.cursor,CursorMode::Absent);assert!(!store.capabilities(&doc).cursor_replacement);
  assert_eq!(serde_json::to_value(doc).unwrap(),value["state"]["initialProject"]);
 }
}
#[test]
fn native_arrow_choice_and_capture_defaults_are_preserved() {
 let (_temp,dir,store,mut value)=setup();
 value["state"]["initialProject"]["cursorSettings"]=json!({"enabled":true,"scale":2.0,"smoothingUs":100000,"style":"arrow"});
 value["state"]["initialProject"]["camera"]=json!({"panTransitionUs":234567,"settleUs":345678});
 value["state"]["initialProject"]["clicks"]=json!([{"atUs":100000,"x":0.5,"y":0.2}]);
 value["state"]["initialProject"]["zooms"]=json!([{"id":"zoom-0","clipId":"clip-0","startUs":100000,"endUs":500000,"x":0.5,"y":0.5,"scale":2.0,"introUs":0,"holdUs":0,"outroUs":0}]);
 save(&dir,&value);let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(serde_json::to_value(doc).unwrap(),value["state"]["initialProject"]);
}
// Exercise every native object, including objects inside the reused project decoder.
fn object_paths(value:&Value,path:String,out:&mut Vec<String>) {
 match value {Value::Object(fields)=>{out.push(path.clone());for (key,value) in fields {object_paths(value,format!("{path}/{key}"),out);}},Value::Array(values)=>for (i,value) in values.iter().enumerate(){object_paths(value,format!("{path}/{i}"),out);},_=>{}}
}
fn duplicate_at(value:&Value,path:&str)->String {
 if path.is_empty() {let object=value.as_object().unwrap();let (key,field)=object.iter().next().unwrap();let mut text=value.to_string();text.pop();text.push_str(&format!(",{}:{}}}",json!(key),field));return text;}
 let (part,rest)=path.strip_prefix('/').unwrap().split_once('/').map(|(a,b)|(a,format!("/{b}"))).unwrap_or((path.strip_prefix('/').unwrap(),String::new()));
 match value {
  Value::Object(fields)=>format!("{{{}}}",fields.iter().map(|(key,value)|format!("{}:{}",json!(key),if key==part {duplicate_at(value,&rest)} else {value.to_string()})).collect::<Vec<_>>().join(",")),
  Value::Array(values)=>format!("[{}]",values.iter().enumerate().map(|(i,value)|if i.to_string()==part {duplicate_at(value,&rest)} else {value.to_string()}).collect::<Vec<_>>().join(",")),_=>panic!("not an object path")
 }
}
#[test]
fn every_native_field_is_required_and_objects_reject_unknown_and_duplicate_keys() {
 let mut with_audio=manifest();with_audio["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});
 let mut values=vec![manifest(),with_audio];let mut degraded=manifest();absent(&mut degraded,false);values.push(degraded);
 let mut incomplete=manifest();incomplete["state"]=json!({"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"});values.push(incomplete);
 for value in values {
  decode(&value).unwrap();let mut paths=vec![];object_paths(&value,String::new(),&mut paths);
  for path in paths {
   let mut bad=value.clone();bad.pointer_mut(&path).unwrap().as_object_mut().unwrap().insert("unknownField".into(),json!(true));assert!(decode(&bad).is_err(),"unknown {path}");
   let raw=duplicate_at(&value,&path);assert!(focusflow_core::capture::decode(raw.as_bytes(),"fixture").is_err(),"duplicate {path}: {raw}");
   for key in value.pointer(&path).unwrap().as_object().unwrap().keys() {
    let mut bad=value.clone();bad.pointer_mut(&path).unwrap().as_object_mut().unwrap().remove(key);assert!(decode(&bad).is_err(),"missing {path}/{key}");
   }
  }
 }
}
#[test]
fn strict_values_and_semantic_relationships_reject_invalid_authority() {
 let value=manifest();
 for (path,bad) in [
  ("/schemaVersion",json!(2)),("/schemaVersion",json!(1.0)),("/captureId",json!("other")),("/state/status",json!("complete")),("/source/backend",json!("wayland")),
  ("/source/windowId",json!("1")),("/source/initialBounds/x",json!(-1)),("/source/initialBounds/x",json!(2147483647)),("/source/initialBounds/y",json!(2147483647)),
  ("/source/initialBounds/width",json!(101)),("/source/initialBounds/width",json!(0)),("/source/initialBounds/height",json!(16386)),
  ("/state/videoTiming/endpoint/endPts",json!(0)),("/state/videoTiming/endpoint/endPts",json!(9223372036854775807u64)),("/state/videoTiming/endpoint/endPts",json!(-1)),("/state/videoTiming/endpoint/endPts",json!(15360.0)),
  ("/state/videoTiming/endpoint/timeBase/numerator",json!(0)),("/state/videoTiming/endpoint/timeBase/denominator",json!(2147483648u64)),("/state/videoTiming/endpoint/timeBase/denominator",json!(0)),
  ("/state/videoTiming/endpoint/interval",json!("closed")),("/state/videoTiming/endpoint/provenance",json!("containerDuration")),
  ("/state/videoTiming/clock",Value::Null),("/state/videoTiming/clock/completedFrames",json!(0)),("/state/videoTiming/clock/completedFrames",json!(31)),("/state/videoTiming/clock/completedFrames",json!(2592001)),
  ("/state/videoTiming/clock/cadence/numerator",json!(2)),("/state/videoTiming/clock/cadence/denominator",json!(60)),
  ("/state/videoTiming/clock/anchor/lowNs",json!("-1")),("/state/videoTiming/clock/anchor/lowNs",json!("00")),("/state/videoTiming/clock/anchor/lowNs",json!("+1")),("/state/videoTiming/clock/anchor/lowNs",json!("1e6")),("/state/videoTiming/clock/anchor/lowNs",json!(1)),
  ("/state/videoTiming/clock/anchor/highExclusiveNs",json!("149900000")),("/state/videoTiming/clock/anchor/highExclusiveNs",json!("150901001")),("/state/videoTiming/clock/anchor/highExclusiveNs",json!("170141183460469231731687303715884105728")),
  ("/state/cursorTiming",Value::Null),("/state/initialProject/schemaVersion",json!(2)),("/state/initialProject/revision",json!(1)),("/state/initialProject/projectId",json!("other")),
  ("/state/initialProject/video/path",json!("other.mp4")),("/state/initialProject/video/width",json!(102)),("/state/initialProject/video/durationUs",json!(999999)),("/state/initialProject/video/cursor",json!("embedded")),
  ("/state/initialProject/cursor/indexPath",json!("index.json")),("/state/initialProject/microphone",json!({"path":"audio.wav"})),("/state/initialProject/timing/provenance",json!("measured")),("/state/initialProject/timing/audioOffsetUs",json!(0)),
  ("/state/initialProject/clips/0/id",json!("other")),("/state/initialProject/clips/0/inUs",json!(1)),("/state/initialProject/clips/0/outUs",json!(999999)),
  ("/state/initialProject/warnings",json!([])),("/state/initialProject/warnings",json!(["legacyTimingUnknown","embeddedCursor"])),("/state/initialProject/warnings",json!(["legacyTimingUnknown","cursorCaptureIncomplete"])),
  ("/diagnostic",json!({"stage":"clock","message":"without degradation"})),
 ] {let mut bad_value=value.clone();*bad_value.pointer_mut(path).unwrap()=bad;assert!(decode(&bad_value).is_err(),"accepted {path}: {}",bad_value.pointer(path).unwrap());}
 let mut window=value.clone();window["source"]["kind"]=json!("window");window["source"]["initialBounds"]["x"]=json!(-100);
 for xid in ["1","4294967295"] {window["source"]["windowId"]=json!(xid);decode(&window).unwrap();}
 for xid in [Value::Null,json!(1),json!("0"),json!("01"),json!("+1"),json!("4294967296"),json!("0x20")] {window["source"]["windowId"]=xid;assert!(decode(&window).is_err());}
 let mut degraded=value.clone();degrade(&mut degraded);
 for text in ["x".repeat(1025),String::new(),"é".repeat(513)] {degraded["diagnostic"]["message"]=json!(text);assert!(decode(&degraded).is_err());}
 degraded["diagnostic"]["message"]=json!("é".repeat(512));decode(&degraded).unwrap();
}
#[test]
fn exact_endpoint_and_clock_boundary_arithmetic_is_not_floored_early() {
 let mut value=manifest();absent(&mut value,false);
 for (ticks,n,d,duration,valid) in [(2u64,1u32,2_000_000u32,1u64,true),(1,1,2_000_000,1,false),(86_400,1,1,86_400_000_000,true),(172_800_000_001,1,2_000_000,86_400_000_000,false),(i64::MAX as u64-1,i32::MAX as u32,i32::MAX as u32,1,false)] {
  let mut candidate=value.clone();candidate["state"]["videoTiming"]["endpoint"]["endPts"]=json!(ticks);candidate["state"]["videoTiming"]["endpoint"]["timeBase"]=json!({"numerator":n,"denominator":d});
  candidate["state"]["initialProject"]["video"]["durationUs"]=json!(duration);candidate["state"]["initialProject"]["clips"][0]["outUs"]=json!(duration);
  assert_eq!(decode(&candidate).is_ok(),valid,"{candidate}");
 }
 let mut value=manifest();value["state"]["videoTiming"]["clock"]["anchor"]=json!({"lowNs":"0","highExclusiveNs":"1001000"});decode(&value).unwrap();
 value["state"]["videoTiming"]["clock"]["anchor"]=json!({"lowNs":"170141183460469231731687303715884105726","highExclusiveNs":"170141183460469231731687303715884105727"});decode(&value).unwrap();
 for dimension in [2,16384] {value["source"]["initialBounds"]["width"]=json!(dimension);value["state"]["initialProject"]["video"]["width"]=json!(dimension);decode(&value).unwrap();}
 let mut value=manifest();value["state"]["videoTiming"]["clock"]["completedFrames"]=json!(2592000);value["state"]["videoTiming"]["endpoint"]["endPts"]=json!(86400);value["state"]["videoTiming"]["endpoint"]["timeBase"]=json!({"numerator":1,"denominator":1});value["state"]["initialProject"]["video"]["durationUs"]=json!(86400000000u64);value["state"]["initialProject"]["clips"][0]["outUs"]=json!(86400000000u64);decode(&value).unwrap();
}
#[test]
fn imports_compare_exact_endpoint_and_geometry_and_keep_sources_on_failure() {
 let (_temp,dir,store,mut value)=setup();absent(&mut value,false);
 // 1 second plus half a microsecond has the same microsecond projection as the real video.
 value["state"]["videoTiming"]["endpoint"]["endPts"]=json!(2000001);value["state"]["videoTiming"]["endpoint"]["timeBase"]=json!({"numerator":1,"denominator":2000000});decode(&value).unwrap();rejected(&store,&dir,&value);
 value=manifest();value["state"]["videoTiming"]["endpoint"]["timeBase"]=json!({"numerator":2,"denominator":30720});save(&dir,&value);
 let evidence=focusflow_core::ffmpeg::probe_video_evidence(Path::new("ffmpeg"),&dir.join("screen.mp4")).unwrap();assert_eq!((evidence.video.width,evidence.video.height,evidence.video.duration_us),(100,50,1_000_000));assert_eq!(evidence.endpoint.ticks*evidence.endpoint.numerator as u64,evidence.endpoint.denominator as u64);assert_eq!(evidence.first_packet_duration.ticks*evidence.first_packet_duration.numerator as u64*30,evidence.first_packet_duration.denominator as u64);
 // Independent ffprobe reading verifies generated media endpoint, geometry and frame count.
 let output=Command::new("ffprobe").args(["-v","error","-select_streams","v:0","-show_entries","stream=width,height,nb_frames,duration_ts,time_base","-of","json"]).arg(dir.join("screen.mp4")).output().unwrap();assert!(output.status.success());let media:Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(media["streams"][0]["nb_frames"],"30");assert_eq!(media["streams"][0]["width"],100);assert_eq!(media["streams"][0]["height"],50);assert_eq!(media["streams"][0]["duration_ts"],15360);assert_eq!(media["streams"][0]["time_base"],"1/15360");
 store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();fs::remove_file(dir.join("project.json")).unwrap();
 value["source"]["initialBounds"]["width"]=json!(102);value["state"]["initialProject"]["video"]["width"]=json!(102);rejected(&store,&dir,&value);
}
#[test]
fn optional_microphone_projects_only_usable_wav_and_keeps_authority() {
 for mode in ["valid","missing","empty","corrupt","header-only"] {
  let (_temp,dir,store,mut value)=setup();value["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});
  if mode=="valid"||mode=="header-only" {let mut writer=hound::WavWriter::create(dir.join("mic.wav"),hound::WavSpec{channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();if mode=="valid" {for _ in 0..480 {writer.write_sample(123i16).unwrap();}}writer.finalize().unwrap();}
  if mode=="empty" {fs::write(dir.join("mic.wav"),b"").unwrap();}if mode=="corrupt" {fs::write(dir.join("mic.wav"),[1;80]).unwrap();}
  save(&dir,&value);let before=fs::read(dir.join("capture.json")).unwrap();let direct=validated(&dir,&value).unwrap();let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(serde_json::to_value(direct).unwrap(),serde_json::to_value(&doc).unwrap());assert_eq!(doc.microphone.is_some(),mode=="valid");assert_eq!(doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio),mode!="valid");assert_eq!(doc.timing.audio_offset_us,None);assert_eq!(fs::read(dir.join("capture.json")).unwrap(),before);
 }
}
#[test]
fn authority_precedence_and_listing_are_bounded_without_asset_reads() {
 let (_temp,dir,store,mut value)=setup();fs::remove_dir_all(dir.join("cursor")).unwrap();fs::remove_file(dir.join("screen.mp4")).unwrap();
 let page=store.list_projects(None,1).unwrap();assert!(page.issues.is_empty());let summary=&page.projects[0];assert!(summary.needs_import);assert_eq!(summary.revision,None);assert_eq!(summary.duration_us,Some(1_000_000));assert!(!summary.capabilities.cursor_replacement);assert!(!summary.capabilities.cursor_native_style);
 value["state"]=json!({"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"});save(&dir,&value);
 assert!(store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).unwrap_err().message.contains("incomplete"));assert!(!dir.join("project.json").exists());assert_eq!(store.list_projects(None,1).unwrap().issues.len(),1);
 let native=manifest();fs::write(dir.join("project.json"),native["state"]["initialProject"].to_string()).unwrap();fs::write(dir.join("capture.json"),b"invalid").unwrap();let doc=store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).unwrap();assert_eq!(doc.project_id,"fixture");assert!(!store.list_projects(None,1).unwrap().projects[0].needs_import);
 for bytes in [b"invalid".as_slice(),br#"{"schemaVersion":99}"#.as_slice()] {fs::write(dir.join("project.json"),bytes).unwrap();assert!(store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).is_err());assert!(store.list_projects(None,1).unwrap().projects.is_empty());assert_eq!(fs::read(dir.join("project.json")).unwrap(),bytes);}
}
#[test]
fn invalid_native_rows_advance_pagination_and_cap_error_messages() {
 let temp=TestDir::new();let store=ProjectStore::new(temp.path()).unwrap();
 for id in ["a","b","c"] {let dir=temp.path().join(id);fs::create_dir(&dir).unwrap();let mut value=manifest();value["captureId"]=json!(id);value["state"]["initialProject"]["projectId"]=json!(id);if id=="b" {value["state"]["future".repeat(2000)]=json!(true);}save(&dir,&value);}
 let page=store.list_projects(None,2).unwrap();assert_eq!(page.projects.len(),1);assert_eq!(page.issues.len(),1);assert!(page.issues[0].error.message.len()<=1024);assert_eq!(page.next_cursor.as_deref(),Some("b"));let next=store.list_projects(page.next_cursor.as_deref(),2).unwrap();assert_eq!(next.projects[0].project_id,"c");assert!(next.next_cursor.is_none());
}
#[cfg(unix)]
#[test]
fn native_aliases_and_cursor_namespace_are_errors_even_for_optional_missing_assets() {
 use std::os::unix::fs::symlink;
 for relative in ["capture.json","screen.mp4","mic.wav","cursor/index.json","cursor/samples.ndjson","cursor/a.pam"] {
  for dangling in [false,true] {
   let (_temp,dir,store,mut value)=setup();value["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});save(&dir,&value);if relative=="mic.wav" {fs::write(dir.join(relative),b"invalid audio").unwrap();}
   let original=dir.join(relative);let target=dir.join("target");fs::rename(&original,&target).unwrap();if dangling {fs::remove_file(&target).unwrap();}symlink(&target,&original).unwrap();
   let error=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap_err();assert_eq!(error.code,focusflow_core::ErrorCode::InvalidPath,"{relative} {error:?}");assert!(!dir.join("project.json").exists());
  }
 }
 let (_temp,dir,store,value)=setup();for relative in ["../outside","samples.ndjson","cursor/../screen.mp4","cursor//samples.ndjson","cursor/./samples.ndjson","cursor/a:stream","cursor/a\\b"] {
  let index=json!({"schemaVersion":1,"width":100,"height":50,"durationUs":1_000_000,"samplesPath":relative,"shapes":[]});fs::write(dir.join("cursor/index.json"),index.to_string()).unwrap();rejected(&store,&dir,&value);
 }
 // Directory aliases are rejected even if a missing optional cursor image lies beneath them.
 let (_temp,dir,store,value)=setup();fs::rename(dir.join("cursor"),dir.join("cursor-real")).unwrap();symlink(dir.join("cursor-real"),dir.join("cursor")).unwrap();rejected(&store,&dir,&value);
 let (_temp,dir,store,_)=setup();fs::remove_file(dir.join("capture.json")).unwrap();fs::create_dir(dir.join("capture.json")).unwrap();assert!(store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).is_err());assert_eq!(store.list_projects(None,1).unwrap().issues.len(),1);
}
#[test]
fn capture_size_cap_and_general_project_option_compatibility() {
 let mut bytes=manifest().to_string().into_bytes();bytes.resize(focusflow_core::project::MAX_JSON_BYTES as usize,b' ');focusflow_core::capture::decode(&bytes,"fixture").unwrap();bytes.push(b' ');assert!(focusflow_core::capture::decode(&bytes,"fixture").is_err());
 let mut project=manifest()["state"]["initialProject"].clone();project.as_object_mut().unwrap().remove("microphone");project["timing"].as_object_mut().unwrap().remove("audioOffsetUs");serde_json::from_value::<ProjectDocument>(project.clone()).unwrap();let mut value=manifest();value["state"]["initialProject"]=project;assert!(decode(&value).is_err());
}
#[test]
fn direct_native_errors_are_bounded_even_with_long_unicode_unknown_fields() {
 let mut value=manifest();value["é".repeat(1_000_000)]=json!(true);let error=decode(&value).unwrap_err();assert!(error.message.len()<=1024);assert!(error.message.contains("native capture")&&error.message.contains("capture.json"));
 let (_temp,dir,store,_)=setup();save(&dir,&value);let error=store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).unwrap_err();assert!(error.message.len()<=1024);assert!(!dir.join("project.json").exists());
}
#[test]
fn every_declared_native_image_must_be_complete_before_initial_enabling() {
 for style in ["native","arrow"] {
  for loss in ["missing","corrupt"] {
   let (_temp,dir,store,mut value)=setup();value["state"]["initialProject"]["cursorSettings"]["style"]=json!(style);
   let mut index:Value=serde_json::from_slice(&fs::read(dir.join("cursor/index.json")).unwrap()).unwrap();
   index["shapes"].as_array_mut().unwrap().push(json!({"id":"unused","path":"cursor/unused.pam","width":2,"height":2,"hotspotX":1,"hotspotY":1}));fs::write(dir.join("cursor/index.json"),index.to_string()).unwrap();
   if loss=="corrupt" {fs::write(dir.join("cursor/unused.pam"),b"broken").unwrap();}
   rejected(&store,&dir,&value);
   degrade(&mut value);save(&dir,&value);let direct=validated(&dir,&value).unwrap();let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!(serde_json::to_value(direct).unwrap(),serde_json::to_value(&doc).unwrap());assert!(!doc.cursor_settings.enabled);
   // Ordinary project capability/edit rules still ignore unused images after strict initial import.
   assert!(store.capabilities(&doc).cursor_native_style);
   let mut settings=doc.cursor_settings.clone();settings.enabled=true;settings.style=CursorStyle::Arrow;
   let edited=store.apply_edits("fixture",0,&[focusflow_core::edits::Edit::SetCursorSettings{settings}]).unwrap();assert!(edited.cursor_settings.enabled);assert_eq!(edited.revision,1);assert_eq!(fs::read(dir.join("capture.json")).unwrap(),serde_json::to_vec(&value).unwrap());
  }
 }
}
#[test]
fn duplicate_state_tags_and_nullable_fields_never_fall_through() {
 let raw=manifest().to_string();
 for (field,value) in [("schemaVersion","1"),("status","\"finalized\""),("diagnostic","null"),("microphone","null"),("audioOffsetUs","null"),("windowId","null")] {
  let needle=format!("\"{field}\":{value}");let replacement=format!("{needle},{needle}");let bad=raw.replacen(&needle,&replacement,1);assert_ne!(bad,raw);assert!(focusflow_core::capture::decode(bad.as_bytes(),"fixture").is_err(),"duplicate {field}");
 }
 let mixed=raw.replace("\"status\":\"finalized\"","\"status\":\"incomplete\",\"status\":\"finalized\"");assert!(focusflow_core::capture::decode(mixed.as_bytes(),"fixture").is_err());
}

fn wire_fixtures() -> Vec<Value> {
 let mut full=manifest();full["state"]["initialProject"]["microphone"]=json!({"path":"mic.wav"});
 full["state"]["initialProject"]["clicks"]=json!([{"atUs":100000,"x":0.5,"y":0.2}]);
 full["state"]["initialProject"]["zooms"]=json!([{"id":"zoom-0","clipId":"clip-0","startUs":100000,"endUs":500000,"x":0.5,"y":0.5,"scale":2.0,"introUs":0,"holdUs":0,"outroUs":0}]);
 let mut gradient=full.clone();gradient["state"]["initialProject"]["frameStyle"]["background"]=json!({"type":"gradient","start":"#112233","end":"#445566"});gradient["state"]["initialProject"]["cursorSettings"]["style"]=json!("arrow");gradient["state"]["initialProject"]["frameStyle"]["canvas"]=json!("square");
 let mut degraded=full.clone();degrade(&mut degraded);
 let mut missing=full.clone();absent(&mut missing,false);
 let mut incomplete=manifest();incomplete["state"]=json!({"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"});
 vec![full,gradient,degraded,missing,incomplete]
}
fn literal_positions(value:&Value)->Vec<&'static str> {
 let candidates=["/source/backend","/source/kind","/diagnostic/stage","/state/status","/state/videoPath","/state/videoCursor","/state/rawSamplesPath",
 "/state/videoTiming/endpoint/provenance","/state/videoTiming/endpoint/interval","/state/videoTiming/clock/sourceClock","/state/videoTiming/clock/epoch","/state/videoTiming/clock/provenance","/state/videoTiming/clock/interactions","/state/videoTiming/clock/audio",
 "/state/cursorTiming/clock","/state/cursorTiming/observation","/state/cursorTiming/initialState","/state/cursorTiming/visibility","/state/cursorTiming/serverVisibility",
 "/state/initialProject/video/cursor","/state/initialProject/video/path","/state/initialProject/cursor/indexPath","/state/initialProject/cursorSettings/style","/state/initialProject/microphone/path","/state/initialProject/timing/provenance","/state/initialProject/frameStyle/canvas","/state/initialProject/frameStyle/background/type","/state/initialProject/clips/0/id","/state/initialProject/warnings/0","/state/initialProject/warnings/1"];
 candidates.into_iter().filter(|path|value.pointer(path).is_some_and(Value::is_string)).collect()
}
fn wrong_literal_types(literal:&Value)->[Value;5] {
 [json!({literal.as_str().unwrap():null}),json!([literal]),json!(true),Value::Null,json!(1)]
}
fn positional_object(value:&Value,path:&str)->Value {
 let keys:&[&str]=match path {
  ""=>&["schemaVersion","captureId","source","diagnostic","state"],
  "/source"=>&["backend","kind","initialBounds","windowId"],
  "/source/initialBounds"=>&["x","y","width","height"],
  "/diagnostic"=>&["stage","message"],
  "/state" if value["status"]=="incomplete"=>&["status","videoPath","videoCursor","rawSamplesPath"],
  "/state"=>&["status","videoTiming","cursorTiming","initialProject"],
  "/state/videoTiming"=>&["endpoint","clock"],
  "/state/videoTiming/endpoint"=>&["provenance","endPts","timeBase","interval"],
  "/state/videoTiming/endpoint/timeBase"|"/state/videoTiming/clock/cadence"=>&["numerator","denominator"],
  "/state/videoTiming/clock"=>&["sourceClock","epoch","anchor","completedFrames","cadence","provenance","interactions","audio"],
  "/state/videoTiming/clock/anchor"=>&["lowNs","highExclusiveNs"],
  "/state/cursorTiming"=>&["clock","observation","initialState","visibility","serverVisibility"],
  "/state/initialProject"=>&["frameStyle","schemaVersion","projectId","revision","video","cursor","cursorSettings","microphone","timing","camera","clips","zooms","clicks","warnings"],
  "/state/initialProject/video"=>&["cursor","path","width","height","durationUs"],
  "/state/initialProject/cursor"=>&["indexPath"],
  "/state/initialProject/cursorSettings"=>&["enabled","scale","smoothingUs","style"],
  "/state/initialProject/microphone"=>&["path"],
  "/state/initialProject/timing"=>&["provenance","audioOffsetUs"],
  "/state/initialProject/camera"=>&["panTransitionUs","settleUs"],
  "/state/initialProject/frameStyle"=>&["canvas","background","paddingPx","cornerRadiusPx","shadow"],
  "/state/initialProject/frameStyle/background" if value["type"]=="solid"=>&["type","color"],
  "/state/initialProject/frameStyle/background"=>&["type","start","end"],
  "/state/initialProject/clips/0"=>&["id","inUs","outUs"],
  "/state/initialProject/zooms/0"=>&["id","clipId","startUs","endUs","x","y","scale","introUs","holdUs","outroUs"],
  "/state/initialProject/clicks/0"=>&["atUs","x","y"],
  _=>panic!("Missing named-object regression fixture for {path}"),
 };
 assert_eq!(value.as_object().unwrap().len(),keys.len(),"fixture fields {path}");
 Value::Array(keys.iter().map(|key|value.get(key).unwrap_or_else(||panic!("Missing {path}/{key}")).clone()).collect())
}
#[test]
fn native_wire_literals_require_strings_at_every_contract_position() {
 let mut accepted=vec![];
 for (fixture,value) in wire_fixtures().into_iter().enumerate() {
  decode(&value).unwrap();
  for path in literal_positions(&value) {
   for replacement in wrong_literal_types(value.pointer(path).unwrap()) {
    let mut bad=value.clone();*bad.pointer_mut(path).unwrap()=replacement.clone();
    if decode(&bad).is_ok() {accepted.push(format!("fixture {fixture} {path}: {replacement}"));}
   }
  }
 }
 assert!(accepted.is_empty(),"accepted non-string literal encodings: {accepted:#?}");
}
#[test]
fn native_wire_named_objects_reject_valid_positional_arrays_everywhere() {
 let mut accepted=vec![];
 for (fixture,value) in wire_fixtures().into_iter().enumerate() {
  decode(&value).unwrap();let mut paths=vec![];object_paths(&value,String::new(),&mut paths);
  for path in paths {
   let mut bad=value.clone();*bad.pointer_mut(&path).unwrap()=positional_object(value.pointer(&path).unwrap(),&path);
   if decode(&bad).is_ok() {accepted.push(format!("fixture {fixture} {path}"));}
  }
 }
 assert!(accepted.is_empty(),"accepted positional named objects: {accepted:#?}");
}
#[test]
fn native_wire_wrong_types_fail_production_import_without_writes() {
 let (_temp,dir,store,_)=setup();
 for value in wire_fixtures() {
  for path in literal_positions(&value) {
   for replacement in wrong_literal_types(value.pointer(path).unwrap()) {
    let mut bad=value.clone();*bad.pointer_mut(path).unwrap()=replacement;
    rejected(&store,&dir,&bad);
    assert!(serde_json::from_value::<focusflow_core::capture::CaptureManifestV1>(bad).is_err());
   }
  }
  let mut paths=vec![];object_paths(&value,String::new(),&mut paths);
  for path in paths {
   let mut bad=value.clone();*bad.pointer_mut(&path).unwrap()=positional_object(value.pointer(&path).unwrap(),&path);
   rejected(&store,&dir,&bad);
   assert!(serde_json::from_value::<focusflow_core::capture::CaptureManifestV1>(bad).is_err());
  }
 }
}
#[test]
fn native_wire_guards_preserve_genuine_collections_and_ordinary_project_reads() {
 let (_temp,dir,store,_)=setup();let mut value=wire_fixtures().remove(0);value["state"]["initialProject"]["microphone"]=Value::Null;save(&dir,&value);
 let doc=store.open_or_import("fixture",Path::new("ffmpeg"),defaults()).unwrap();assert_eq!((doc.clips.len(),doc.zooms.len(),doc.clicks.len(),doc.warnings.len()),(1,1,1,1));
 // Alternate Serde representations remain compatible for ordinary saved project documents.
 let mut old=value["state"]["initialProject"].clone();old["video"]["cursor"]=json!({"separate":null});old["camera"]=json!([200000,100000]);
 let decoded=serde_json::from_value::<ProjectDocument>(old.clone()).unwrap();assert_eq!(decoded,doc);fs::write(dir.join("project.json"),old.to_string()).unwrap();let before=fs::read(dir.join("project.json")).unwrap();
 assert_eq!(store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).unwrap(),doc);assert_eq!(fs::read(dir.join("project.json")).unwrap(),before);
}

#[test]
fn zoom_mode_capture_v1_keeps_schema_three_fixed_initial_project() {
    let mut value = manifest();
    let mut zoom = common::fixed_zoom_wire();
    zoom["startUs"] = json!(100_000); zoom["endUs"] = json!(700_000);
    value["state"]["initialProject"]["zooms"] = json!([zoom]);
    let decoded = decode(&value).unwrap(); decoded.validate("fixture").unwrap();
    let doc = decoded.finalized_project().unwrap();
    assert_eq!((doc.schema_version, doc.revision), (3, 0));
    assert_eq!(doc.zooms[0].mode, ZoomMode::Fixed);
    assert_eq!(serde_json::to_value(doc).unwrap(), value["state"]["initialProject"]);
    for mode in ["fixed", "follow"] {
        let mut bad = value.clone(); bad["state"]["initialProject"]["zooms"][0]["mode"] = json!(mode);
        assert!(decode(&bad).is_err());
    }
    value["state"]["initialProject"]["schemaVersion"] = json!(4);
    value["state"]["initialProject"]["zooms"][0]["mode"] = json!("fixed");
    assert!(decode(&value).is_err());
}
