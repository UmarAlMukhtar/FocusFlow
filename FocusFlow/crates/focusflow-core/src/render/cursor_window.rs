//! Bounded transient cursor windows. Source history lives only in PreparedCursor.
use crate::{CoreResult, cursor::{scaled_shape, source_frame_time}, project::{invalid,require}, store::RenderSnapshot};
use super::{RenderPlan,clip_grid,GraphText};
use std::sync::atomic::{AtomicBool,Ordering};
pub const MAX_FRAMES:u64=900;
// This input map is shared by the window graph and command builder.
pub(crate) const VIDEO_INPUT:usize=0;
pub(crate) const ATLAS_INPUT:usize=VIDEO_INPUT+1;
pub struct CursorWindow {
 pub clip:usize, pub first:u64, pub count:u64, pub source_first:u64,
 pub atlas:Option<Vec<u8>>, pub graph:String,
 pub camera:Option<[String;3]>,
}
fn layout(w:u32,h:u32,cells:usize)->Option<(u32,u32,u32)> {
 let cols=(4096/w).min(cells as u32); if cols==0{return None;}
 let rows=(cells as u32).div_ceil(cols);
 let (aw,ah)=(cols*w,rows*h);
 (cells<=64 && ah<=4096 && aw as usize*ah as usize*4<=16*1024*1024).then_some((aw,ah,cols))
}
fn expression<T:PartialEq+std::fmt::Display>(values:&[T], clock:&str)->CoreResult<String> {
 require(!values.is_empty() && values.len()<=900,"render.graphBytes","Cursor expressions require 1..900 leaves")?;
 fn tree<T:PartialEq+std::fmt::Display>(out:&mut GraphText,v:&[T],offset:usize,clock:&str)->CoreResult<()> {
  if v.iter().all(|n|*n==v[0]) {return out.append(format_args!("{}",v[0]));}
  let m=v.len()/2; out.append(format_args!("if(lt({clock},{}),",offset+m))?;
  tree(out,&v[..m],offset,clock)?;out.append(format_args!(","))?;tree(out,&v[m..],offset+m,clock)?;out.append(format_args!(")"))
 }
 let mut out=GraphText::new(128*1024);tree(&mut out,values,0,clock)?;Ok(out.text)
}
pub fn next(snapshot:&RenderSnapshot,plan:&RenderPlan,first:u64,end:u64,cancel:&AtomicBool)->CoreResult<CursorWindow> {
 let cursor=snapshot.prepared_cursor.as_ref().ok_or_else(||invalid("cursor","Cursor was not prepared inside the render worker"))?;
 if plan.follow.is_some() {
  snapshot.prepared_focus.as_ref().ok_or_else(||invalid("render.follow","Focus was not prepared inside the render worker"))?.validate(plan)?;
 }
 let fps=plan.preset.fps();
 let (ci,clip,base,source)=plan.clips.iter().enumerate().find_map(|(i,c)| {
  let (a,n,s)=clip_grid(c,fps).ok()?;(first>=a && first<a+n).then_some((i,c,a,s))
 }).ok_or_else(||invalid("cursor","Cursor frame is outside retained clips"))?;
 let (_,n,_)=clip_grid(clip,fps)?;
 let limit=end.min(base+n).min(first+MAX_FRAMES);
 if !snapshot.document.cursor_settings.enabled {
  require(limit>first,"render.window","Empty camera window")?;
  let camera=camera_expressions(snapshot,first,limit,fps,cancel)?;
  return Ok(CursorWindow{clip:ci,first,count:limit-first,source_first:source+first-base,atlas:None,graph:String::new(),camera});
 }
 let mut samples=Vec::new();let mut shapes=Vec::new();let(mut w,mut h)=(1,1);
 for k in first..limit {
  if cancel.load(Ordering::Acquire){return Err(crate::ffmpeg::cancel_error());}
  let sample=cursor.sample_at(source_frame_time(clip,k,fps)?,fps);
  if let Some(s)=&sample { if !shapes.contains(&s.shape_index) {
   let (sw,sh,_,_)=scaled_shape(&cursor.shapes[s.shape_index],cursor.scale)?;
   if layout(w.max(sw),h.max(sh),shapes.len()+1).is_none(){break;}
   w=w.max(sw);h=h.max(sh);shapes.push(s.shape_index);
  }}
  samples.push(sample);
 }
 require(!samples.is_empty(),"cursor.window","One cursor frame exceeds atlas limits")?;
 let (aw,ah,cols)=layout(w,h,shapes.len().max(1)).ok_or_else(||invalid("cursor.atlas","Cursor atlas exceeds resource bounds"))?;
 let mut atlas=format!("P7\nWIDTH {aw}\nHEIGHT {ah}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();
 let header=atlas.len();atlas.resize(header+aw as usize*ah as usize*4,0);let pixels=&mut atlas[header..];
 for (i,shape_index) in shapes.iter().enumerate(){
  if cancel.load(Ordering::Acquire){return Err(crate::ffmpeg::cancel_error());}
  let shape=&cursor.shapes[*shape_index];let(sw,sh,_,_)=scaled_shape(shape,cursor.scale)?;
  let cell=i as u32;let(ox,oy)=(cell%cols*w,cell/cols*h);
  for y in 0..sh {for x in 0..sw {
   let src=((y as u64*shape.height as u64/sh as u64)*shape.width as u64+x as u64*shape.width as u64/sw as u64) as usize*4;
   let dst=((oy+y) as usize*aw as usize+(ox+x) as usize)*4;
   pixels[dst..dst+4].copy_from_slice(&shape.pixels[src..src+4]);
  }}
 }
 let mut visible=Vec::new();let mut tx=Vec::new();let mut ty=Vec::new();let mut x=Vec::new();let mut y=Vec::new();
 for sample in &samples {
  visible.push(i64::from(sample.is_some()));
  let (cell,px,py)=if let Some(s)=sample {
   let (_,_,hx,hy)=scaled_shape(&cursor.shapes[s.shape_index],cursor.scale)?;
   (shapes.iter().position(|i|*i==s.shape_index).unwrap(),
    (s.x*snapshot.document.video.width as f64).round() as i64-hx as i64,
    (s.y*snapshot.document.video.height as f64).round() as i64-hy as i64)
  }else{(0,0,0)};
  tx.push(cell as i64%cols as i64*w as i64);ty.push(cell as i64/cols as i64*h as i64);x.push(px);y.push(py);
 }
 let atlas_input=ATLAS_INPUT;
 let graph=format!("[{atlas_input}:v]loop=loop=-1:size=1:start=0,setpts=N/({fps}*TB),crop=w={w}:h={h}:x='{}':y='{}':exact=1[cursor_tile];\n[vc{ci}]format=rgba[cursor_source];\n[cursor_source][cursor_tile]overlay=x='{}':y='{}':enable='{}':eval=frame:format=rgb:alpha=straight:shortest=1[cursor_composed]",expression(&tx,"n")?,expression(&ty,"n")?,expression(&x,"(n-1)")?,expression(&y,"(n-1)")?,expression(&visible,"n")?);

 let camera=camera_expressions(snapshot,first,first+samples.len() as u64,fps,cancel)?;
 Ok(CursorWindow{clip:ci,first,count:samples.len() as u64,source_first:source+first-base,atlas:Some(atlas),graph,camera})
}
fn camera_expressions(snapshot:&RenderSnapshot,first:u64,end:u64,fps:u32,cancel:&AtomicBool)->CoreResult<Option<[String;3]>> {
 let Some(focus)=&snapshot.prepared_focus else {return Ok(None)};
 let cursor=snapshot.prepared_cursor.as_ref().ok_or_else(||invalid("cursor","Missing follow positions"))?;
 let mut x=Vec::new();let mut y=Vec::new();let mut scale=Vec::new();
 for frame in first..end {
  if cancel.load(Ordering::Acquire) {return Err(crate::ffmpeg::cancel_error());}
  let p=focus.pose_at(u128::from(frame)*1_000_000,fps,cursor)?;
  require(p.x.is_finite()&&p.y.is_finite()&&p.scale.is_finite(),"render.follow","Nonfinite camera pose")?;
  x.push(p.x);y.push(p.y);scale.push(p.scale);
 }
 // Position axes each expand one position and two scale trees.
 require(x.len()*3<=900,"render.graphBytes","Follow expression exceeds 900 expanded leaves")?;
 Ok(Some([expression(&scale,"on")?,expression(&x,"on")?,expression(&y,"on")?]))
}
/// Apply the same deterministic graph-budget retry before any window is written or encoded.
pub(crate) fn next_with_graph(snapshot: &RenderSnapshot, plan: &RenderPlan, first: u64, end: u64,
    selection: Option<&crate::preview::PreviewSelection>, cancel: &AtomicBool) -> CoreResult<(CursorWindow, String)> {
    let mut limit=end.min(first+MAX_FRAMES);
    loop {
        let attempt=next(snapshot,plan,first,limit,cancel).and_then(|window| {
            super::window_graph(snapshot,plan,selection,&window).map(|graph|(window,graph))
        });
        match attempt {
            Ok(value)=>return Ok(value),
            Err(error) if error.field.as_deref()==Some("render.graphBytes") && limit>first+1=>limit=first+(limit-first)/2,
            Err(error)=>return Err(error),
        }
    }
}
#[cfg(test)]mod tests {
 use super::*;
 #[test]fn limits_and_balanced_runs(){assert!(layout(1,1,64).is_some());assert!(layout(1,1,65).is_none());assert!(expression(&[1;901],"n").is_err());assert!(layout(2048,2048,2).is_none());assert!(layout(2048,2048,1).is_some());assert!(layout(512,512,16).is_some());assert!(layout(512,512,17).is_none());assert_eq!(expression(&[7;900],"n").unwrap(),"7");let v:Vec<_>=(0..900).collect();assert!(expression(&v,"n").unwrap().len()<128*1024);}
 mod common { use crate as focusflow_core; include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/common/mod.rs")); }
 #[test]fn follow_only_graph_budget_halves_without_atlas_input_or_rounding(){
  let temp=common::TestDir::new();let mut doc=common::fixture_project();doc.schema_version=4;doc.video.duration_us=31_000_000;doc.clips[0].out_us=31_000_000;
  doc.video.cursor=crate::project::CursorMode::Separate;doc.cursor=Some(crate::project::CursorAsset{index_path:"index.json".into()});
  doc.zooms=vec![crate::project::ZoomElement{mode:crate::project::ZoomMode::Follow,id:"z".into(),clip_id:"clip-0".into(),start_us:0,end_us:31_000_000,x:0.2,y:0.3,scale:2.0,intro_us:0,hold_us:0,outro_us:0}];
  std::fs::write(temp.path().join("screen.mp4"),b"fixture").unwrap();
  std::fs::write(temp.path().join("index.json"),serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":31_000_000,"samplesPath":"samples.ndjson","shapes":[]}).to_string()).unwrap();
  std::fs::write(temp.path().join("samples.ndjson"),serde_json::json!({"atUs":0,"x":0.625,"y":0.5,"visible":true,"shapeId":null,"discontinuity":false}).to_string()).unwrap();
  let mut snapshot=RenderSnapshot{project_dir:temp.path().into(),document:doc,prepared_cursor:None,prepared_focus:None};
  let plan=super::super::compile(&snapshot.document,super::super::ExportPreset::Balanced).unwrap();let cancel=AtomicBool::new(false);snapshot.prepare_cursor(&cancel).unwrap();snapshot.prepare_focus(&plan,&||Ok(false)).unwrap();
  assert_eq!(next(&snapshot,&plan,0,900,&cancel).err().unwrap().field.as_deref(),Some("render.graphBytes"));
  let(w,graph)=next_with_graph(&snapshot,&plan,0,900,None,&cancel).unwrap();assert_eq!(w.count,225);assert!(w.atlas.is_none());assert!(graph.contains("(0.625)*iw"));assert!(!graph.contains("cursor_tile"));assert!(graph.contains(":d=1:"));
  let args=crate::ffmpeg::cursor_window_args(&snapshot,&plan,&w,temp.path(),false).unwrap();assert_eq!(args.iter().filter(|a|*a=="-i").count(),1);assert!(!args.iter().any(|a|a.to_string_lossy().contains(".pam")));
 }
 #[test]fn production_maximum_atlas_decodes_once_and_splits_before_second_shape(){
  use std::{fs,process::Command};
  let temp=common::TestDir::new();let dir=temp.path();let mut doc=common::fixture_project();doc.schema_version=3;
  doc.video.cursor=crate::project::CursorMode::Separate;doc.cursor=Some(crate::project::CursorAsset{index_path:"index.json".into()});doc.cursor_settings.enabled=true;doc.cursor_settings.scale=4.0;
  doc.clips[0].out_us=400000;
  let index=serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5000000,"samplesPath":"samples.ndjson","shapes":[{"id":"a","path":"a.pam","width":512,"height":512,"hotspotX":256,"hotspotY":256},{"id":"b","path":"a.pam","width":512,"height":512,"hotspotX":256,"hotspotY":256}]});
  fs::write(dir.join("index.json"),index.to_string()).unwrap();
  let sample=|at,id|serde_json::json!({"atUs":at,"x":0.5,"y":0.5,"visible":true,"shapeId":id,"discontinuity":false}).to_string()+"\n";
  fs::write(dir.join("samples.ndjson"),sample(0,"a")+&sample(200000,"b")).unwrap();
  let mut pam=b"P7\nWIDTH 512\nHEIGHT 512\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();pam.resize(pam.len()+512*512*4,255);fs::write(dir.join("a.pam"),pam).unwrap();
  assert!(Command::new("ffmpeg").args(["-v","error","-y","-f","lavfi","-i","color=black:s=100x50:r=30:d=1","-c:v","libx264"]).arg(dir.join("screen.mp4")).status().unwrap().success());
  let mut snapshot=RenderSnapshot{project_dir:dir.into(),document:doc,prepared_focus:None,prepared_cursor:None};snapshot.prepare_cursor(&AtomicBool::new(false)).unwrap();let plan=super::super::compile(&snapshot.document,super::super::ExportPreset::Balanced).unwrap();
  let window=next(&snapshot,&plan,0,12,&AtomicBool::new(false)).unwrap();assert_eq!(window.count,6);assert!(window.atlas.as_ref().unwrap().len()<=16*1024*1024+256);
  fs::write(dir.join("cursor-atlas.pam"),window.atlas.as_ref().unwrap()).unwrap();fs::write(dir.join(crate::ffmpeg::FILTER_SCRIPT),super::super::window_graph(&snapshot,&plan,None,&window).unwrap()).unwrap();
  let args=crate::ffmpeg::cursor_window_args(&snapshot,&plan,&window,dir,false).unwrap();let out=Command::new("ffmpeg").args(["-loglevel","debug"]).args(args).output().unwrap();let log=String::from_utf8_lossy(&out.stderr);assert!(out.status.success(),"{log}");
  let tail=log.split("Input file #1").last().unwrap();assert!(tail.contains("1 packets read")&&tail.contains("1 frames decoded"),"{tail}");println!("2048x2048 atlas: {}",tail.lines().take(3).collect::<Vec<_>>().join(" "));
  // Preparation owns bytes: deleting the producer files cannot alter the next window.
  fs::remove_file(dir.join("a.pam")).unwrap();fs::remove_file(dir.join("samples.ndjson")).unwrap();let next_window=next(&snapshot,&plan,6,12,&AtomicBool::new(false)).unwrap();assert_eq!(next_window.count,6);
 }

}
