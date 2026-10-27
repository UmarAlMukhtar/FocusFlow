mod common;
use focusflow_core::{cursor,project::*,render::{self,ExportPreset},store::RenderSnapshot};
use std::{fs,sync::atomic::AtomicBool};
use serde_json::json;
fn fixture()->(common::TestDir,RenderSnapshot) {
 let t=common::TestDir::new();let mut d=common::fixture_project();d.schema_version=4;d.video.cursor=CursorMode::Separate;d.cursor=Some(CursorAsset{index_path:"index.json".into()});d.clicks.clear();
 d.zooms=vec![ZoomElement{mode:ZoomMode::Follow,id:"z".into(),clip_id:"clip-0".into(),start_us:0,end_us:1_000_000,x:0.2,y:0.3,scale:2.0,intro_us:1_000_000,hold_us:500_000,outro_us:1_000_000}];
 fs::write(t.path().join("screen.mp4"),b"fixture").unwrap();
 fs::write(t.path().join("index.json"),json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5_000_000,"samplesPath":"samples.ndjson","shapes":[]}).to_string()).unwrap();
 rows(t.path(),&[(0,0.8),(200_000,0.8),(400_000,0.8),(600_000,0.8),(800_000,0.8),(1_000_000,0.8),(1_200_000,0.6),(1_400_000,0.6),(1_600_000,0.1)]);
 let s=RenderSnapshot{project_dir:t.path().into(),document:d,prepared_cursor:None,prepared_focus:None};(t,s)
}
fn rows(dir:&std::path::Path,rows:&[(u64,f64)]) {fs::write(dir.join("samples.ndjson"),rows.iter().map(|(at,x)|json!({"atUs":at,"x":x,"y":0.5,"visible":true,"shapeId":null,"discontinuity":true}).to_string()+"\n").collect::<String>()).unwrap();}
fn prepare(s:&mut RenderSnapshot,p:&render::RenderPlan) {s.prepare_cursor(&AtomicBool::new(false)).unwrap();s.prepare_focus(p,&||Ok(false)).unwrap();}
fn pose(s:&RenderSnapshot,n:u128,r:u32)->render::CameraPose {s.prepared_focus.as_ref().unwrap().pose_at(n,r,s.prepared_cursor.as_ref().unwrap()).unwrap()}
fn close(v:f64,w:f64) {assert!((v-w).abs()<1e-12,"{v} != {w}");}
#[test]
fn live_intro_hold_and_anchored_outro_have_literal_poses() {
 let (_t,mut s)=fixture();let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();prepare(&mut s,&p);
 close(pose(&s,500_000,1).x,0.65);close(pose(&s,500_000,1).scale,1.5);
 close(pose(&s,1_300_000,1).x,0.6);close(pose(&s,2_000_000,1).x,0.55);close(pose(&s,2_000_000,1).scale,1.5);
 for k in 0..75 {assert_eq!(pose(&s,k*1_000_000,30),pose(&s,k*2_000_000,60));}
 let w=render::cursor_window::next(&s,&p,0,75,&AtomicBool::new(false)).unwrap();assert!(w.atlas.is_none());assert!(w.graph.is_empty());
 assert!(s.prepared_focus.as_ref().unwrap().buffer_bytes()<16*1024*1024);
 let mut changed=p.clone();changed.follow.as_mut().unwrap().targets[0].fallback[0]=0.9;
 assert!(s.prepared_focus.as_ref().unwrap().validate(&changed).is_err());
}
#[test]
fn clipped_outro_and_chained_live_intro_are_causal() {
 let (t,mut s)=fixture();s.document.zooms[0].intro_us=0;s.document.zooms[0].end_us=500_000;s.document.zooms[0].hold_us=0;
 let mut next=s.document.zooms[0].clone();next.id="next".into();next.start_us=1_000_000;next.end_us=2_000_000;next.intro_us=1_000_000;s.document.camera.pan_transition_us=1_000_000;s.document.zooms.push(next);
 rows(t.path(),&[(0,0.8),(500_000,0.8),(1_000_000,0.2),(1_400_000,0.2),(1_600_000,0.9),(1_800_000,0.9)]);
 let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();prepare(&mut s,&p);
 close(pose(&s,750_000,1).x,0.725);close(pose(&s,1_000_000,1).x,0.65);close(pose(&s,1_500_000,1).x,0.425);
 close(pose(&s,2_000_000,1).x,0.9);
}
#[test]
fn source_rational_mapping_and_cut_reset_are_independent_of_window_start() {
 let (t,mut s)=fixture();s.document.clips[0].in_us=1;s.document.clips[0].out_us=500_001;s.document.zooms[0].start_us=1;s.document.zooms[0].end_us=500_001;s.document.zooms[0].intro_us=0;
 let mut c=s.document.clips[0].clone();c.id="repeat".into();s.document.clips.push(c);let mut z=s.document.zooms[0].clone();z.id="repeat-z".into();z.clip_id="repeat".into();z.intro_us=500_000;s.document.zooms.push(z);
 rows(t.path(),&[(0,0.8)]);let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();prepare(&mut s,&p);
 assert_eq!(cursor::source_frame_time(&p.clips[1],15,30).unwrap(),30);
 close(pose(&s,500_000,1).x,0.5);close(pose(&s,750_000,1).x,0.65);
 let a=render::cursor_window::next(&s,&p,15,30,&AtomicBool::new(false)).unwrap();let b=render::cursor_window::next(&s,&p,23,30,&AtomicBool::new(false)).unwrap();
 assert_eq!(a.count,15);assert_eq!(b.count,7);
}
#[test]
fn constant_follow_matches_fixed_zero_and_clipped_phase_poses() {
 for timings in [[0,0,0],[100_000,400_000,900_000],[900_000,0,400_000]] {
  let (t,mut s)=fixture();rows(t.path(),&[(0,0.8)]);
  let z=&mut s.document.zooms[0];z.x=0.8;z.y=0.5;z.intro_us=timings[0];z.hold_us=timings[1];z.outro_us=timings[2];
  let mut z=z.clone();z.id="fixed-next".into();z.mode=ZoomMode::Fixed;z.start_us=1_500_000;z.end_us=2_000_000;z.x=0.3;z.scale=3.0;s.document.zooms.push(z);
  let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();prepare(&mut s,&p);
  let mut fixed=s.document.clone();fixed.zooms[0].mode=ZoomMode::Fixed;let f=render::compile(&fixed,ExportPreset::Balanced).unwrap();
  for at in (0..5_000_000).step_by(10_000) {let actual=pose(&s,at as u128,1);let expected=f.camera.iter().find(|i|i.start_us<=at&&at<i.end_us).unwrap().pose_at(at);close(actual.x,expected.x);close(actual.y,expected.y);close(actual.scale,expected.scale);}
 }
}
#[test]
fn preparation_and_window_cancellation_are_hard_errors() {
 let (_t,mut s)=fixture();let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();s.prepare_cursor(&AtomicBool::new(false)).unwrap();
 assert_eq!(s.prepare_focus(&p,&||Ok(true)).unwrap_err().code,focusflow_core::ErrorCode::Cancelled);
 s.prepare_focus(&p,&||Ok(false)).unwrap();assert_eq!(render::cursor_window::next(&s,&p,0,30,&AtomicBool::new(true)).err().unwrap().code,focusflow_core::ErrorCode::Cancelled);
}
#[test]
fn clipped_live_endpoint_keeps_current_target_before_resolving_boundary() {
 use render::follow::{Endpoint,FollowPlan,Phase,Target,PreparedFocus};
 let(t,mut s)=fixture();rows(t.path(),&[(0,0.2),(200_000,0.2),(400_000,0.2),(500_000,0.9)]);s.prepare_cursor(&AtomicBool::new(false)).unwrap();
 let mut p=render::compile(&s.document,ExportPreset::Balanced).unwrap();
 p.follow=Some(FollowPlan{targets:vec![Target{entry_us:0,fallback:[0.1,0.5]}],phases:vec![
  Phase{clip_index:0,start_us:0,end_us:500_000,source_start_us:0,output_start_us:0,from:Endpoint::Fixed{x:0.5,y:0.5},to:Endpoint::Follow{zoom_index:0},weight:0.5,scale_from:1.0,scale_to:1.5},
  Phase{clip_index:0,start_us:500_000,end_us:1_000_000,source_start_us:0,output_start_us:0,from:Endpoint::PreviousBoundary,to:Endpoint::Fixed{x:0.5,y:0.5},weight:1.0,scale_from:1.5,scale_to:1.0}]});
 let f=PreparedFocus::prepare(&p,s.prepared_cursor.as_ref().unwrap(),&||Ok(false)).unwrap().unwrap();
 close(f.pose_at(250_000,1,s.prepared_cursor.as_ref().unwrap()).unwrap().x,0.425);
 close(f.pose_at(500_000,1,s.prepared_cursor.as_ref().unwrap()).unwrap().x,0.7);
 close(f.pose_at(750_000,1,s.prepared_cursor.as_ref().unwrap()).unwrap().x,0.6);
}
#[test]
fn follow_phase_count_is_independent_of_telemetry_and_metadata_is_bounded() {
 let(t,mut s)=fixture();let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();let count=p.follow.as_ref().unwrap().phases.len();
 rows(t.path(),&(0..10_000).map(|i|(i*100,0.8)).collect::<Vec<_>>());prepare(&mut s,&p);assert_eq!(render::compile(&s.document,ExportPreset::Balanced).unwrap().follow.unwrap().phases.len(),count);
 let mut excessive=p.clone();let f=excessive.follow.as_mut().unwrap();f.phases=vec![f.phases[0].clone();65_537];
 assert!(render::follow::PreparedFocus::prepare(&excessive,s.prepared_cursor.as_ref().unwrap(),&||Ok(false)).is_err());
 let mut large=p;large.follow.as_mut().unwrap().targets=vec![render::follow::Target{entry_us:0,fallback:[0.5,0.5]};700_000];
 assert!(render::follow::PreparedFocus::prepare(&large,s.prepared_cursor.as_ref().unwrap(),&||Ok(false)).is_err());
}
#[test]
fn unavailable_target_holds_while_intro_continues_and_entry_resets_hold() {
 let(t,mut s)=fixture();rows(t.path(),&[(0,0.8)]);
 s.document.zooms[0].start_us=200_000;s.document.zooms[0].end_us=1_200_000;
 let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();prepare(&mut s,&p);
 close(pose(&s,700_000,1).x,0.65);
 s.document.zooms[0].start_us=250_000;s.document.zooms[0].end_us=1_250_000;
 let p=render::compile(&s.document,ExportPreset::Balanced).unwrap();prepare(&mut s,&p);
 close(pose(&s,750_000,1).x,0.35);
}
