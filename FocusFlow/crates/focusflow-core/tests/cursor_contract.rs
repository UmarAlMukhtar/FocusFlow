mod common;
use common::{fixture_project,TestDir};
use focusflow_core::{store::ProjectStore, legacy::LegacyDefaults, edits::Edit, ErrorCode};
use std::{fs,path::Path};
fn defaults()->LegacyDefaults { LegacyDefaults {scale:2.0,intro_us:0,hold_us:0,outro_us:0,pan_transition_us:0,settle_us:0} }
#[test]
fn capture_authority_blocks_legacy_even_when_malformed() {
 let temp=TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();common::write_legacy(&dir);
 fs::write(dir.join("capture.json"),b"malformed").unwrap();
 let store=ProjectStore::new(temp.path()).unwrap();
 let e=store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults()).unwrap_err();
 assert_eq!(e.code,ErrorCode::InvalidDocument); assert!(e.message.contains("native"));assert!(!dir.join("project.json").exists());
 assert_eq!(store.list_projects(None,100).unwrap().issues[0].error.code,ErrorCode::InvalidDocument);
}
#[test]
fn old_read_preserves_bytes_then_upgrade_is_atomic() {
 let temp=TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();
 let before=serde_json::to_vec(&fixture_project()).unwrap();fs::write(dir.join("project.json"),&before).unwrap();
 let store=ProjectStore::new(temp.path()).unwrap();store.get("fixture").unwrap();assert_eq!(fs::read(dir.join("project.json")).unwrap(),before);
 let doc=store.apply_edits("fixture",0,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:1,out_us:100}]).unwrap();
 assert_eq!(doc.schema_version,4);assert_eq!(doc.revision,1);
 let saved=fs::read(dir.join("project.json")).unwrap();
 assert_eq!(store.apply_edits("fixture",0,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:2,out_us:100}]).unwrap_err().code,ErrorCode::RevisionConflict);
 assert_eq!(fs::read(dir.join("project.json")).unwrap(),saved);
}
use focusflow_core::{cursor::*,project::*};
use serde_json::{json,Value};
use std::sync::{Arc,atomic::AtomicBool};
fn sample(at:u64,x:f64,y:f64)->Value {json!({"atUs":at,"x":x,"y":y,"visible":true,"shapeId":"a","discontinuity":false})}
struct Assets { _temp:TestDir, dir:std::path::PathBuf, doc:ProjectDocument, index:Value }
impl Assets {
 fn new(samples:&[Value])->Self {
  let temp=TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();
  let mut doc=fixture_project();doc.schema_version=3;doc.video.cursor=CursorMode::Separate;doc.cursor=Some(CursorAsset{index_path:"index.json".into()});doc.cursor_settings.enabled=true;
  let index=json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5_000_000,"samplesPath":"samples.ndjson","shapes":[{"id":"a","path":"a.pam","width":2,"height":2,"hotspotX":1,"hotspotY":1}]});
  let mut this=Self{_temp:temp,dir,doc,index};this.save_index();this.samples(samples);this.pam();this
 }
 fn save_index(&mut self) {fs::write(self.dir.join("index.json"),serde_json::to_vec(&self.index).unwrap()).unwrap();}
 fn samples(&self,samples:&[Value]) {let data=samples.iter().map(|v|format!("{}\n",v)).collect::<String>();fs::write(self.dir.join("samples.ndjson"),data).unwrap();}
 fn pam(&self) {let mut p=b"P7\nWIDTH 2\nHEIGHT 2\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();p.extend([255;16]);fs::write(self.dir.join("a.pam"),p).unwrap();}
 fn prepare(&self)->focusflow_core::CoreResult<Arc<PreparedCursor>> {Ok(prepare_cursor(&self.dir,&self.doc,&AtomicBool::new(false))?.unwrap())}
 fn arrow(&mut self) {self.doc.cursor_settings.style=CursorStyle::Arrow;}
}
#[test]
fn strict_versions_and_required_schema_three_fields() {
 let mut old=serde_json::to_value(fixture_project()).unwrap();
 for version in [1,2] {old["schemaVersion"]=json!(version);let d:ProjectDocument=serde_json::from_value(old.clone()).unwrap();assert_eq!(d.video.cursor,CursorMode::Embedded);assert_eq!(d.cursor_settings,CursorSettings::default());
 for (key,value) in [("cursor",Value::Null),("cursorSettings",serde_json::to_value(CursorSettings::default()).unwrap())] {let mut bad=old.clone();bad[key]=value;assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());}
 let mut bad=old.clone();bad["video"]["cursor"]=json!("embedded");assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());}
 let a=Assets::new(&[sample(0,0.0,0.0)]);let current=serde_json::to_value(a.doc).unwrap();assert!(serde_json::from_value::<ProjectDocument>(current.clone()).is_ok());
 for key in ["cursor","cursorSettings","frameStyle"] {let mut bad=current.clone();bad.as_object_mut().unwrap().remove(key);assert!(serde_json::from_value::<ProjectDocument>(bad).is_err(),"{key}");}
 let mut bad=current.clone();bad["video"].as_object_mut().unwrap().remove("cursor");assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());
 let mut bad=current;bad["cursorSettings"]["unexpected"]=json!(1);assert!(serde_json::from_value::<ProjectDocument>(bad).is_err());
}
#[test]
fn source_invariants_settings_ranges_and_restore_recovery() {
 let mut a=Assets::new(&[sample(0,0.0,0.0)]);fs::write(a.dir.join("screen.mp4"),b"test-owned").unwrap();
 fs::write(a.dir.join("project.json"),serde_json::to_vec(&a.doc).unwrap()).unwrap();
 let store=ProjectStore::new(a._temp.path()).unwrap();let enabled=EditableState::from_document(&a.doc);
 fs::write(a.dir.join("a.pam"),b"broken").unwrap();
 let before=fs::read(a.dir.join("project.json")).unwrap();
 let mut settings=a.doc.cursor_settings.clone();settings.scale=2.0;
 assert!(store.apply_edits("fixture",0,&[Edit::SetCursorSettings{settings:settings.clone()}]).is_err());assert_eq!(fs::read(a.dir.join("project.json")).unwrap(),before);
 // Unrelated edits retain broken enabled settings; explicit disabling always works.
 let d=store.apply_edits("fixture",0,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:1,out_us:100}]).unwrap();assert!(d.cursor_settings.enabled);
 settings.enabled=false;let d=store.apply_edits("fixture",1,&[Edit::SetCursorSettings{settings}]).unwrap();assert!(!d.cursor_settings.enabled);
 assert!(store.restore_edits("fixture",2,&enabled).is_err());assert_eq!(store.get("fixture").unwrap().revision,2);
 a.pam();let restored=store.restore_edits("fixture",2,&enabled).unwrap();assert_eq!(restored.revision,3);assert_eq!(restored.video,a.doc.video);assert_eq!(restored.cursor,a.doc.cursor);assert_eq!(restored.timing,a.doc.timing);
 for scale in [0.5,4.0] {a.doc.cursor_settings.scale=scale;assert!(validate(&a.doc).is_ok());}
 for scale in [0.4999,4.0001,f64::NAN] {a.doc.cursor_settings.scale=scale;assert!(validate(&a.doc).is_err());}a.doc.cursor_settings.scale=1.0;
 a.doc.cursor_settings.smoothing_us=500000;assert!(validate(&a.doc).is_ok());a.doc.cursor_settings.smoothing_us+=1;assert!(validate(&a.doc).is_err());a.doc.cursor_settings.smoothing_us=0;
 for mode in [CursorMode::Embedded,CursorMode::Absent] {a.doc.video.cursor=mode;assert!(validate(&a.doc).is_err());a.doc.cursor=None;a.doc.cursor_settings.enabled=false;assert!(validate(&a.doc).is_ok());a.doc.cursor_settings.enabled=true;assert!(validate(&a.doc).is_err());}
}
#[test]
fn native_corruption_arrow_choice_disable_and_unchecked_capabilities() {
 let mut a=Assets::new(&[sample(0,0.1,0.2)]);let prepared=a.prepare().unwrap();assert_eq!(prepared.shapes[0].pixels.len(),16);
 fs::write(a.dir.join("a.pam"),b"corrupt").unwrap();assert_eq!(prepared.shapes[0].pixels.len(),16);
 assert!(a.prepare().is_err());let report=inspect_cursor(&a.dir,&a.doc);let c=capabilities(&a.doc,&report);assert!(c.cursor_replacement&&c.cursor_smoothing&&!c.cursor_native_style&&c.cursor_follow);
 a.arrow();let p=a.prepare().unwrap();assert_eq!((p.shapes[0].width,p.shapes[0].height,p.shapes[0].hotspot_x,p.shapes[0].hotspot_y),(32,48,2,2));
 assert_eq!(scaled_shape(&p.shapes[0],0.75).unwrap(),(24,36,2,2));
 let c=capabilities(&a.doc,&CursorAssetReport::Unchecked);assert!(!c.cursor_replacement);assert_eq!(c.cursor_unavailable_reason,"Open the project to validate cursor assets");
 fs::write(a.dir.join("index.json"),b"malformed").unwrap();a.doc.cursor_settings.enabled=false;assert!(prepare_cursor(&a.dir,&a.doc,&AtomicBool::new(false)).unwrap().is_none());
 assert!(!capabilities(&a.doc,&inspect_cursor(&a.dir,&a.doc)).cursor_replacement);
 let report=CursorAssetReport::Unavailable(focusflow_core::CoreError{code:ErrorCode::InvalidDocument,message:"é".repeat(40000),field:None});assert!(capabilities(&a.doc,&report).cursor_unavailable_reason.len()<=1024);
}
#[test]
fn held_input_analytic_tau_step_seek_shape_visibility_and_stale_equality() {
 let mut samples=vec![sample(0,0.0,0.0),sample(100_000,1.0,1.0),sample(200_000,1.0,1.0),sample(450_000,0.0,0.0),sample(700_001,0.5,0.5)];
 let mut a=Assets::new(&samples);a.doc.cursor_settings.smoothing_us=100_000;let p=a.prepare().unwrap();
 assert_eq!(p.sample_at(99_999,1).unwrap().x,0.0);assert_eq!(p.sample_at(100_000,1).unwrap().x,0.0);
 assert!((p.sample_at(200_000,1).unwrap().x-(1.0-(-1.0f64).exp())).abs()<1e-12);
 assert!(p.sample_at(450_000,1).is_some());assert!(p.sample_at(700_000,1).is_none());assert_eq!(p.sample_at(700_001,1).unwrap().x,0.5);
 assert!(p.sample_at(5_000_000,1).is_none());assert!(p.sample_at(0,0).is_none());
 for k in 0..60 {assert_eq!(p.sample_at(k*1_000_000,30),p.sample_at(k*2*1_000_000,60));}
 assert_eq!(p.sample_at(220_000,1),p.sample_at(440_000,2));
 samples[1]["discontinuity"]=json!(true);a.samples(&samples);assert_eq!(a.prepare().unwrap().sample_at(100_000,1).unwrap().x,1.0);
 samples[1]["visible"]=json!(false);a.samples(&samples);let p=a.prepare().unwrap();assert!(p.sample_at(100_000,1).is_none());assert_eq!(p.sample_at(200_000,1).unwrap().x,1.0);
 a.samples(&[sample(100,0.1,0.2)]);let p=a.prepare().unwrap();assert!(p.sample_at(99,1).is_none());assert!(p.sample_at(100,1).is_some());assert!(p.sample_at(250100,1).is_none());
 a.doc.cursor_settings.smoothing_us=0;a.samples(&[sample(0,0.0,0.0),sample(1,1.0,1.0)]);assert_eq!(a.prepare().unwrap().sample_at(1,1).unwrap().x,1.0);
}
#[test]
fn source_clock_preserves_fractional_trim_and_repeated_clip_history() {
 let mut a=Assets::new(&[sample(0,0.0,0.0),sample(100000,1.0,1.0),sample(200000,1.0,1.0)]);a.doc.cursor_settings.smoothing_us=100000;
 let p=a.prepare().unwrap();let clip=focusflow_core::edits::MappedClip{clip_id:"c".into(),source_start_us:123456,source_end_us:300000,output_start_us:0};
 assert_eq!(source_frame_time(&clip,0,30).unwrap(),123456*30);
 let repeat=focusflow_core::edits::MappedClip{output_start_us:1_000_000,..clip.clone()};
 assert_eq!(p.sample_at(source_frame_time(&clip,0,30).unwrap(),30),p.sample_at(source_frame_time(&repeat,30,30).unwrap(),30));
 assert!(source_frame_time(&clip,0,0).is_err());assert!(source_frame_time(&repeat,0,30).is_err());
}
#[test]
fn sample_index_paths_and_image_validation_boundaries() {
 let mut a=Assets::new(&[sample(0,-20.48,41.96)]);assert!(a.prepare().is_ok());
 for (x,y) in [(-20.480001,0.0),(21.480001,0.0),(0.0,-40.960001),(0.0,41.960001)] {a.samples(&[sample(0,x,y)]);assert!(a.prepare().is_err());}
 a.samples(&[sample(0,21.48,-40.96)]);assert!(a.prepare().is_ok());
 for mut v in [sample(5_000_000,0.0,0.0),sample(0,0.0,0.0)] {if v["atUs"]==0 {v["shapeId"]=json!("unknown");}a.samples(&[v]);assert!(a.prepare().is_err());}
 a.samples(&[sample(0,0.0,0.0),sample(0,1.0,1.0)]);assert!(a.prepare().is_err());
 for key in ["atUs","x","y","visible","shapeId","discontinuity"] {let mut v=sample(0,0.0,0.0);v.as_object_mut().unwrap().remove(key);a.samples(&[v]);assert!(a.prepare().is_err(),"{key}");}
 a.samples(&[sample(0,0.0,0.0)]);
 for path in ["../a.pam","/a.pam","a\\pam","C:a.pam","a/./b","a//b"] {a.index["shapes"][0]["path"]=json!(path);a.save_index();a.arrow();assert!(a.prepare().is_err(),"{path}");}
 a.index["shapes"][0]["path"]=json!("a.pam");a.save_index();
 for (key,n) in [("width",513),("height",513),("width",0),("height",0),("hotspotX",2),("hotspotY",2)] {let before=a.index.clone();a.index["shapes"][0][key]=json!(n);a.save_index();assert!(a.prepare().is_err());a.index=before;a.save_index();}
 let original=fs::read(a.dir.join("a.pam")).unwrap();a.doc.cursor_settings.style=CursorStyle::Native;
 for bytes in [original[..original.len()-1].to_vec(),[original.as_slice(),b"x"].concat(),original.iter().copied().collect::<Vec<_>>().into_iter().collect::<Vec<_>>()] {
  if bytes==original {continue;}fs::write(a.dir.join("a.pam"),bytes).unwrap();assert!(a.prepare().is_err());
 }
 for (from,to) in [("DEPTH 4","DEPTH 3"),("MAXVAL 255","MAXVAL 256"),("RGB_ALPHA","RGB"),("WIDTH 2","WIDTH 2\nWIDTH 2"),("WIDTH 2","UNKNOWN 2"),("WIDTH 2","WIDTH 3")] {let mut bytes=String::from_utf8(original[..original.len()-16].to_vec()).unwrap().replace(from,to).into_bytes();bytes.extend([255;16]);fs::write(a.dir.join("a.pam"),bytes).unwrap();assert!(a.prepare().is_err(),"{from}");}
 a.pam();a.samples(&[]);assert!(a.prepare().is_err());
 a.samples(&[sample(0,0.0,0.0)]);assert!(prepare_cursor(&a.dir,&a.doc,&AtomicBool::new(true)).unwrap_err().code==ErrorCode::Cancelled);
}
#[test]
fn every_small_resource_limit_accepts_n_and_rejects_n_plus_one() {
 let mut a=Assets::new(&[sample(0,0.1,0.2)]);
 // Index bytes, including harmless trailing JSON whitespace.
 let base=serde_json::to_vec(&a.index).unwrap();let mut bytes=base.clone();bytes.resize(MAX_INDEX_BYTES as usize,b' ');fs::write(a.dir.join("index.json"),&bytes).unwrap();assert!(a.prepare().is_ok());bytes.push(b' ');fs::write(a.dir.join("index.json"),bytes).unwrap();assert!(a.prepare().is_err());a.save_index();
 // Record cap includes newline; final newline is optional but has the same byte cap.
 let base=serde_json::to_vec(&sample(0,0.1,0.2)).unwrap();
 for newline in [false,true] {let mut record=base.clone();record.resize(MAX_RECORD_BYTES-usize::from(newline),b' ');if newline {record.push(b'\n');}fs::write(a.dir.join("samples.ndjson"),&record).unwrap();assert!(a.prepare().is_ok());record.insert(0,b' ');fs::write(a.dir.join("samples.ndjson"),record).unwrap();assert!(a.prepare().is_err());}a.samples(&[sample(0,0.1,0.2)]);
 // Shape count and ID length.
 let shape=a.index["shapes"][0].clone();a.index["shapes"]=Value::Array((0..256).map(|n|{let mut s=shape.clone();s["id"]=json!(if n==0 {"a".into()} else {format!("a{n}")});s}).collect());a.save_index();assert!(a.prepare().is_ok());a.index["shapes"].as_array_mut().unwrap().push(shape.clone());a.save_index();assert!(a.prepare().is_err());
 a.index["shapes"]=json!([shape]);let id="a".repeat(128);a.index["shapes"][0]["id"]=json!(id);a.save_index();let mut s=sample(0,0.0,0.0);s["shapeId"]=json!(id);a.samples(&[s.clone()]);assert!(a.prepare().is_ok());a.index["shapes"][0]["id"]=json!("a".repeat(129));a.save_index();assert!(a.prepare().is_err());a.index["shapes"][0]["id"]=json!("a");a.save_index();a.samples(&[sample(0,0.0,0.0)]);
 // Relative path byte limit with real nested directories; every nested ref uses project root.
 let relative=format!("{}/{}/{}","a".repeat(200),"b".repeat(200),"c".repeat(110));assert_eq!(relative.len(),512);let path=a.dir.join(&relative);fs::create_dir_all(path.parent().unwrap()).unwrap();fs::copy(a.dir.join("samples.ndjson"),&path).unwrap();a.index["samplesPath"]=json!(relative);a.save_index();assert!(a.prepare().is_ok());a.index["samplesPath"]=json!(format!("{relative}x"));a.save_index();assert!(a.prepare().is_err());a.index["samplesPath"]=json!("samples.ndjson");a.save_index();
 // PAM header bytes and native dimensions/payload exact bounds.
 let original=fs::read(a.dir.join("a.pam")).unwrap();let header=String::from_utf8(original[..original.len()-16].to_vec()).unwrap();
 let padding=" ".repeat(MAX_HEADER_BYTES-header.len());let padded=header.replace("WIDTH 2\n",&format!("WIDTH 2{padding}\n"));assert_eq!(padded.len(),256);let mut bytes=padded.into_bytes();bytes.extend([255;16]);fs::write(a.dir.join("a.pam"),bytes).unwrap();assert!(a.prepare().is_ok());
 let bad=header.replace("WIDTH 2\n",&format!("WIDTH 2{padding} \n"));let mut bytes=bad.into_bytes();bytes.extend([255;16]);fs::write(a.dir.join("a.pam"),bytes).unwrap();assert!(a.prepare().is_err());
 let mut shape=a.index["shapes"][0].clone();shape["width"]=json!(512);shape["height"]=json!(512);a.index["shapes"]=Value::Array((0..32).map(|n|{let mut s=shape.clone();s["id"]=json!(if n==0 {"a".into()} else {format!("a{n}")});s}).collect());a.save_index();
 let mut pam=b"P7\nWIDTH 512\nHEIGHT 512\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();pam.resize(pam.len()+512*512*4,255);fs::write(a.dir.join("a.pam"),pam).unwrap();
 let p=a.prepare().unwrap();assert_eq!(p.shapes.iter().map(|s|s.pixels.len()).sum::<usize>(),MAX_NATIVE_BYTES);assert_eq!(scaled_shape(&p.shapes[0],4.0).unwrap(),(2048,2048,4,4));drop(p);
 let mut extra=shape;extra["id"]=json!("extra");extra["width"]=json!(1);extra["height"]=json!(1);extra["hotspotX"]=json!(0);extra["hotspotY"]=json!(0);a.index["shapes"].as_array_mut().unwrap().push(extra);a.save_index();assert!(a.prepare().is_err());
}
#[test]
fn shape_edges_native_visibility_and_immutable_snapshots() {
 let mut s=sample(100000,1.0,1.0);s["shapeId"]=json!("b");let mut a=Assets::new(&[sample(0,0.0,0.0),s]);let mut b=a.index["shapes"][0].clone();b["id"]=json!("b");a.index["shapes"].as_array_mut().unwrap().push(b);a.save_index();a.doc.cursor_settings.smoothing_us=100000;
 let p=a.prepare().unwrap();assert_eq!(p.sample_at(99999,1).unwrap().shape_index,0);let at=p.sample_at(100000,1).unwrap();assert_eq!(at.shape_index,1);assert_eq!(at.x,0.0);
 let copy=p.clone();assert!(Arc::ptr_eq(&p,&copy));assert!(Arc::ptr_eq(&p.shapes,&copy.shapes));assert!(p.buffer_bytes()<112*1024*1024);
 fs::remove_file(a.dir.join("a.pam")).unwrap();assert_eq!(p.shapes[0].pixels.len(),16);assert!(a.prepare().is_err());
 let mut s=sample(0,0.2,0.2);s["shapeId"]=Value::Null;a.samples(&[s]);assert!(a.prepare().is_err());a.arrow();assert!(a.prepare().is_ok());
}
#[test]
#[cfg(unix)]
fn unsafe_shape_symlinks_are_invalid_even_for_explicit_arrow() {
 let mut a=Assets::new(&[sample(0,0.0,0.0)]);a.arrow();fs::remove_file(a.dir.join("a.pam")).unwrap();let outside=TestDir::new();fs::write(outside.path().join("outside.pam"),b"not read").unwrap();
 std::os::unix::fs::symlink(outside.path().join("outside.pam"),a.dir.join("a.pam")).unwrap();assert_eq!(a.prepare().unwrap_err().code,ErrorCode::InvalidPath);
 fs::remove_file(outside.path().join("outside.pam")).unwrap();assert_eq!(a.prepare().unwrap_err().code,ErrorCode::InvalidPath);
 fs::remove_file(a.dir.join("a.pam")).unwrap();std::os::unix::fs::symlink(outside.path(),a.dir.join("alias")).unwrap();a.index["shapes"][0]["path"]=json!("alias/missing.pam");a.save_index();assert_eq!(a.prepare().unwrap_err().code,ErrorCode::InvalidPath);
}
#[test]
#[ignore = "Explicit near-cap memory/time evidence; run once rather than in every suite"]
fn maximum_sample_count_bytes_and_inspection_cost() {
 use std::{io::{BufWriter,Write},time::Instant};
 let mut a=Assets::new(&[sample(0,0.0,0.0)]);a.arrow();
 let file=fs::File::create(a.dir.join("samples.ndjson")).unwrap();let mut writer=BufWriter::new(file);
 let extra=MAX_SAMPLES_BYTES as usize-MAX_SAMPLES*134;
 for i in 0..MAX_SAMPLES {let mut line=serde_json::to_vec(&sample(i as u64,0.0,0.0)).unwrap();line.resize(133+usize::from(i<extra),b' ');line.push(b'\n');writer.write_all(&line).unwrap();}writer.flush().unwrap();drop(writer);
 assert_eq!(fs::metadata(a.dir.join("samples.ndjson")).unwrap().len(),MAX_SAMPLES_BYTES);
 let start=Instant::now();let p=a.prepare().unwrap();let elapsed=start.elapsed();assert_eq!(p.sample_count(),MAX_SAMPLES);assert!(p.buffer_bytes()<112*1024*1024);
 println!("MAXIMUM fixture bytes={} samples={} prepare_ms={} retained_cursor_bytes={}",MAX_SAMPLES_BYTES,p.sample_count(),elapsed.as_millis(),p.buffer_bytes());drop(p);
 let start=Instant::now();assert!(capabilities(&a.doc,&inspect_cursor(&a.dir,&a.doc)).cursor_replacement);println!("MAXIMUM inspection_ms={}",start.elapsed().as_millis());
 let file=fs::OpenOptions::new().write(true).open(a.dir.join("samples.ndjson")).unwrap();file.set_len(MAX_SAMPLES_BYTES+1).unwrap();assert!(a.prepare().is_err());drop(file);
 let file=fs::File::create(a.dir.join("samples.ndjson")).unwrap();let mut writer=BufWriter::new(file);
 for i in 0..=MAX_SAMPLES {writeln!(writer,"{}",sample(i as u64,0.0,0.0)).unwrap();}writer.flush().unwrap();drop(writer);
 let error=a.prepare().unwrap_err();assert!(error.message.contains("1000000"),"{error:?}");
}
#[test]
fn malformed_records_index_geometry_and_project_authority_remain_strict() {
 let mut a=Assets::new(&[sample(0,0.0,0.0)]);a.arrow();
 for bytes in [b"{\"atUs\":0".as_slice(),b"{\"atUs\":0,\"atUs\":1,\"x\":0,\"y\":0,\"visible\":true,\"shapeId\":null,\"discontinuity\":false}\n",b"{\"atUs\":0,\"x\":1e999,\"y\":0,\"visible\":true,\"shapeId\":null,\"discontinuity\":false}\n",b"{\"atUs\":0,\"x\":0,\"y\":0,\"visible\":true,\"shapeId\":null,\"discontinuity\":false,\"unknown\":1}\n"] {fs::write(a.dir.join("samples.ndjson"),bytes).unwrap();assert!(a.prepare().is_err());}a.samples(&[sample(0,0.0,0.0)]);
 for (key,value) in [("schemaVersion",json!(2)),("width",json!(99)),("height",json!(49)),("durationUs",json!(4_999_999)),("unknown",json!(true))] {let old=a.index.clone();a.index[key]=value;a.save_index();assert!(a.prepare().is_err(),"{key}");a.index=old;a.save_index();}
 let old=a.index.clone();let shape=a.index["shapes"][0].clone();a.index["shapes"].as_array_mut().unwrap().push(shape);a.save_index();assert!(a.prepare().is_err());a.index=old;a.save_index();
 fs::write(a.dir.join("project.json"),serde_json::to_vec(&a.doc).unwrap()).unwrap();fs::write(a.dir.join("capture.json"),b"malformed capture authority").unwrap();
 let store=ProjectStore::new(a._temp.path()).unwrap();assert_eq!(store.open_or_import("fixture",Path::new("absent"),defaults()).unwrap(),a.doc);
 for bytes in [b"malformed".as_slice(),b"{\"schemaVersion\":4}"] {fs::write(a.dir.join("project.json"),bytes).unwrap();assert!(store.open_or_import("fixture",Path::new("absent"),defaults()).is_err());assert_eq!(fs::read(a.dir.join("project.json")).unwrap(),bytes);}
 // Structurally valid broken cursor assets still open with unavailable capabilities.
 fs::write(a.dir.join("project.json"),serde_json::to_vec(&a.doc).unwrap()).unwrap();fs::write(a.dir.join("index.json"),b"malformed index").unwrap();let d=store.get("fixture").unwrap();assert!(!store.capabilities(&d).cursor_replacement);
 let page=store.list_projects(None,100).unwrap();assert_eq!(page.projects[0].capabilities.cursor_unavailable_reason,"Open the project to validate cursor assets");
}
#[test]
fn canonical_pam_header_rejects_unicode_whitespace() {
 let a=Assets::new(&[sample(0,0.0,0.0)]);
 let mut pam="P7\nWIDTH\u{2003}2\nHEIGHT 2\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".as_bytes().to_vec();pam.extend([255;16]);
 fs::write(a.dir.join("a.pam"),pam).unwrap();assert!(a.prepare().is_err());
}
#[test]
fn unused_unavailable_native_shape_keeps_visible_shape_indices_and_minimum_image() {
 let mut a=Assets::new(&[sample(0,0.0,0.0)]);let mut unused=a.index["shapes"][0].clone();unused["id"]=json!("unused");unused["path"]=json!("missing.pam");
 a.index["shapes"].as_array_mut().unwrap().insert(0,unused);a.save_index();let p=a.prepare().unwrap();assert!(p.shapes[0].pixels.is_empty());assert_eq!(p.sample_at(0,1).unwrap().shape_index,1);assert_eq!(p.shapes[1].pixels.len(),16);
 a.index["shapes"][1]["width"]=json!(1);a.index["shapes"][1]["height"]=json!(1);a.index["shapes"][1]["hotspotX"]=json!(0);a.index["shapes"][1]["hotspotY"]=json!(0);a.save_index();let mut pam=b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();pam.extend([255;4]);fs::write(a.dir.join("a.pam"),pam).unwrap();let p=a.prepare().unwrap();assert_eq!(p.shapes[1].pixels.len(),4);
}

fn enable_follow(a:&mut Assets) {
 a.doc.schema_version=4;a.doc.cursor_settings.enabled=false;
 a.doc.zooms=vec![ZoomElement{mode:ZoomMode::Follow,id:"follow".into(),clip_id:"clip-0".into(),start_us:0,end_us:1_000_000,x:0.2,y:0.3,scale:2.0,intro_us:0,hold_us:0,outro_us:0}];
}
#[test]
fn follow_prepares_positions_with_drawing_off_and_missing_native_pixels() {
 let mut a=Assets::new(&[sample(0,0.7,0.6)]);enable_follow(&mut a);fs::remove_file(a.dir.join("a.pam")).unwrap();
 let p=prepare_cursor(&a.dir,&a.doc,&AtomicBool::new(false)).unwrap();
 assert!(p.is_some(),"follow requires source positions even with drawing disabled");
 assert!(p.unwrap().shapes.is_empty(),"follow-only must not decode any images");
 assert!(capabilities(&a.doc,&inspect_cursor(&a.dir,&a.doc)).cursor_follow);
}
#[test]
fn follow_store_validates_changed_targets_but_allows_broken_asset_recovery() {
 let mut a=Assets::new(&[sample(0,0.7,0.6)]);enable_follow(&mut a);
 let follow=a.doc.zooms.pop().unwrap();fs::write(a.dir.join("project.json"),serde_json::to_vec(&a.doc).unwrap()).unwrap();
 let store=ProjectStore::new(a._temp.path()).unwrap();fs::write(a.dir.join("samples.ndjson"),b"broken").unwrap();
 let before=fs::read(a.dir.join("project.json")).unwrap();
 assert!(store.apply_edits("fixture",0,&[Edit::PutZoom{zoom:follow.clone()}]).is_err());
 assert_eq!(fs::read(a.dir.join("project.json")).unwrap(),before);
 a.samples(&[sample(0,0.7,0.6)]);store.apply_edits("fixture",0,&[Edit::PutZoom{zoom:follow.clone()}]).unwrap();
 fs::write(a.dir.join("samples.ndjson"),b"broken").unwrap();
 assert!(store.apply_edits("fixture",1,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:1,out_us:2_000_000}]).is_err());
 let mut fixed=follow;fixed.mode=ZoomMode::Fixed;
 assert_eq!(store.apply_edits("fixture",1,&[Edit::PutZoom{zoom:fixed}]).unwrap().revision,2);
}
#[test]
fn follow_left_limit_hold_entry_freshness_and_hidden_sentinel_oracle() {
 let mut hidden=sample(200_000,0.0,0.0);hidden["visible"]=json!(false);
 let mut a=Assets::new(&[sample(0,0.0,0.0),sample(100_000,1.0,1.0),hidden]);enable_follow(&mut a);a.doc.cursor_settings.smoothing_us=100_000;
 let p=a.prepare().unwrap();let fallback=[0.2,0.3];
 assert!((p.position_at(150_000,1).unwrap()[0]-0.3934693402873666).abs()<1e-12);
 assert!(p.position_at(200_000,1).is_none());
 assert!((p.follow_focus_at(300_000,1,199_999,fallback)[0]-0.6321205588285577).abs()<1e-12);
 assert_eq!(p.follow_focus_at(300_000,1,200_000,fallback),fallback);
 assert_eq!(p.follow_focus_at(500_000,1,0,fallback),p.follow_focus_at(1_000_000,2,0,fallback));
 a.samples(&[sample(0,0.0,0.0),sample(100_000,1.0,1.0)]);let p=a.prepare().unwrap();
 assert!(p.position_at(350_000,1).is_none());assert!(p.position_at(699_999,2).is_some());
 assert!((p.follow_focus_at(350_000,1,0,fallback)[0]-0.9179150013761012).abs()<1e-12);
 assert_eq!(p.follow_focus_at(350_000,1,350_000,fallback),fallback);
 let mut row=sample(350_000,-0.1,1.1);row["discontinuity"]=json!(true);
 a.samples(&[sample(100_000,1.0,1.0),row]);let p=a.prepare().unwrap();
 assert_eq!(p.position_at(350_000,1),Some([-0.1,1.1]));assert_eq!(p.follow_focus_at(350_000,1,0,fallback),[0.0,1.0]);
 fs::remove_file(a.dir.join("samples.ndjson")).unwrap();assert_eq!(p.follow_focus_at(350_000,1,0,fallback),[0.0,1.0]);
}
#[test]
fn follow_validation_ignores_broken_native_pixels_but_not_positions_or_paths() {
 let mut a=Assets::new(&[sample(0,0.7,0.6)]);enable_follow(&mut a);let z=a.doc.zooms.pop().unwrap();a.doc.cursor_settings.enabled=true;
 fs::write(a.dir.join("a.pam"),b"broken").unwrap();fs::write(a.dir.join("project.json"),serde_json::to_vec(&a.doc).unwrap()).unwrap();
 let store=ProjectStore::new(a._temp.path()).unwrap();assert_eq!(store.apply_edits("fixture",0,&[Edit::PutZoom{zoom:z.clone()}]).unwrap().revision,1);
 let state=EditableState::from_document(&store.get("fixture").unwrap());fs::write(a.dir.join("samples.ndjson"),b"broken").unwrap();
 let mut fixed=z.clone();fixed.id="fixed".into();fixed.mode=ZoomMode::Fixed;fixed.start_us=2_000_000;fixed.end_us=3_000_000;
 assert_eq!(store.apply_edits("fixture",1,&[Edit::PutZoom{zoom:fixed}]).unwrap().revision,2);
 let before=fs::read(a.dir.join("project.json")).unwrap();
 let mut changed=z;changed.x=0.4;assert!(store.apply_edits("fixture",2,&[Edit::PutZoom{zoom:changed}]).is_err());
 let mut smooth=state.clone();smooth.cursor_settings.smoothing_us=1;assert!(store.restore_edits("fixture",2,&smooth).is_err());
 assert!(store.apply_edits("fixture",2,&[Edit::SplitClip{clip_id:"clip-0".into(),at_us:500_000,right_clip_id:"right".into()}]).is_err());
 assert_eq!(fs::read(a.dir.join("project.json")).unwrap(),before);
 assert!(store.apply_edits("fixture",2,&[Edit::RemoveZoom{zoom_id:"follow".into()}]).is_ok());
 enable_follow(&mut a);a.index["shapes"][0]["path"]=json!("../escape.pam");a.save_index();a.samples(&[sample(0,0.7,0.6)]);assert!(a.prepare().is_err());
 a.index["shapes"][0]["path"]=json!("a.pam");a.save_index();let mut bad=sample(0,0.0,0.0);bad["x"]=json!(1e300);a.samples(&[bad]);assert!(a.prepare().is_err());
 let mut hidden=sample(0,0.0,0.0);hidden["visible"]=json!(false);a.samples(&[hidden]);assert!(!capabilities(&a.doc,&inspect_cursor(&a.dir,&a.doc)).cursor_follow);
}
