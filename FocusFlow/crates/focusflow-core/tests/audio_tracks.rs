mod common;
use focusflow_core::{project::{self, ProjectDocument}, edits::Edit, store::ProjectStore};
use serde_json::{json, Value};

fn schema5() -> Value {
    let mut wire = common::project_wire(4);
    wire["schemaVersion"] = json!(5);
    wire.as_object_mut().unwrap().remove("microphone");
    wire.as_object_mut().unwrap().remove("timing");
    wire["audioTracks"] = json!({"microphone":null,"system":{"path":"system.wav","sampleRate":48000,"channels":2,"frameCount":48000,"timing":{"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1000000,"provenance":"synthetic","segments":[{"generation":0,"sourceStartFrame":0,"sourceEndFrame":48000,"nativeStartTick":0,"nativeEndTick":1000000,"videoStartUs":250000,"videoEndUs":1250000,"uncertaintyUs":1}]}}});
    wire
}
#[test]
fn schema5_roundtrip_and_strict_legacy() {
    let wire=schema5();
    let doc:ProjectDocument=serde_json::from_value(wire.clone()).expect("schema5 accepted");
    project::validate(&doc).unwrap();
    assert_eq!(serde_json::to_value(&doc).unwrap(),wire);
    let mut future=doc.clone();future.schema_version=6;assert!(serde_json::to_value(future).is_err());
    for version in 1..=4 {
        let mut old=common::project_wire(version); old["audioTracks"]=wire["audioTracks"].clone();
        assert!(serde_json::from_value::<ProjectDocument>(old).is_err());
        let mut old=common::project_wire(version); old["warnings"]=json!(["invalidSystemAudio"]);
        assert!(serde_json::from_value::<ProjectDocument>(old).is_err());
    }
    for key in ["microphone","timing","unexpected"] {
        let mut bad=wire.clone();bad[key]=Value::Null;
        assert!(serde_json::from_value::<ProjectDocument>(bad).is_err(),"{key}");
    }
    for key in ["microphone","system"] {
        let mut bad=wire.clone();bad["audioTracks"].as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());
    }
    let mut bad=wire;bad["schemaVersion"]=json!(6);
    assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());
}
#[test]
fn schema5_bounds_and_segment_order() {
    let valid=schema5();
    for (key,value) in [("sampleRate",json!(7999)),("sampleRate",json!(192001)),("channels",json!(3)),("frameCount",json!(0)),("frameCount",json!(48000_u64*86400+1)),("path",json!("../escape.wav"))] {
        let mut bad=valid.clone();bad["audioTracks"]["system"][key]=value;
        let result=serde_json::from_value::<ProjectDocument>(bad).and_then(|d|project::validate(&d).map(|_|d).map_err(serde::de::Error::custom));
        assert!(result.is_err(),"{key}");
    }
    for (key,value) in [("sourceEndFrame",json!(48001)),("nativeEndTick",json!(0)),("videoEndUs",json!(250000)),("uncertaintyUs",json!(86400000001_u64)),("nativeStartTick",json!(9007199254740992_u64))] {
        let mut bad=valid.clone();bad["audioTracks"]["system"]["timing"]["segments"][0][key]=value;
        let d:ProjectDocument=serde_json::from_value(bad).unwrap(); assert!(project::validate(&d).is_err(),"{key}");
    }
}
#[test]
fn schema5_edit_restore_keeps_audio_immutable() {
    let temp=common::TestDir::new();let dir=temp.path().join("fixture");std::fs::create_dir(&dir).unwrap();
    let wire=schema5();let original=serde_json::to_vec(&wire).unwrap();
    std::fs::write(dir.join("project.json"),&original).unwrap();std::fs::write(dir.join("screen.mp4"),b"video").unwrap();
    let store=ProjectStore::new(temp.path()).unwrap();let before=store.get("fixture").unwrap();
    let state=serde_json::from_value(json!({"clips":wire["clips"],"zooms":wire["zooms"],"frameStyle":wire["frameStyle"],"cursorSettings":wire["cursorSettings"]})).unwrap();
    let snapshot=store.snapshot("fixture",0).unwrap();
    assert_eq!(snapshot.document.schema_version,5);
    assert_eq!(std::fs::read(dir.join("project.json")).unwrap(),original);
    let edited=store.apply_edits("fixture",0,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:500000,out_us:4000000}]).unwrap();
    assert_eq!(edited.schema_version,5);assert_eq!(serde_json::to_value(&edited).unwrap()["audioTracks"],wire["audioTracks"]);
    let restored=store.restore_edits("fixture",1,&state).unwrap();assert_eq!(restored.schema_version,5);
    assert_eq!(serde_json::to_value(store.get("fixture").unwrap()).unwrap()["audioTracks"],wire["audioTracks"]);
    assert_eq!(before.revision,0);
}

fn audio_fixture(offset: i64, rate: u32, frames:u32) -> (common::TestDir, focusflow_core::store::RenderSnapshot) {
    let temp=common::TestDir::new();std::fs::write(temp.path().join("screen.mp4"),b"video").unwrap();
    let spec=hound::WavSpec{channels:1,sample_rate:rate,bits_per_sample:16,sample_format:hound::SampleFormat::Int};
    let mut w=hound::WavWriter::create(temp.path().join("system.wav"),spec).unwrap();for i in 0..frames {w.write_sample((i%123) as i16).unwrap();}w.finalize().unwrap();
    let mut v=schema5();v["zooms"]=json!([]);v["audioTracks"]["system"]["channels"]=json!(1);v["audioTracks"]["system"]["sampleRate"]=json!(rate);v["audioTracks"]["system"]["frameCount"]=json!(frames);v["audioTracks"]["system"]["timing"]=json!({"type":"estimated","offsetUs":offset,"uncertaintyUs":0});
    let snapshot=focusflow_core::store::RenderSnapshot{project_dir:temp.path().to_path_buf(),document:serde_json::from_value(v).unwrap(),prepared_cursor:None,prepared_focus:None};(temp,snapshot)
}
#[test]
fn sample_grid_offsets_gaps_restarts_and_clip_phase() {
    use focusflow_core::{audio::compile_audio,render::{self,ExportPreset}};
    let (_temp,mut s)=audio_fixture(250000,48000,96000);let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let a=compile_audio(&s,&plan,None,true).unwrap();
    assert_eq!(a.total_samples,240000);assert_eq!(a.spans[0].sample_count,12000);assert!(a.spans[0].source.is_none());assert_eq!(a.spans[1].output_first_sample,12000);
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(json!({"type":"estimated","offsetUs":-250000,"uncertaintyUs":0})).unwrap();
    let a=compile_audio(&s,&plan,None,true).unwrap();assert_eq!(a.spans[0].source.as_ref().unwrap().position_numerator/a.spans[0].source.as_ref().unwrap().position_denominator,12000);
    s.document.clips=vec![project::Clip{id:"a".into(),in_us:0,out_us:333333},project::Clip{id:"b".into(),in_us:0,out_us:333333}];
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let a=compile_audio(&s,&plan,None,true).unwrap();assert_eq!(a.total_samples,32000);assert_eq!(a.spans.iter().map(|s|s.sample_count).sum::<u64>(),32000);
}
#[test]
fn preview_is_crop_of_full_audio_plan() {
    use focusflow_core::{audio::compile_audio,render::{self,ExportPreset},preview::{self,PreviewRequest}};
    let (_temp,mut s)=audio_fixture(-12345,44100,220500);s.document.clips=vec![project::Clip{id:"a".into(),in_us:111111,out_us:2333333},project::Clip{id:"b".into(),in_us:123456,out_us:1345678}];
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let full=compile_audio(&s,&plan,None,true).unwrap();let selection=preview::select(&plan,&PreviewRequest::Video{start_us:1_000_000,end_us:2_500_000}).unwrap();let crop=compile_audio(&s,&plan,Some(&selection),true).unwrap();
    assert_eq!(crop.first_sample,48000);assert_eq!(crop.total_samples,72000);
    for span in &crop.spans {let absolute=span.output_first_sample+crop.first_sample;let original=full.spans.iter().find(|s|s.output_first_sample<=absolute && absolute<s.output_first_sample+s.sample_count).unwrap();if let (Some(a),Some(b))=(&span.source,&original.source){let shift=i128::from(absolute-original.output_first_sample);assert_eq!(a.position_numerator*b.position_denominator,(b.position_numerator+shift*b.step_numerator*b.position_denominator/b.step_denominator)*a.position_denominator);}}
}

#[test]
fn mapped_drift_uses_explicit_anchors() {
    use focusflow_core::{audio::compile_audio,render::{self,ExportPreset}};
    let (_t,mut s)=audio_fixture(0,48000,96000);
    let track=s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap();
    track.timing=serde_json::from_value(json!({"type":"mapped","clockDomain":"qpc","ticksPerSecond":1000000,"provenance":"native","segments":[
        {"generation":0,"sourceStartFrame":0,"sourceEndFrame":48000,"nativeStartTick":500,"nativeEndTick":1000500,"videoStartUs":0,"videoEndUs":1001000,"uncertaintyUs":77},
        {"generation":1,"sourceStartFrame":48000,"sourceEndFrame":96000,"nativeStartTick":0,"nativeEndTick":1000000,"videoStartUs":1251000,"videoEndUs":2252000,"uncertaintyUs":88}]})).unwrap();
    let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();let a=compile_audio(&s,&p,None,true).unwrap();
    assert_eq!(a.spans[0].sample_count,48048);assert_eq!(a.spans[1].sample_count,12000);assert!(a.spans[1].source.is_none());
    let span=a.spans[0].source.as_ref().unwrap();assert_eq!(span.step_numerator*1001,span.step_denominator*1000);
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=project::AudioTrackTiming::Unknown {};
    let b=compile_audio(&s,&p,None,true).unwrap();let span=b.spans[0].source.as_ref().unwrap();assert_eq!(span.step_numerator,span.step_denominator);
}
#[test]
fn audio_plan_caps_before_growth() {
    use focusflow_core::{audio::compile_audio,render::{self,ExportPreset}};
    let (_t,mut s)=audio_fixture(0,8000,8000);
    let segments:Vec<_>=(0..1024).map(|n|json!({"generation":0,"sourceStartFrame":n*2,"sourceEndFrame":n*2+1,"nativeStartTick":n*250,"nativeEndTick":n*250+125,"videoStartUs":n*250,"videoEndUs":n*250+125,"uncertaintyUs":0})).collect();
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(json!({"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1000000,"provenance":"synthetic","segments":segments})).unwrap();
    s.document.clips=(0..10).map(|n|project::Clip{id:format!("c{n}"),in_us:0,out_us:1_000_000}).collect();let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    assert!(compile_audio(&s,&p,None,true).unwrap_err().message.contains("16384"));
}
#[cfg(unix)]
#[test]
fn optional_assets_validate_independently_and_containment_is_hard_error() {
    use focusflow_core::{audio::prepare_audio,ErrorCode};
    let (_t,mut s)=audio_fixture(0,44100,44100);
    let tracks=s.document.audio_tracks.as_mut().unwrap();let mut mic=tracks.system.clone().unwrap();mic.path="missing.wav".into();tracks.microphone=Some(mic);
    assert_eq!(prepare_audio(&s,true).unwrap().len(),1);
    std::os::unix::fs::symlink("gone.wav",s.project_dir.join("missing.wav")).unwrap();assert_eq!(prepare_audio(&s,true).unwrap_err().code,ErrorCode::InvalidPath);
    std::fs::remove_file(s.project_dir.join("missing.wav")).unwrap();std::os::unix::fs::symlink("/etc/passwd",s.project_dir.join("missing.wav")).unwrap();assert_eq!(prepare_audio(&s,true).unwrap_err().code,ErrorCode::InvalidPath);
}

#[test]
fn mapped_generations_bounds_and_immutable_edit_boundary() {
    let good=schema5();let segment=good["audioTracks"]["system"]["timing"]["segments"][0].clone();
    for (field,value) in [("sampleRate",json!(8000)),("sampleRate",json!(192000)),("channels",json!(1)),("channels",json!(2))] {
        let mut wire=good.clone();wire["audioTracks"]["system"][field]=value;wire["audioTracks"]["system"]["timing"]=json!({"type":"unknown"});project::validate(&serde_json::from_value::<ProjectDocument>(wire).unwrap()).unwrap();
    }
    let mut wire=good.clone();wire["audioTracks"]["system"]["frameCount"]=json!(96000);
    let mut next=segment.clone();next["sourceStartFrame"]=json!(48000);next["sourceEndFrame"]=json!(96000);next["videoStartUs"]=json!(1250000);next["videoEndUs"]=json!(2250000);next["generation"]=json!(1);
    wire["audioTracks"]["system"]["timing"]["segments"].as_array_mut().unwrap().push(next.clone());project::validate(&serde_json::from_value::<ProjectDocument>(wire.clone()).unwrap()).unwrap();
    for (field,value) in [("generation",json!(0)),("sourceStartFrame",json!(47999)),("videoStartUs",json!(1249999))] {
        let mut bad=wire.clone();bad["audioTracks"]["system"]["timing"]["segments"][1][field]=value;
        assert!(project::validate(&serde_json::from_value::<ProjectDocument>(bad).unwrap()).is_err(),"{field}");
    }
    for key in ["clockDomain","provenance"] {let mut bad=good.clone();bad["audioTracks"]["system"]["timing"][key]=json!("future");assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());}
    let mut doc:ProjectDocument=serde_json::from_value(good).unwrap();doc.schema_version=4;assert!(serde_json::to_value(&doc).is_err());
    let state=json!({"clips":doc.clips,"zooms":doc.zooms,"frameStyle":doc.frame_style,"cursorSettings":doc.cursor_settings,"audioTracks":null});assert!(serde_json::from_value::<project::EditableState>(state).is_err());
}
#[test]
fn corrupt_encoding_or_metadata_drops_only_optional_track() {
    use focusflow_core::audio::prepare_audio;
    let (_t,mut s)=audio_fixture(0,48000,48000);
    let mut microphone=s.document.audio_tracks.as_ref().unwrap().system.clone().unwrap();microphone.path="mic.wav".into();s.document.audio_tracks.as_mut().unwrap().microphone=Some(microphone);
    for bytes in [vec![],b"not a WAV".to_vec(),std::fs::read(s.project_dir.join("system.wav")).unwrap()[..60].to_vec()] {
        std::fs::write(s.project_dir.join("mic.wav"),bytes).unwrap();let prepared=prepare_audio(&s,true).unwrap();assert_eq!(prepared.len(),1);assert_eq!(prepared[0].kind,project::AudioTrackKind::System);
    }
    std::fs::copy(s.project_dir.join("system.wav"),s.project_dir.join("mic.wav")).unwrap();s.document.audio_tracks.as_mut().unwrap().microphone.as_mut().unwrap().frame_count=47999;assert_eq!(prepare_audio(&s,true).unwrap().len(),1);
    let mut w=hound::WavWriter::create(s.project_dir.join("mic.wav"),hound::WavSpec{channels:1,sample_rate:48000,bits_per_sample:32,sample_format:hound::SampleFormat::Float}).unwrap();w.write_sample(f32::NAN).unwrap();w.finalize().unwrap();assert_eq!(prepare_audio(&s,true).unwrap().len(),1);
}

#[test]
fn strict_timing_wire_and_exact_limits() {
    let original=schema5();
    for timing in [json!({"type":"unknown","offsetUs":0}),json!({"type":"estimated","offsetUs":0}),json!({"type":"estimated","offsetUs":0,"uncertaintyUs":0,"segments":[]}),json!({"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1,"provenance":"synthetic"})] {
        let mut bad=original.clone();bad["audioTracks"]["system"]["timing"]=timing;assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());
    }
    for (rate,channels) in [(8000,1),(192000,2)] {
        let mut wire=original.clone();wire["audioTracks"]["system"]["sampleRate"]=json!(rate);wire["audioTracks"]["system"]["channels"]=json!(channels);wire["audioTracks"]["system"]["frameCount"]=json!(rate as u64*86400);wire["audioTracks"]["system"]["timing"]=json!({"type":"estimated","offsetUs":-86400000000_i64,"uncertaintyUs":86400000000_u64});
        project::validate(&serde_json::from_value::<ProjectDocument>(wire.clone()).unwrap()).unwrap();wire["audioTracks"]["system"]["timing"]["offsetUs"]=json!(-86400000001_i64);assert!(project::validate(&serde_json::from_value::<ProjectDocument>(wire).unwrap()).is_err());
    }
    let mut wire=original;wire["audioTracks"]["system"]["sampleRate"]=json!(8000);wire["audioTracks"]["system"]["frameCount"]=json!(2048);
    let mut segments:Vec<_>=(0..1024).map(|n|json!({"generation":0,"sourceStartFrame":n*2,"sourceEndFrame":n*2+1,"nativeStartTick":n*250,"nativeEndTick":n*250+125,"videoStartUs":n*250,"videoEndUs":n*250+125,"uncertaintyUs":0})).collect();
    wire["audioTracks"]["system"]["timing"]["segments"]=json!(segments);project::validate(&serde_json::from_value::<ProjectDocument>(wire.clone()).unwrap()).unwrap();segments.push(segments[1023].clone());wire["audioTracks"]["system"]["timing"]["segments"]=json!(segments);assert!(project::validate(&serde_json::from_value::<ProjectDocument>(wire).unwrap()).is_err());
}

#[test]
fn canonical_short_track_and_preview_crop_use_real_signal() {
    use focusflow_core::{audio::{compile_audio,normalize},render::{self,ExportPreset},preview::{self,PreviewRequest}};
    for rate in [8000,192000] {
        let (_t,mut snapshot)=audio_fixture(0,rate,1);
        let mut writer=hound::WavWriter::create(snapshot.project_dir.join("system.wav"),hound::WavSpec{channels:1,sample_rate:rate,bits_per_sample:32,sample_format:hound::SampleFormat::Float}).unwrap();writer.write_sample(0.75_f32).unwrap();writer.finalize().unwrap();
        snapshot.document.video.duration_us=1_000_000;snapshot.document.clips[0].out_us=1_000_000;
        let plan=render::compile(&snapshot.document,ExportPreset::Balanced).unwrap();let audio=compile_audio(&snapshot,&plan,None,true).unwrap();
        let dir=common::TestDir::new();let launch=|_:&std::path::Path,args:Vec<std::ffi::OsString>|{let mut c=std::process::Command::new("/usr/bin/ffmpeg");c.args(args);c};
        let prepared=normalize::normalize(&audio,dir.path(),&||Ok(false),&launch).unwrap().unwrap();let bytes=std::fs::read(prepared.path()).unwrap();assert_eq!(bytes.len(),48000*8);
        let samples:Vec<_>=bytes.chunks_exact(4).map(|b|f32::from_le_bytes(b.try_into().unwrap())).collect();assert!(samples[0]>0.0,"short{rate} must retain signal");assert_eq!(samples[0],samples[1]);assert!(samples[..(48000_u64.div_ceil(u64::from(rate))*2)as usize].iter().all(|v|*v!=0.0),"Every real one-frame reconstruction sample must survive");assert!(samples[24..].iter().all(|v|*v==0.0));
        let selection=preview::select(&plan,&PreviewRequest::Video{start_us:0,end_us:100000}).unwrap();let audio=compile_audio(&snapshot,&plan,Some(&selection),true).unwrap();let other=common::TestDir::new();let crop=normalize::normalize(&audio,other.path(),&||Ok(false),&launch).unwrap().unwrap();assert_eq!(std::fs::read(crop.path()).unwrap(),bytes[..4800*8]);
    }
}

#[test]
fn mapped_drift_bounds_use_video_over_nominal_source_duration() {
    let mut wire=schema5();wire["audioTracks"]["system"]["timing"]["segments"][0]["videoStartUs"]=json!(0);
    for end in [995000,1005000]{wire["audioTracks"]["system"]["timing"]["segments"][0]["videoEndUs"]=json!(end);project::validate(&serde_json::from_value::<ProjectDocument>(wire.clone()).unwrap()).unwrap();}
    for end in [994999,1005001]{wire["audioTracks"]["system"]["timing"]["segments"][0]["videoEndUs"]=json!(end);assert!(project::validate(&serde_json::from_value::<ProjectDocument>(wire.clone()).unwrap()).unwrap_err().message.contains("video duration / nominal"));}
}

#[test]
fn schema5_render_argv_and_graph_share_private_raw_input() {
    use focusflow_core::{render::{self,ExportPreset},ffmpeg,audio::normalize::RAW_AUDIO};
    let (_t,s)=audio_fixture(0,48000,48000);let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();let graph=render::filter_graph(&s,&p,true).unwrap();let args=ffmpeg::render_args(&s,&p,&s.project_dir,true).unwrap();
    assert!(graph.contains("[1:a]"));assert!(args.iter().any(|a|a==&s.project_dir.join(RAW_AUDIO).into_os_string()));assert!(!args.iter().any(|a|a==&s.project_dir.join("system.wav").into_os_string()));
}

fn normalize_fixture(snapshot:&focusflow_core::store::RenderSnapshot,selection:Option<&focusflow_core::preview::PreviewSelection>)->(common::TestDir,Vec<u8>){
    use focusflow_core::{audio::{compile_audio,normalize},render::{self,ExportPreset}};
    let plan=render::compile(&snapshot.document,ExportPreset::Balanced).unwrap();let audio=compile_audio(snapshot,&plan,selection,true).unwrap();let dir=common::TestDir::new();let launch=|_:&std::path::Path,args:Vec<std::ffi::OsString>|{let mut c=std::process::Command::new("/usr/bin/ffmpeg");c.args(args);c};
    let prepared=normalize::normalize(&audio,dir.path(),&||Ok(false),&launch).unwrap().unwrap();let data=std::fs::read(prepared.path()).unwrap();drop(prepared);assert!(!dir.path().join("audio-scratch").exists());(dir,data)
}
#[test]
fn canonical_actual_seek_seam_and_fractional_clip_preview_match_full_pcm(){
    use focusflow_core::{render::{self,ExportPreset},preview::{self,PreviewRequest}};
    let (_t,mut s)=audio_fixture(0,192000,192000*22);
    s.document.video.duration_us=23_000_000;s.document.clips=vec![project::Clip{id:"long".into(),in_us:12345,out_us:22_100_000},project::Clip{id:"repeat".into(),in_us:12345,out_us:345678}];
    let t=s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap();t.timing=serde_json::from_value(json!({"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1000000,"provenance":"synthetic","segments":[{"generation":0,"sourceStartFrame":0,"sourceEndFrame":192000*22,"nativeStartTick":0,"nativeEndTick":22000000,"videoStartUs":0,"videoEndUs":22110000,"uncertaintyUs":0}]})).unwrap();
    let (_full_dir,full)=normalize_fixture(&s,None);let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    for (a,b) in [(19_900_000,20_200_000),(22_000_000,22_300_000)]{
        let selection=preview::select(&p,&PreviewRequest::Video{start_us:a,end_us:b}).unwrap();let(_crop_dir,crop)=normalize_fixture(&s,Some(&selection));let first=(selection.first_frame*48000).div_ceil(30) as usize;assert_eq!(crop,full[first*8..first*8+crop.len()]);
    }
}
#[test]
fn two_track_normalization_mix_and_corrupt_sibling(){
    let (_t,mut s)=audio_fixture(250000,48000,24000);s.document.video.duration_us=1_000_000;s.document.clips[0].out_us=1_000_000;
    for (name,value,frames) in [("system.wav",0.2_f32,24000),("mic.wav",0.8_f32,48000)]{
        let mut w=hound::WavWriter::create(s.project_dir.join(name),hound::WavSpec{channels:1,sample_rate:48000,bits_per_sample:32,sample_format:hound::SampleFormat::Float}).unwrap();for _ in 0..frames{w.write_sample(value).unwrap();}w.finalize().unwrap();
    }
    let mut mic=s.document.audio_tracks.as_ref().unwrap().system.clone().unwrap();mic.path="mic.wav".into();mic.frame_count=48000;mic.timing=project::AudioTrackTiming::Unknown{};s.document.audio_tracks.as_mut().unwrap().microphone=Some(mic);
    let (_d,raw)=normalize_fixture(&s,None);let at=|data:&[u8],n:usize|f32::from_le_bytes(data[n*8..n*8+4].try_into().unwrap());
    for (n,expected) in [(6000,0.4),(24000,0.5),(42000,0.4)]{assert!((at(&raw,n)-expected).abs()<0.0001,"{n} {}",at(&raw,n));}
    std::fs::write(s.project_dir.join("mic.wav"),b"corrupt sibling").unwrap();let(_d,raw)=normalize_fixture(&s,None);assert!((at(&raw,24000)-0.2).abs()<0.0001);assert_eq!(at(&raw,6000),0.0);
    std::fs::write(s.project_dir.join("system.wav"),b"corrupt").unwrap();let p=focusflow_core::render::compile(&s.document,focusflow_core::render::ExportPreset::Balanced).unwrap();assert!(focusflow_core::audio::compile_audio(&s,&p,None,true).unwrap().tracks.is_empty());
}

#[test]
fn adjacent_fractional_split_preserves_pcm_and_removed_source_remains_zero_boundary(){
    let (_t,mut s)=audio_fixture(0,44100,88200);s.document.video.duration_us=2_000_000;s.document.clips[0].out_us=2_000_000;
    let (_d,unsplit)=normalize_fixture(&s,None);
    s.document.clips=vec![project::Clip{id:"left".into(),in_us:0,out_us:333333},project::Clip{id:"right".into(),in_us:333333,out_us:2_000_000}];
    let (_d,split)=normalize_fixture(&s,None);assert!(unsplit==split,"Administrative source-contiguous fractional split must preserve PCM");
    s.document.clips[1].in_us=666667;let(_d,cut)=normalize_fixture(&s,None);
    let mut w=hound::WavWriter::create(s.project_dir.join("system.wav"),hound::WavSpec{channels:1,sample_rate:44100,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for n in 0..88200{w.write_sample(if(14700..29401).contains(&n){20000_i16}else{(n%123)as i16}).unwrap();}w.finalize().unwrap();
    let(_d,changed)=normalize_fixture(&s,None);assert!(cut==changed,"Samples removed by a real cut cannot leak through reconstruction support");
}

#[test]
fn mapped_pipeline_markers_follow_all_accepted_drift_slopes_and_unknown_stays_nominal(){
    let(_t,mut s)=audio_fixture(0,48000,96000);s.document.video.duration_us=3_000_000;s.document.clips[0].out_us=3_000_000;
    let markers=[17_u32,12000,47999,95982];let mut w=hound::WavWriter::create(s.project_dir.join("system.wav"),hound::WavSpec{channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for n in 0..96000{w.write_sample(if markers.contains(&n){24576_i16}else{0}).unwrap();}w.finalize().unwrap();
    for ppm in [-5000_i64,-1000,1000,5000]{
        s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(json!({"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1000000,"provenance":"synthetic","segments":[{"generation":0,"sourceStartFrame":0,"sourceEndFrame":96000,"nativeStartTick":0,"nativeEndTick":2000000,"videoStartUs":123,"videoEndUs":123+2000000+2*ppm,"uncertaintyUs":0}]})).unwrap();
        let(_d,data)=normalize_fixture(&s,None);let at=|n:usize|f32::from_le_bytes(data[n*8..n*8+4].try_into().unwrap()).abs();
        for marker in markers{let expected=marker as f64*(1.0+ppm as f64/1e6)+123.0*48000.0/1e6;let center=expected.round()as usize;let peak=(center.saturating_sub(8)..center+9).max_by(|a,b|at(*a).total_cmp(&at(*b))).unwrap();assert!((peak as f64-expected).abs()<=2.0,"ppm{ppm} marker{marker} peak{peak} expected{expected}");}
    }
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=project::AudioTrackTiming::Unknown{};let(_d,data)=normalize_fixture(&s,None);for marker in markers{assert!(f32::from_le_bytes(data[marker as usize*8..marker as usize*8+4].try_into().unwrap())>0.7);}
}
#[test]
fn mapped_generation_restart_and_real_gap_do_not_borrow_context(){
    let(_t,mut s)=audio_fixture(0,44100,88200);s.document.video.duration_us=3_000_000;s.document.clips[0].out_us=3_000_000;
    let timing=json!({"type":"mapped","clockDomain":"monotonic","ticksPerSecond":1000000,"provenance":"native","segments":[{"generation":0,"sourceStartFrame":0,"sourceEndFrame":44100,"nativeStartTick":1000,"nativeEndTick":1001000,"videoStartUs":0,"videoEndUs":1000000,"uncertaintyUs":99},{"generation":1,"sourceStartFrame":44100,"sourceEndFrame":88200,"nativeStartTick":0,"nativeEndTick":1000000,"videoStartUs":1250000,"videoEndUs":2250000,"uncertaintyUs":101}]});
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(timing).unwrap();let(_d,original)=normalize_fixture(&s,None);assert!(original[48000*8..60000*8].iter().all(|b|*b==0));
    let mut w=hound::WavWriter::create(s.project_dir.join("system.wav"),hound::WavSpec{channels:1,sample_rate:44100,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for n in 0..88200{w.write_sample(if n<44100{20000_i16}else{(n%123)as i16}).unwrap();}w.finalize().unwrap();let(_d,changed)=normalize_fixture(&s,None);assert!(original[60000*8..]==changed[60000*8..],"Prior generation samples cannot leak across the real gap/restart");
}

#[test]
fn canonical_pcm_formats_preserve_stereo_and_duplicate_mono_exactly(){
 for format in ["s16","s24","s32","f32"]{for channels in [1_u16,2]{
    let(_t,mut s)=audio_fixture(0,48000,48);s.document.video.duration_us=1000;s.document.clips[0].out_us=1000;s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().channels=channels;
    let bits=match format{"s16"=>16,"s24"=>24,_=>32};let sample_format=if format=="f32"{hound::SampleFormat::Float}else{hound::SampleFormat::Int};
    let mut w=hound::WavWriter::create(s.project_dir.join("system.wav"),hound::WavSpec{channels,sample_rate:48000,bits_per_sample:bits,sample_format}).unwrap();for _ in 0..48{for channel in 0..channels{let value=if channel==0{0.5_f32}else{-0.25};match format{"s16"=>w.write_sample((value*32768.0)as i16).unwrap(),"s24"=>w.write_sample((value*8388608.0)as i32).unwrap(),"s32"=>w.write_sample((value*2147483648.0)as i32).unwrap(),_=>w.write_sample(value).unwrap()}}}w.finalize().unwrap();
    let(_d,data)=normalize_fixture(&s,None);assert_eq!(data.len(),48*8);for frame in data.chunks_exact(8){assert_eq!(f32::from_le_bytes(frame[..4].try_into().unwrap()),0.5,"{format}");assert_eq!(f32::from_le_bytes(frame[4..].try_into().unwrap()),if channels==1{0.5}else{-0.25},"{format}");}
 }}
}
#[test]
fn one_hour_logical_mapping_preview_matches_local_reference_without_long_media(){
    use focusflow_core::{render::{self,ExportPreset},preview::{self,PreviewRequest}};
    let(_t,mut s)=audio_fixture(0,48000,96000);s.document.video.duration_us=3_603_000_000;s.document.clips[0].out_us=3_603_000_000;
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(json!({"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1000000,"provenance":"synthetic","segments":[{"generation":0,"sourceStartFrame":0,"sourceEndFrame":96000,"nativeStartTick":3600000000_u64,"nativeEndTick":3602000000_u64,"videoStartUs":3600000000_i64,"videoEndUs":3602000000_i64,"uncertaintyUs":0}]})).unwrap();
    let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();let selection=preview::select(&p,&PreviewRequest::Video{start_us:3_600_100_000,end_us:3_601_900_000}).unwrap();let(_d,crop)=normalize_fixture(&s,Some(&selection));
    s.document.clips=vec![project::Clip{id:"local".into(),in_us:3_600_000_000,out_us:3_602_000_000}];let(_d,reference)=normalize_fixture(&s,None);assert!(crop==reference[4800*8..91200*8]);
}
#[test]
fn normalization_cancels_before_children_and_during_bounded_host_copies(){
    use focusflow_core::{audio::{compile_audio,normalize},render::{self,ExportPreset},ErrorCode};
    let(_t,s)=audio_fixture(0,48000,96000);let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();let audio=compile_audio(&s,&p,None,true).unwrap();
    for stage in ["before","context","final"]{
        let dir=common::TestDir::new();let calls=std::sync::atomic::AtomicU64::new(0);let launch=|_:&std::path::Path,args:Vec<std::ffi::OsString>|{calls.fetch_add(1,std::sync::atomic::Ordering::Relaxed);let mut c=std::process::Command::new("/usr/bin/ffmpeg");c.args(args);c};
        let cancel=||Ok(stage=="before" || std::fs::metadata(dir.path().join("audio-scratch").join(if stage=="context"{"context.f32"}else{"final.f32"})).is_ok_and(|m|m.len()>0));
        let error=match normalize::normalize(&audio,dir.path(),&cancel,&launch){Err(e)=>e,_=>panic!("expected cancellation")};assert_eq!(error.code,ErrorCode::Cancelled);assert!(!dir.path().join("audio-scratch").exists());assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed),match stage{"before"=>0,"context"=>1,_=>2});
    }
}

#[test]
fn audio_stage_rejects_actual_short_and_long_encoder_output_without_padding(){
    use focusflow_core::{audio::{compile_audio,normalize},render::{self,ExportPreset},ErrorCode};
    for end in [12000,48000]{
        let(_t,mut s)=audio_fixture(0,48000,96000);s.document.clips[0].out_us=500000;let before=std::fs::read(s.project_dir.join("system.wav")).unwrap();let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();let audio=compile_audio(&s,&p,None,true).unwrap();let dir=common::TestDir::new();let calls=std::sync::atomic::AtomicU64::new(0);
        let launch=|work:&std::path::Path,args:Vec<std::ffi::OsString>|{calls.fetch_add(1,std::sync::atomic::Ordering::Relaxed);let script=work.join("stage.filter");let text=std::fs::read_to_string(&script).unwrap();assert!(text.contains("end_pts=24000"));std::fs::write(&script,text.replace("end_pts=24000",&format!("end_pts={end}"))).unwrap();let mut c=std::process::Command::new("/usr/bin/ffmpeg");c.args(args);c};
        let error=match normalize::normalize(&audio,dir.path(),&||Ok(false),&launch){Err(e)=>e,_=>panic!("wrong output length accepted")};assert_eq!(error.code,ErrorCode::EncoderFailed);assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed),1);assert!(!dir.path().join("audio-scratch").exists());assert_eq!(std::fs::read(s.project_dir.join("system.wav")).unwrap(),before);
    }
}

#[test]
fn contiguous_affine_segments_coalesce_but_generation_or_slope_changes_keep_zero_boundaries(){
    let(_t,mut s)=audio_fixture(0,44100,88200);s.document.video.duration_us=3_000_000;s.document.clips[0].out_us=3_000_000;let(_d,nominal)=normalize_fixture(&s,None);
    let base=json!({"type":"mapped","clockDomain":"synthetic","ticksPerSecond":1000000,"provenance":"synthetic","segments":[{"generation":0,"sourceStartFrame":0,"sourceEndFrame":22050,"nativeStartTick":0,"nativeEndTick":500000,"videoStartUs":0,"videoEndUs":500000,"uncertaintyUs":0},{"generation":0,"sourceStartFrame":22050,"sourceEndFrame":88200,"nativeStartTick":500000,"nativeEndTick":2000000,"videoStartUs":500000,"videoEndUs":2000000,"uncertaintyUs":0}]});
    s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(base.clone()).unwrap();let(_d,coalesced)=normalize_fixture(&s,None);assert!(nominal==coalesced,"Continuous same-generation affine anchors must not create a reconstruction boundary");
    for change in ["generation","slope"]{
        let mut timing=base.clone();if change=="generation"{timing["segments"][1]["generation"]=json!(1);timing["segments"][1]["nativeStartTick"]=json!(0);timing["segments"][1]["nativeEndTick"]=json!(1500000);}else{timing["segments"][1]["videoEndUs"]=json!(2001500);}
        s.document.audio_tracks.as_mut().unwrap().system.as_mut().unwrap().timing=serde_json::from_value(timing).unwrap();
        let write=|changed:bool|{let mut w=hound::WavWriter::create(s.project_dir.join("system.wav"),hound::WavSpec{channels:1,sample_rate:44100,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for n in 0..88200{w.write_sample(if changed && n<22050{20000_i16}else{(n%123)as i16}).unwrap();}w.finalize().unwrap();};
        write(false);let(_d,original)=normalize_fixture(&s,None);write(true);let(_d,changed)=normalize_fixture(&s,None);assert!(original[24000*8..]==changed[24000*8..],"Distinct {change} domain cannot borrow prior source samples");
    }
}
