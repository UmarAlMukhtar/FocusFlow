mod common;
use focusflow_core::{project::*, render::{self,ExportPreset},store::RenderSnapshot};
#[test]
fn enabled_cursor_cannot_silently_use_single_pass() {
 let temp=common::TestDir::new(); common::write_legacy(temp.path());
 let mut doc=common::fixture_project(); doc.schema_version=3; doc.video.cursor=CursorMode::Separate;
 doc.cursor=Some(CursorAsset{index_path:"cursor.json".into()}); doc.cursor_settings.enabled=true;
 let s=RenderSnapshot{project_dir:temp.path().into(),document:doc,prepared_focus:None,prepared_cursor:None};
 let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();
 assert!(render::filter_graph(&s,&p,false).is_err(),"enabled cursor must never silently disappear");
}
use std::{fs,path::Path,process::Command};
use focusflow_core::{jobs::{JobManager,JobState},preview::PreviewRequest};
fn run(program:&str,args:&[&str])->Vec<u8>{let o=Command::new(program).args(args).output().unwrap();assert!(o.status.success(),"{}",String::from_utf8_lossy(&o.stderr));o.stdout}
fn media_fixture()->(common::TestDir,std::path::PathBuf,ProjectDocument){
 let temp=common::TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();common::write_legacy(&dir);
 run("ffmpeg",&["-v","error","-y","-f","lavfi","-i","color=black:s=100x50:r=60:d=5","-c:v","libx264","-preset","ultrafast",dir.join("screen.mp4").to_str().unwrap()]);
 let mut doc=common::fixture_project();doc.schema_version=3;doc.video.cursor=CursorMode::Separate;doc.cursor=Some(CursorAsset{index_path:"index.json".into()});doc.cursor_settings.enabled=true;
 doc.clips=vec![Clip{id:"a".into(),in_us:12345,out_us:1_012_346},Clip{id:"b".into(),in_us:12345,out_us:1_012_346}];
 fs::write(dir.join("index.json"),serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5_000_000,"samplesPath":"samples.ndjson","shapes":[{"id":"a","path":"a.pam","width":8,"height":8,"hotspotX":4,"hotspotY":4},{"id":"b","path":"b.pam","width":8,"height":8,"hotspotX":4,"hotspotY":4}]}).to_string()).unwrap();
 let samples=(0..50).map(|i|serde_json::json!({"atUs":i*100000,"x":0.5,"y":0.5,"visible":i%5!=3,"shapeId":if i%10>=5{"b"}else{"a"},"discontinuity":false}).to_string()+"\n").collect::<String>();fs::write(dir.join("samples.ndjson"),samples).unwrap();
 let mut pam=b"P7\nWIDTH 8\nHEIGHT 8\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();pam.extend([255;256]);fs::write(dir.join("a.pam"),&pam).unwrap();for pixel in pam[b"P7\nWIDTH 8\nHEIGHT 8\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".len()..].chunks_exact_mut(4){pixel.copy_from_slice(&[255,0,0,255]);}fs::write(dir.join("b.pam"),pam).unwrap();
 fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();(temp,dir,doc)
}
#[test]fn production_cursor_grid_visibility_and_previews(){
 let(temp,dir,_)=media_fixture();let m=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();
 for preset in [ExportPreset::Balanced,ExportPreset::HighQuality]{
 let j=m.start_export("fixture",0,preset).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let path=dir.join(j.output_relative_path.unwrap());let raw=run("ffmpeg",&["-v","error","-i",path.to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);
 let fps=preset.fps();let count=(2_000_002_u64*fps as u64).div_ceil(1_000_000) as usize;assert_eq!(raw.len(),count*100*50*3);
 for k in 0..count {let global=k as u64*1_000_000;let offset=if global>=1_000_001*fps as u64{1_000_001}else{0};let source=(12345*fps as u64+global-offset*fps as u64)/fps as u64;let visible=(source/100000)%5!=3;let pixel=&raw[k*15000+(25*100+50)*3..][..3];assert_eq!(pixel[0]>150,visible,"fps{fps} frame{k} source{source} pixel{pixel:?}");if visible{assert_eq!(pixel[1]>150,(source/100000)%10<5,"shape at fps{fps} frame{k}: {pixel:?}");}}
 let pts:serde_json::Value=serde_json::from_slice(&run("ffprobe",&["-v","error","-select_streams","v","-show_packets","-of","json",path.to_str().unwrap()])).unwrap();
 for(k,p)in pts["packets"].as_array().unwrap().iter().enumerate(){assert_eq!(p["pts"].as_i64().unwrap(),k as i64*(90000/fps) as i64);}
 }
 let j=m.start_preview("fixture",0,PreviewRequest::Still{at_us:400000}).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let raw=run("ffmpeg",&["-v","error","-i",dir.join(j.output_relative_path.unwrap()).to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);assert_eq!(&raw[(25*100+50)*3..][..3],&[255,255,255]);
 let j=m.start_preview("fixture",0,PreviewRequest::Video{start_us:33333,end_us:1_400_000}).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
}
#[test]fn production_audio_and_preparation_failures(){
 for audio in ["valid","short","missing","corrupt"]{
 let(temp,dir,mut doc)=media_fixture();doc.microphone=Some(AudioAsset{path:"mic.wav".into()});
 if audio=="valid"||audio=="short"{let mut w=hound::WavWriter::create(dir.join("mic.wav"),hound::WavSpec{channels:1,sample_rate:8000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for _ in 0..if audio=="short"{500}else{40000}{w.write_sample(1000_i16).unwrap();}w.finalize().unwrap();}
 if audio=="corrupt"{fs::write(dir.join("mic.wav"),b"bad wav").unwrap();}
 fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
 let m=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();
 for preview in [false,true]{let j=if preview{m.start_preview("fixture",0,PreviewRequest::Video{start_us:100000,end_us:1_900_000})}else{m.start_export("fixture",0,ExportPreset::Balanced)}.unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{audio} {:?}",j.error);
 let p:serde_json::Value=serde_json::from_slice(&run("ffprobe",&["-v","error","-show_streams","-of","json",dir.join(j.output_relative_path.unwrap()).to_str().unwrap()])).unwrap();let streams=p["streams"].as_array().unwrap();assert_eq!(streams.iter().any(|s|s["codec_name"]=="aac"),audio=="valid"||audio=="short");let video=streams.iter().find(|s|s["codec_type"]=="video").unwrap();assert_eq!(video["nb_frames"].as_str().unwrap(),if preview{"54"}else{"61"});}
 }
 let(temp,dir,_)=media_fixture();fs::write(dir.join("samples.ndjson"),b"bad").unwrap();let m=JobManager::new(temp.path(),Path::new("absent-encoder")).unwrap();let j=m.start_export("fixture",0,ExportPreset::Balanced).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Failed);assert!(!j.warnings.iter().any(|w|w.contains("retrying")));
}
#[test]fn alpha_hotspot_offcrop_and_styled_preview_match_export(){
 let(temp,dir,mut doc)=media_fixture();doc.clips=vec![Clip{id:"clip-0".into(),in_us:0,out_us:1_000_000}];
 let mut pam=b"P7\nWIDTH 8\nHEIGHT 8\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();for _ in 0..64{pam.extend([255,255,255,128]);}fs::write(dir.join("a.pam"),pam).unwrap();
 fs::write(dir.join("samples.ndjson"),serde_json::json!({"atUs":0,"x":-0.02,"y":0.5,"visible":true,"shapeId":"a","discontinuity":false}).to_string()).unwrap();fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
 let m=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();let j=m.start_preview("fixture",0,PreviewRequest::Still{at_us:0}).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let raw=run("ffmpeg",&["-v","error","-i",dir.join(j.output_relative_path.unwrap()).to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);
 let pixel=|x:usize,y:usize|raw[(y*100+x)*3];assert!((pixel(0,24) as i16-128).abs()<=4);assert!(pixel(4,24)<4);assert!(pixel(0,19)<4);
 // Matching source history with smoothing, a fixed camera and canvas styling.
 doc.cursor_settings.smoothing_us=200000;doc.frame_style.padding_px=4;doc.frame_style.background=FrameBackground::Solid{color:"#123456".into()};
 doc.zooms=vec![ZoomElement{mode:ZoomMode::Fixed,id:"z".into(),clip_id:"clip-0".into(),start_us:0,end_us:800000,x:0.5,y:0.5,scale:1.5,intro_us:200000,hold_us:0,outro_us:200000}];
 fs::write(dir.join("samples.ndjson"),(0..10).map(|i|serde_json::json!({"atUs":i*100000,"x":0.3+i as f64*0.04,"y":0.5,"visible":true,"shapeId":"a","discontinuity":false}).to_string()+"\n").collect::<String>()).unwrap();
 fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
 let j=m.start_export("fixture",0,ExportPreset::Balanced).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let export=run("ffmpeg",&["-v","error","-i",dir.join(j.output_relative_path.unwrap()).to_str().unwrap(),"-vf","select=eq(n\\,12)","-frames:v","1","-f","rawvideo","-pix_fmt","rgb24","-"]);
 let j=m.start_preview("fixture",0,PreviewRequest::Still{at_us:400000}).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let preview=run("ffmpeg",&["-v","error","-i",dir.join(j.output_relative_path.unwrap()).to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);assert_eq!(preview.len(),export.len());let mean=preview.iter().zip(&export).map(|(a,b)|a.abs_diff(*b)as f64).sum::<f64>()/export.len()as f64;assert!(mean<3.0,"mean encoded RGB difference {mean}");
}
#[test]fn production_visibility_and_shape_at_nine_hundred_frame_window_join(){
 let(temp,dir,mut doc)=media_fixture();doc.video.duration_us=31_000_000;doc.clips=vec![Clip{id:"long".into(),in_us:0,out_us:31_000_000}];
 run("ffmpeg",&["-v","error","-y","-f","lavfi","-i","color=black:s=100x50:r=30:d=31","-c:v","libx264","-preset","ultrafast",dir.join("screen.mp4").to_str().unwrap()]);
 let mut index:serde_json::Value=serde_json::from_slice(&fs::read(dir.join("index.json")).unwrap()).unwrap();index["durationUs"]=31_000_000.into();fs::write(dir.join("index.json"),index.to_string()).unwrap();
 fs::write(dir.join("samples.ndjson"),(0..310).map(|i|serde_json::json!({"atUs":i*100000,"x":if i>=300{0.7}else{0.3},"y":0.5,"visible":i!=299,"shapeId":if i==300{"b"}else{"a"},"discontinuity":false}).to_string()+"\n").collect::<String>()).unwrap();fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
 let m=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();let j=m.start_export("fixture",0,ExportPreset::Balanced).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let path=dir.join(j.output_relative_path.unwrap());let raw=run("ffmpeg",&["-v","error","-i",path.to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);assert_eq!(raw.len(),930*15000);
 for(k,x,expected)in [(896,30,[255,255,255]),(899,30,[0,0,0]),(900,70,[255,0,0]),(903,70,[255,255,255])]{let p=&raw[k*15000+(25*100+x)*3..][..3];for(a,b)in p.iter().zip(expected){assert!(a.abs_diff(b)<20,"frame{k}: {p:?}");}}
 let pts:serde_json::Value=serde_json::from_slice(&run("ffprobe",&["-v","error","-select_streams","v","-show_packets","-of","json",path.to_str().unwrap()])).unwrap();for(k,p)in pts["packets"].as_array().unwrap().iter().enumerate(){assert_eq!(p["pts"].as_i64().unwrap(),k as i64*3000);}
}
#[test]fn lossless_rgba_compositing_isolates_alpha_and_hotspot(){
 let(_temp,dir,doc)=media_fixture();
 let mut pam=b"P7\nWIDTH 8\nHEIGHT 8\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();for _ in 0..64{pam.extend([255,255,255,128]);}fs::write(dir.join("a.pam"),pam).unwrap();
 fs::write(dir.join("samples.ndjson"),serde_json::json!({"atUs":0,"x":0.0,"y":0.5,"visible":true,"shapeId":"a","discontinuity":false}).to_string()).unwrap();
 let cancel=std::sync::atomic::AtomicBool::new(false);let mut snapshot=RenderSnapshot{project_dir:dir.clone(),document:doc,prepared_focus:None,prepared_cursor:None};snapshot.prepare_cursor(&cancel).unwrap();let plan=render::compile(&snapshot.document,ExportPreset::Balanced).unwrap();let window=render::cursor_window::next(&snapshot,&plan,0,1,&cancel).unwrap();
 fs::write(dir.join("atlas.pam"),window.atlas.unwrap()).unwrap();let graph=format!("[0:v]format=rgba[vc0];{}",window.graph);fs::write(dir.join("composite.filter"),graph).unwrap();
 let raw=run("ffmpeg",&["-v","error","-f","lavfi","-i","color=black:s=100x50:r=30:d=1","-f","image2","-framerate","30","-i",dir.join("atlas.pam").to_str().unwrap(),"-filter_complex_threads","1","-filter_complex_script",dir.join("composite.filter").to_str().unwrap(),"-map","[cursor_composed]","-frames:v","1","-f","rawvideo","-pix_fmt","rgba","-"]);
 assert_eq!(&raw[(25*100)*4..][..4],&[128,128,128,255]);assert_eq!(&raw[(25*100+3)*4..][..4],&[128,128,128,255]);assert_eq!(&raw[(25*100+4)*4..][..4],&[0,0,0,255]);
}
#[test]fn preparation_callback_observes_remote_cancel_at_record_boundary_and_preserves_errors(){
 let(_temp,dir,doc)=media_fixture();let records=(0..4096).map(|i|serde_json::json!({"atUs":i,"x":0.5,"y":0.5,"visible":true,"shapeId":"a","discontinuity":false}).to_string()+"\n").collect::<String>()+"malformed record after checkpoint\n";fs::write(dir.join("samples.ndjson"),records).unwrap();
 let calls=std::cell::Cell::new(0);let callback=||{let n=calls.get()+1;calls.set(n);if n==2{fs::write(dir.join("cancel"),b"1").unwrap();return Ok(false);}Ok(dir.join("cancel").exists())};
 let e=focusflow_core::cursor::prepare_cursor_with_cancel(&dir,&doc,&callback).unwrap_err();assert_eq!(e.code,focusflow_core::ErrorCode::Cancelled);assert_eq!(calls.get(),3);
 let e=focusflow_core::cursor::prepare_cursor_with_cancel(&dir,&doc,&||Err(focusflow_core::CoreError{code:focusflow_core::ErrorCode::Io,message:"cancel-file read failed".into(),field:None})).unwrap_err();assert_eq!(e.message,"cancel-file read failed");
}
#[cfg(unix)]
#[test]fn forced_aac_failure_publishes_real_completed_video_without_reencoding(){
 use std::os::unix::fs::PermissionsExt;
 let(temp,dir,mut doc)=media_fixture();let mut wav=hound::WavWriter::create(dir.join("mic.wav"),hound::WavSpec{channels:1,sample_rate:8000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for _ in 0..8000{wav.write_sample(500_i16).unwrap();}wav.finalize().unwrap();doc.microphone=Some(AudioAsset{path:"mic.wav".into()});fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
 let wrapper=temp.path().join("aac-failure.sh");fs::write(&wrapper,b"#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$0.log\"\ncase \" $* \" in *' -c:a aac '*) exit 42;; esac\nexec ffmpeg \"$@\"\n").unwrap();fs::set_permissions(&wrapper,fs::Permissions::from_mode(0o700)).unwrap();
 let m=JobManager::new(temp.path(),&wrapper).unwrap();let j=m.start_export("fixture",0,ExportPreset::Balanced).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);assert!(j.warnings.iter().any(|w|w.contains("without audio using completed video")));
 let p:serde_json::Value=serde_json::from_slice(&run("ffprobe",&["-v","error","-show_streams","-of","json",dir.join(j.output_relative_path.unwrap()).to_str().unwrap()])).unwrap();let streams=p["streams"].as_array().unwrap();assert_eq!(streams.len(),1);assert_eq!(streams[0]["nb_frames"],"61");
 let log=fs::read_to_string(temp.path().join("aac-failure.sh.log")).unwrap();assert_eq!(log.lines().count(),4);assert_eq!(log.lines().filter(|line|line.contains("-c:v libx264")).count(),2);
}
#[test]
fn follow_without_overlay_centers_generated_markers_across_windows_and_seeks() {
 let(temp,dir,mut doc)=media_fixture();doc.schema_version=4;doc.cursor_settings.enabled=false;doc.video.duration_us=31_000_000;doc.clips=vec![Clip{id:"follow".into(),in_us:0,out_us:31_000_000}];
 doc.zooms=vec![ZoomElement{mode:ZoomMode::Follow,id:"follow".into(),clip_id:"follow".into(),start_us:0,end_us:31_000_000,x:0.5,y:0.5,scale:2.0,intro_us:0,hold_us:0,outro_us:0}];
 run("ffmpeg",&["-v","error","-y","-f","lavfi","-i","color=black:s=100x50:r=60:d=31,drawbox=x=22:y=22:w=6:h=6:c=red:t=fill,drawbox=x=72:y=22:w=6:h=6:c=blue:t=fill","-c:v","libx264","-preset","ultrafast","-crf","10",dir.join("screen.mp4").to_str().unwrap()]);
 let mut index:serde_json::Value=serde_json::from_slice(&fs::read(dir.join("index.json")).unwrap()).unwrap();index["durationUs"]=31_000_000.into();fs::write(dir.join("index.json"),index.to_string()).unwrap();
 fs::remove_file(dir.join("a.pam")).unwrap();fs::remove_file(dir.join("b.pam")).unwrap();
 fs::write(dir.join("samples.ndjson"),(0..310).map(|i|serde_json::json!({"atUs":i*100000,"x":if i>=150{0.75}else{0.25},"y":0.5,"visible":true,"shapeId":"a","discontinuity":true}).to_string()+"\n").collect::<String>()).unwrap();
 fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
 let before:Vec<_>=["screen.mp4","project.json","index.json","samples.ndjson"].map(|p|(p,fs::read(dir.join(p)).unwrap())).into();
 let m=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();
 for preset in [ExportPreset::Balanced,ExportPreset::HighQuality] {
  let j=m.start_export("fixture",0,preset).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
  let path=dir.join(j.output_relative_path.unwrap());let raw=run("ffmpeg",&["-v","error","-i",path.to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);let fps=preset.fps() as usize;assert_eq!(raw.len(),31*fps*15000);
  for k in [0,224,225,449,450,899,900,31*fps-1] {let p=&raw[k*15000+(25*100+50)*3..][..3];if k<15*fps {assert!(p[0]>180&&p[2]<40,"red marker at frame{k}: {p:?}");}else{assert!(p[2]>180&&p[0]<40,"blue marker at frame{k}: {p:?}");}}
  let pts:serde_json::Value=serde_json::from_slice(&run("ffprobe",&["-v","error","-select_streams","v","-show_packets","-of","json",path.to_str().unwrap()])).unwrap();for(k,p)in pts["packets"].as_array().unwrap().iter().enumerate(){assert_eq!(p["pts"].as_i64().unwrap(),k as i64*(90000/fps) as i64);}
  assert!(!fs::read_dir(path.parent().unwrap()).unwrap().any(|e|e.unwrap().file_name().to_string_lossy().starts_with("cursor-")));
 }
 for at in [7_533_333,15_000_000,30_100_000] {
  let j=m.start_preview("fixture",0,PreviewRequest::Still{at_us:at}).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
  let raw=run("ffmpeg",&["-v","error","-i",dir.join(j.output_relative_path.unwrap()).to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);let p=&raw[(25*100+50)*3..][..3];assert!(p[if at<15_000_000{0}else{2}]>180,"{at}: {p:?}");
 }
 let j=m.start_preview("fixture",0,PreviewRequest::Video{start_us:14_900_001,end_us:15_100_000}).unwrap();let j=m.wait("fixture",&j.job_id).unwrap();assert_eq!(j.state,JobState::Completed,"{:?}",j.error);
 let raw=run("ffmpeg",&["-v","error","-i",dir.join(j.output_relative_path.unwrap()).to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]);assert_eq!(raw.len(),5*15000);for k in 0..5 {assert!(raw[k*15000+(25*100+50)*3+if k<2{0}else{2}]>180);}
 for(p,b)in before {assert_eq!(fs::read(dir.join(p)).unwrap(),b);}
}

// Changing cursor visibility must never change source conversion or camera resampling.
#[test]
fn production_cursor_pixels_only_change_inside_resampled_glyph() {
 let temp=common::TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();common::write_legacy(&dir);
 let mut ppm=b"P6\n192 128\n255\n".to_vec();
 for y in 0..128 {for x in 0..192 {ppm.extend([[255,12,28],[8,238,47],[24,41,255],[248,215,11]][(x/3+y/5)%4]);}}
 fs::write(dir.join("edges.ppm"),ppm).unwrap();
 run("ffmpeg",&["-v","error","-y","-loop","1","-i",dir.join("edges.ppm").to_str().unwrap(),"-t","1","-r","30","-c:v","libx264","-crf","0","-pix_fmt","yuv420p",dir.join("screen.mp4").to_str().unwrap()]);
 let mut doc=common::fixture_project();doc.schema_version=4;doc.video.width=192;doc.video.height=128;doc.video.duration_us=1_000_000;doc.video.cursor=CursorMode::Separate;
 doc.cursor=Some(CursorAsset{index_path:"index.json".into()});doc.clips=vec![Clip{id:"clip-0".into(),in_us:0,out_us:1_000_000}];
 fs::write(dir.join("index.json"),serde_json::json!({"schemaVersion":1,"width":192,"height":128,"durationUs":1_000_000,"samplesPath":"samples.ndjson","shapes":[{"id":"native","path":"native.pam","width":8,"height":8,"hotspotX":4,"hotspotY":4}]}).to_string()).unwrap();
 fs::write(dir.join("samples.ndjson"),(0..10).map(|i|serde_json::json!({"atUs":i*100_000,"x":0.0,"y":0.5,"visible":true,"shapeId":"native","discontinuity":false}).to_string()+"\n").collect::<String>()).unwrap();
 let manager=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();
 for transparent in [false,true] {
  let mut pam=b"P7\nWIDTH 8\nHEIGHT 8\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
  for y in 0..8 {for x in 0..8 {pam.extend([247,29,173,if transparent{0}else if x==0||y==0||x==7||y==7{128}else{255}]);}}
  fs::write(dir.join("native.pam"),pam).unwrap();
  let assets=["screen.mp4","index.json","samples.ndjson","native.pam"].map(|name|(name,fs::read(dir.join(name)).unwrap()));
  for mode in [Some(ZoomMode::Follow),Some(ZoomMode::Fixed),None] {for click in [false,true] {for styled in [false,true] {
   doc.zooms=mode.map(|mode|ZoomElement{mode,id:"zoom".into(),clip_id:"clip-0".into(),start_us:0,end_us:1_000_000,x:0.0,y:0.5,scale:2.0,intro_us:0,hold_us:0,outro_us:0}).into_iter().collect();
   doc.clicks=if click{vec![Click{at_us:300_000,x:0.25,y:0.5}]}else{vec![]};
   doc.frame_style=Default::default();if styled{doc.frame_style.padding_px=4;doc.frame_style.corner_radius_px=4;doc.frame_style.shadow=true;doc.frame_style.background=FrameBackground::Solid{color:"#123456".into()};}
   let mut results=Vec::new();let mut cameras=Vec::new();
   for enabled in [false,true] {
    doc.cursor_settings.enabled=enabled;fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
    let mut snapshot=RenderSnapshot{project_dir:dir.clone(),document:doc.clone(),prepared_focus:None,prepared_cursor:None};let plan=render::compile(&doc,ExportPreset::Balanced).unwrap();snapshot.prepare_cursor(&std::sync::atomic::AtomicBool::new(false)).unwrap();snapshot.prepare_focus(&plan,&||Ok(false)).unwrap();
    if mode==Some(ZoomMode::Follow){cameras.push(render::cursor_window::next(&snapshot,&plan,12,13,&std::sync::atomic::AtomicBool::new(false)).unwrap().camera);}
    let job=manager.start_preview("fixture",0,PreviewRequest::Still{at_us:400_000}).unwrap();let job=manager.wait("fixture",&job.job_id).unwrap();assert_eq!(job.state,JobState::Completed,"{:?}",job.error);
    let metadata=job.output_metadata.as_ref().unwrap();assert_eq!((metadata.width,metadata.height,metadata.first_frame,metadata.frame_count,metadata.fps),(192,128,12,1,30));
    let output=dir.join(job.output_relative_path.unwrap());let graph=fs::read_to_string(output.parent().unwrap().join("render.filter_complex.txt")).unwrap();
    if !enabled{assert!(!graph.contains("[1:v]"));assert!(!graph.contains("cursor_tile"));}
    results.push(run("ffmpeg",&["-v","error","-i",output.to_str().unwrap(),"-f","rawvideo","-pix_fmt","rgb24","-"]));
   }
   if !cameras.is_empty(){assert_eq!(cameras[0],cameras[1],"drawing must not alter Follow camera");}
   assert_eq!(results[0].len(),192*128*3);assert_eq!(results[0].len(),results[1].len());
   let mut inside=0;let mut outside=0;
   for (i,(off,on)) in results[0].chunks_exact(3).zip(results[1].chunks_exact(3)).enumerate(){if off!=on {
    // The clipped 8px glyph, 2x camera, bicubic scales, styled fit and final chroma kernel all fit this fixed bound.
    if !transparent && i%192<32 && (40..88).contains(&(i/192)){inside+=1;}else{outside+=1;}
   }}
   assert_eq!(outside,0,"mode={mode:?} click={click} styled={styled} transparent={transparent}: nonlocal RGB differences");
   if !transparent{assert!(inside>0,"native cursor must remain visible");}
   eprintln!("pixel invariant: mode={mode:?} click={click} styled={styled} transparent={transparent}, glyph changes={inside}, exterior changes={outside}");
  }}}
  for (name,bytes) in assets{assert_eq!(fs::read(dir.join(name)).unwrap(),bytes,"render mutated {name}");}
 }
}
