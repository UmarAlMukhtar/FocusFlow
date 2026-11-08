//! Windows authority assembly and bounded worker protocol; native calls live in `process`.
use super::{owned_feed, RecorderError, RecorderResult};
use focusflow_core::{capture::{*, windows::*}, ffmpeg::SavedCfrGrid, legacy::LegacyDefaults, project::*};
use serde::{Deserialize, Serialize};
use std::{io::{self, Read}, path::Path};
mod lifecycle;
pub(in crate::recorder) mod cursor;
#[cfg(windows)]
mod process;
#[cfg(windows)]
pub(super) use process::Session;
#[cfg(windows)]
pub(crate) use process::dispatch;
#[cfg(all(windows,test))]
pub(super) use process::qualification;

#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
enum SourceAvailabilityEnd {Unavailable,Unconfirmed}
impl SourceAvailabilityEnd {
 fn install(self,cutoff:&mut Option<i64>,now:impl FnOnce()->Result<i64,String>)->Result<Option<Self>,String>{
  if cutoff.is_some(){return Ok(None);}*cutoff=Some(now()?);Ok(Some(self))
 }
 fn notice(self)->&'static str{match self{
  Self::Unavailable=>"The selected window became unavailable. Complete frames were saved; select the window again to continue.",
  Self::Unconfirmed=>"The selected window availability could not be confirmed. Complete frames were saved; select the window again to continue."
 }}
}
fn observe_source(expected:(u32,u32),alive:impl FnOnce()->Result<bool,()>,identity:impl FnOnce()->Result<(u32,u32),SourceAvailabilityEnd>)->Option<SourceAvailabilityEnd>{
 match alive(){Ok(false)=>Some(SourceAvailabilityEnd::Unavailable),Err(())=>Some(SourceAvailabilityEnd::Unconfirmed),Ok(true)=>match identity(){
  Ok((0,_)|(_,0))=>Some(SourceAvailabilityEnd::Unconfirmed),Ok(actual) if actual==expected=>None,Ok(_)=>Some(SourceAvailabilityEnd::Unavailable),Err(reason)=>Some(reason)
 }}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Summary {frequency:i64,admission:i64,epoch:i64,cutoff:i64,closed:bool,selected:u32,written:u32,encoder_exit:i32,geometry_changed:bool,source_availability:Option<SourceAvailabilityEnd>}
impl Summary {
 fn check_stop_ack(&self,sent:bool,ack:Option<i64>)->Result<(),&'static str>{
  if !self.end_flags_valid(){return Err("Worker summary has contradictory end causes");}
  if sent&&!self.closed&&!self.geometry_changed&&self.source_availability.is_none()&&ack!=Some(self.cutoff){return Err("Installed Stop acknowledgement missing or inconsistent");}Ok(())
 }
 fn end_flags_valid(&self)->bool{u8::from(self.closed)+u8::from(self.geometry_changed)+u8::from(self.source_availability.is_some())<=1}
 fn clock(&self,worker_exit:i32)->Result<WindowsOwnedClockV1,&'static str>{
  if !self.end_flags_valid()||worker_exit!=0||self.encoder_exit!=0||self.written==0||self.written>owned_feed::MAX_FRAMES||self.written!=self.selected
   ||!owned_feed::admitted(self.epoch,self.admission,self.frequency)?||self.cutoff<self.admission
   ||owned_feed::complete_cells(self.epoch,self.cutoff,self.frequency)?!=self.written{return Err("Worker summary does not prove complete normal capture");}
  Ok(WindowsOwnedClockV1{provenance:WindowsClockProvenance::OwnedWgcCausalCfrV1,qpc_frequency:self.frequency.to_string(),admission_qpc:self.admission.to_string(),epoch_100ns:self.epoch.to_string(),cutoff_qpc:self.cutoff.to_string(),cutoff_reason:if self.closed{WindowsCutoffReason::SourceClosed}else{WindowsCutoffReason::Stop},completed_frames:self.written.into(),worker_exit_code:worker_exit,encoder_exit_code:self.encoder_exit,interactions:Unmeasured::Unmeasured,audio:Unmeasured::Unmeasured})
 }
}
#[derive(Debug,Serialize,Deserialize)]
#[serde(tag="event",deny_unknown_fields)]
enum Message {Prepared {width:u32,height:u32,cursor:cursor::Admission}, Encoder {pid:u32}, Ready {}, Ack {cutoff:i64}, Terminal {summary:Summary,cursor:cursor::Terminal}, Error {message:String}}
fn read_message(reader:&mut impl Read)->Result<Message,String>{
 let mut bytes=Vec::new();loop {let mut b=[0];match reader.read(&mut b){Ok(0)=>return Err("Worker pipe ended without complete protocol".into()),Ok(_)=>{if b[0]==b'\n'{break;}if bytes.len()>=4096{return Err("Worker message exceeded 4096 bytes".into());}bytes.push(b[0]);},Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)=>return Err(e.to_string())}}
 let shape:serde_json::Value=serde_json::from_slice(&bytes).map_err(|e|format!("Invalid worker message: {e}"))?;
 if !shape.is_object()||shape.get("summary").is_some_and(|value|!value.is_object()){return Err("Worker protocol requires named objects".into());}
 if let Some(cursor)=shape.get("cursor"){
  if !cursor.is_object(){return Err("Cursor protocol requires named objects".into());}
  if let Some(completion)=cursor.get("completion"){
   if !completion.is_object()||["pose","input","poseStats","inputStats","terminal"].iter().any(|key|!completion.get(key).is_some_and(|v|v.is_object())){return Err("Cursor completion requires named objects".into());}
   for key in ["poseBarrier","inputBarrier"]{if !completion["terminal"].get(key).is_some_and(|v|v.is_null()||v.is_object()){return Err("Cursor terminal barrier requires named object or explicit null".into());}}
  }
 }
 // Decode original bytes again so duplicate named fields are not hidden by the shape check.
 serde_json::from_slice(&bytes).map_err(|e|format!("Invalid worker message: {e}"))
}
pub(super) fn recovery(error:impl std::fmt::Display)->RecorderError {RecorderError::new("windows_capture_recovery",format!("Windows recording files were retained for recovery: {error}"),true)}
fn incomplete(id:&str,window:u64,width:u32,height:u32,bounds:Bounds,defaults:LegacyDefaults)->RecorderResult<WindowsCaptureManifestV2>{
 let g=owned_feed::Geometry::new(width,height).map_err(recovery)?;
 let manifest=WindowsCaptureManifestV2{schema_version:2,capture_id:id.into(),source:WindowsSourceV2{backend:WindowsBackend::WindowsWgcOwned,kind:WindowsSourceKind::Window,window_id:window.to_string(),observed_window_bounds:Some(bounds),raw_size:PixelSize{width,height},encoded_size:PixelSize{width:g.encoded_width as u32,height:g.encoded_height as u32},content_rect:Bounds{x:0,y:0,width,height},pointer_origin:NullOnly},initial_defaults:WindowsInitialDefaults{frame_style:Default::default(),camera:CameraDefaults{pan_transition_us:defaults.pan_transition_us,settle_us:defaults.settle_us}},diagnostic:None,state:WindowsCaptureStateV2::Incomplete{video_path:VideoPath::Screen,video_cursor:EmbeddedCursor::Embedded}};
 manifest.validate(id).map_err(recovery)?;Ok(manifest)
}
#[derive(Debug)]
pub(super) struct Completion {manifest:WindowsCaptureManifestV2,clock:WindowsOwnedClockV1,pub(super) notice:Option<String>,cursor:Option<(focusflow_core::capture::windows_observations::WindowsCaptureManifestV3,cursor::CursorObservationCompletion)>}
impl Completion {
 fn new(manifest:WindowsCaptureManifestV2,summary:Summary,worker_exit:i32)->RecorderResult<Self>{
  let clock=summary.clock(worker_exit).map_err(recovery)?;
  let notice=summary.source_availability.map(|reason|reason.notice().into()).or_else(||summary.geometry_changed.then(||"The selected window changed size. Complete frames were saved; start a new recording at the new size.".into()));
  Ok(Self{manifest,clock,notice,cursor:None})
 }
 fn from_worker(manifest:WindowsCaptureManifestV2,defaults:LegacyDefaults,admission:&cursor::Admission,terminal:cursor::Terminal,summary:Summary,worker_exit:i32)->RecorderResult<Self>{
  terminal.validate(admission,summary.cutoff).map_err(recovery)?;
  let mut result=Self::new(manifest,summary,worker_exit)?;
  if let cursor::Terminal::Separate{completion}=terminal{result.cursor=Some((cursor::incomplete(&result.manifest,defaults),completion));}
  result.notice=result.notice.or_else(||admission.notice());Ok(result)
 }
 #[cfg(test)] pub(in crate::recorder) fn observation_receipt(&self)->RecorderResult<Option<serde_json::Value>>{self.cursor.as_ref().map(|(_,completion)|serde_json::to_value(completion).map_err(recovery)).transpose()}
 pub(super) fn count(&self)->u64{self.clock.completed_frames}
 pub(super) fn publish(mut self,dir:&Path,grid:&SavedCfrGrid,has_mic:bool)->RecorderResult<Option<String>>{
  if let Some((manifest,completion))=self.cursor.take(){return cursor::publish(dir,manifest,self.clock,grid,completion,has_mic,self.notice).map_err(recovery);}
  let saved=grid.video;let duration=saved.video.duration_us;
  let project=ProjectDocument{audio_tracks:None,schema_version:3,project_id:self.manifest.capture_id.clone(),revision:0,video:VideoAsset{path:"screen.mp4".into(),width:saved.video.width,height:saved.video.height,duration_us:duration,cursor:CursorMode::Embedded},cursor:None,cursor_settings:Default::default(),microphone:has_mic.then_some(AudioAsset{path:"mic.wav".into()}),timing:Timing{provenance:TimingProvenance::LegacyUnknown,audio_offset_us:None},camera:self.manifest.initial_defaults.camera.clone(),frame_style:self.manifest.initial_defaults.frame_style.clone(),clips:vec![Clip{id:"clip-0".into(),in_us:0,out_us:duration}],zooms:vec![],clicks:vec![],warnings:vec![ProjectWarning::EmbeddedCursor,ProjectWarning::LegacyTimingUnknown]};
  self.manifest.diagnostic=self.notice.map(|message|CaptureDiagnosticV1{stage:DiagnosticStage::Clock,message});
  self.manifest.state=WindowsCaptureStateV2::Finalized{video_timing:WindowsVideoTimingV2{endpoint:WindowsSavedEndpointV2{provenance:WindowsEndpointProvenance::SavedPacketFullGrid,end_pts:saved.endpoint.ticks,time_base:TimeBase{numerator:saved.endpoint.numerator,denominator:saved.endpoint.denominator},interval:Interval::HalfOpen},clock:self.clock},cursor_timing:NullOnly,initial_project:project};
  publish_windows_finalized_assets(dir,&self.manifest,grid).map_err(recovery)?;Ok(None)
 }
}
#[cfg(test)]
mod tests {
 use super::*;
 fn summary()->Summary{Summary{frequency:3_000_000,admission:2_997_000,epoch:10_000_000,cutoff:6_000_000,closed:false,selected:30,written:30,encoder_exit:0,geometry_changed:false,source_availability:None}}

 #[test] fn availability_observations_distinguish_loss_from_unconfirmed_and_stable_identity(){
  use SourceAvailabilityEnd::*;
  assert_eq!(observe_source((7,9),||Ok(false),||panic!("Exited exact owner must not query a recycled PID")),Some(Unavailable));
  assert_eq!(observe_source((7,9),||Err(()),||panic!("Failed wait cannot prove window identity")),Some(Unconfirmed));
  for observed in [(7,9),(8,9),(7,10)]{assert_eq!(observe_source((7,9),||Ok(true),||Ok(observed)),if observed==(7,9){None}else{Some(Unavailable)});}
  assert_eq!(observe_source((7,9),||Ok(true),||Err(Unavailable)),Some(Unavailable));
  assert_eq!(observe_source((7,9),||Ok(true),||Err(Unconfirmed)),Some(Unconfirmed));
  assert_eq!(observe_source((7,9),||Ok(true),||Ok((0,9))),Some(Unconfirmed));
 }
 #[test] fn availability_first_cause_installs_actual_cutoff_and_never_backdates_or_relabels(){
  use SourceAvailabilityEnd::*;
  let mut cutoff=None;assert_eq!(Unavailable.install(&mut cutoff,||Ok(123)).unwrap(),Some(Unavailable));assert_eq!(cutoff,Some(123));
  assert_eq!(Unconfirmed.install(&mut cutoff,||panic!("Existing cutoff wins without another QPC")).unwrap(),None);assert_eq!(cutoff,Some(123));
  for _prior_cause in ["Stop","native Closed","geometry"]{let mut prior=Some(99);assert_eq!(Unavailable.install(&mut prior,||panic!("Late query cannot change first cause")).unwrap(),None);assert_eq!(prior,Some(99));}
  let mut missing=None;assert!(Unavailable.install(&mut missing,||Err("QPC failed".into())).is_err());assert_eq!(missing,None);
 }
 #[test] fn availability_end_allows_only_automatic_ack_exception_and_preserves_stop_clock(){
  let ordinary=summary();assert!(ordinary.check_stop_ack(true,None).is_err());assert!(ordinary.check_stop_ack(true,Some(ordinary.cutoff-1)).is_err());assert!(ordinary.check_stop_ack(true,Some(ordinary.cutoff)).is_ok());assert!(ordinary.check_stop_ack(false,None).is_ok());
  for end in [SourceAvailabilityEnd::Unavailable,SourceAvailabilityEnd::Unconfirmed]{let mut s=summary();s.source_availability=Some(end);assert!(s.check_stop_ack(true,None).is_ok());let c=s.clock(0).unwrap();assert_eq!(c.cutoff_reason,WindowsCutoffReason::Stop);assert_eq!(c.completed_frames,30);}
 }
 #[test] fn availability_notice_is_durable_clock_diagnostic_without_changing_frozen_snapshot(){
  for end in [SourceAvailabilityEnd::Unavailable,SourceAvailabilityEnd::Unconfirmed]{
   let dir=std::env::temp_dir().join(format!("focusflow-availability-{}-{end:?}",std::process::id()));std::fs::create_dir(&dir).unwrap();let dir=dir.canonicalize().unwrap();let id=dir.file_name().unwrap().to_str().unwrap();
   let defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();let m=incomplete(id,42,320,240,Bounds{x:0,y:0,width:320,height:240},defaults).unwrap();publish_windows_incomplete(&dir,&m).unwrap();std::fs::write(dir.join("screen.mp4"),b"retained source").unwrap();
   let grid=SavedCfrGrid{completed_frames:30,video:focusflow_core::ffmpeg::ProbedVideoEvidence{video:focusflow_core::ffmpeg::ProbedVideo{width:320,height:240,duration_us:1_000_000},endpoint:focusflow_core::ffmpeg::PacketTime{ticks:30000,numerator:1,denominator:30000},first_packet_duration:focusflow_core::ffmpeg::PacketTime{ticks:1000,numerator:1,denominator:30000}}};
   let mut s=summary();s.source_availability=Some(end);let c=Completion::new(m,s,0).unwrap();assert!(c.notice.is_some());c.publish(&dir,&grid,false).unwrap();let saved=decode_windows(&std::fs::read(dir.join("capture.json")).unwrap(),id).unwrap();let diagnostic=saved.diagnostic.as_ref().unwrap();assert_eq!(diagnostic.stage,DiagnosticStage::Clock);
   assert!(diagnostic.message.contains(if end==SourceAvailabilityEnd::Unavailable{"became unavailable"}else{"could not be confirmed"}));let doc=saved.finalized_project().unwrap();assert_eq!(doc.video.cursor,CursorMode::Embedded);assert_eq!(doc.camera.pan_transition_us,defaults.pan_transition_us);assert_eq!(std::fs::read(dir.join("screen.mp4")).unwrap(),b"retained source");std::fs::remove_dir_all(dir).unwrap();
  }
 }
 #[test] fn availability_rejects_contradictory_terminal_causes_without_ack_bypass(){
  for (closed,geometry) in [(true,false),(false,true),(true,true)]{let mut s=summary();s.source_availability=Some(SourceAvailabilityEnd::Unavailable);s.closed=closed;s.geometry_changed=geometry;assert!(s.clock(0).is_err());assert!(s.check_stop_ack(true,None).is_err());}
  let mut s=summary();s.closed=true;s.geometry_changed=true;assert!(s.clock(0).is_err());
 }
 #[test] fn terminal_rejects_partial_killed_missing_and_inconsistent_results(){
  assert!(summary().clock(0).is_ok());assert!(summary().clock(1).is_err());
  for change in [0,1,2,3,4,5] {let mut s=summary();match change{0=>s.written=29,1=>s.selected=31,2=>s.encoder_exit=1,3=>{s.selected=0;s.written=0},4=>s.cutoff=5_999_999,_=>s.admission=3_000_001};assert!(s.clock(0).is_err());}
  for bytes in [b"{\"event\":\"Ready\"}".as_slice(),b"{\"event\":\"Terminal\",\"summary\":{}}\n",b"{\"event\":\"Ready\",\"extra\":1}\n",b"{\"event\":\"Terminal\",\"summary\":[3000000,2997000,10000000,6000000,false,30,30,0,false]}\n",b"{\"event\":\"Ready\",\"event\":\"Ready\"}\n"] {assert!(read_message(&mut &bytes[..]).is_err());}
  assert!(read_message(&mut &vec![b'x';4097][..]).is_err());assert!(matches!(read_message(&mut &b"{\"event\":\"Ready\"}\n"[..]),Ok(Message::Ready{})));
 }
 #[test] fn snapshot_freezes_defaults_and_resize_is_stop_not_source_closed(){
  let defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();let m=incomplete("fixture",42,641,359,Bounds{x:-100,y:3,width:641,height:359},defaults).unwrap();
  assert_eq!(m.source.encoded_size,PixelSize{width:642,height:360});assert_eq!(m.source.content_rect,Bounds{x:0,y:0,width:641,height:359});
  let mut s=summary();s.geometry_changed=true;let c=Completion::new(m,s,0).unwrap();assert!(c.notice.is_some());assert_eq!(c.clock.cutoff_reason,WindowsCutoffReason::Stop);
 }
 #[test] fn attempted_marker_failure_retains_session_and_never_produces_video(){
  use super::super::{RecorderState,StartReservation};
  let dir=std::env::temp_dir().join(format!("focusflow-wgc-marker-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();let dir=dir.canonicalize().unwrap();
  let defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();let m=incomplete(dir.file_name().unwrap().to_str().unwrap(),42,320,240,Bounds{x:0,y:0,width:320,height:240},defaults).unwrap();
  std::fs::create_dir(dir.join("capture.windows-v2")).unwrap();let state=RecorderState::new();let mut reservation=StartReservation::acquire(&state).unwrap();reservation.directory=Some(dir.clone());
  assert!(reservation.publish_windows_incomplete(&dir,&m).is_err());drop(reservation);assert!(dir.exists());assert!(!dir.join("screen.mp4").exists());assert!(!dir.join("capture.json").exists());assert!(!state.cleanup_runtime().starting);std::fs::remove_dir_all(dir).unwrap();
 }
 #[test] fn saved_snapshot_uses_frozen_defaults_not_raw_clicks_and_rejects_failed_grid(){
  let dir=std::env::temp_dir().join(format!("focusflow-wgc-snapshot-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();let dir=dir.canonicalize().unwrap();let id=dir.file_name().unwrap().to_str().unwrap();
  let mut defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();defaults.pan_transition_us=123456;
  let m=incomplete(id,42,320,240,Bounds{x:-10,y:-20,width:320,height:240},defaults).unwrap();publish_windows_incomplete(&dir,&m).unwrap();std::fs::write(dir.join("screen.mp4"),b"unchanged media").unwrap();std::fs::write(dir.join("clicks.json"),b"untrusted legacy clicks").unwrap();
  let mut grid=SavedCfrGrid{completed_frames:29,video:focusflow_core::ffmpeg::ProbedVideoEvidence{video:focusflow_core::ffmpeg::ProbedVideo{width:320,height:240,duration_us:1_000_000},endpoint:focusflow_core::ffmpeg::PacketTime{ticks:30000,numerator:1,denominator:30000},first_packet_duration:focusflow_core::ffmpeg::PacketTime{ticks:1000,numerator:1,denominator:30000}}};
  let c=Completion::new(m.clone(),summary(),0).unwrap();assert_eq!(c.count(),30);assert!(c.publish(&dir,&grid,false).is_err());assert!(matches!(decode_windows(&std::fs::read(dir.join("capture.json")).unwrap(),id).unwrap().state,WindowsCaptureStateV2::Incomplete{..}));
  grid.completed_frames=30;Completion::new(m,summary(),0).unwrap().publish(&dir,&grid,false).unwrap();let manifest=decode_windows(&std::fs::read(dir.join("capture.json")).unwrap(),id).unwrap();let doc=manifest.finalized_project().unwrap();
  assert!(doc.clicks.is_empty()&&doc.zooms.is_empty());assert_eq!(doc.camera.pan_transition_us,123456);assert_eq!(doc.video.cursor,CursorMode::Embedded);assert_eq!(std::fs::read(dir.join("screen.mp4")).unwrap(),b"unchanged media");assert!(!dir.join("project.json").exists());std::fs::remove_dir_all(dir).unwrap();
 }

 #[test] fn frozen_media_dimensions_stay_independent_of_observed_bounds_and_legacy_origin(){
  for (width,height,encoded) in [(322,272,(322,272)),(641,359,(642,360))]{
   let dir=std::env::temp_dir().join(format!("focusflow-frozen-{}-{width}",std::process::id()));std::fs::create_dir(&dir).unwrap();let dir=dir.canonicalize().unwrap();let id=dir.file_name().unwrap().to_str().unwrap();
   let defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();let bounds=Bounds{x:-120,y:37,width:336,height:279};
   let manifest=incomplete(id,42,width,height,bounds.clone(),defaults).unwrap();let frozen_source=serde_json::to_value(&manifest.source).unwrap();
   assert_eq!(manifest.source.raw_size,PixelSize{width,height});assert_eq!(manifest.source.encoded_size,PixelSize{width:encoded.0,height:encoded.1});assert_eq!(manifest.source.content_rect,Bounds{x:0,y:0,width,height});assert_eq!(manifest.source.observed_window_bounds,Some(bounds));assert_eq!(frozen_source["pointerOrigin"],serde_json::Value::Null);
   publish_windows_incomplete(&dir,&manifest).unwrap();std::fs::write(dir.join("screen.mp4"),b"immutable media").unwrap();
   std::fs::write(dir.join("clicks.json"),br#"[{"x":310,"y":260,"time":0.1}]"#).unwrap();std::fs::write(dir.join("metadata.json"),br#"{"width":336,"height":279,"x":-120,"y":37}"#).unwrap();
   let mut grid=SavedCfrGrid{completed_frames:30,video:focusflow_core::ffmpeg::ProbedVideoEvidence{video:focusflow_core::ffmpeg::ProbedVideo{width:336,height:280,duration_us:1_000_000},endpoint:focusflow_core::ffmpeg::PacketTime{ticks:30000,numerator:1,denominator:30000},first_packet_duration:focusflow_core::ffmpeg::PacketTime{ticks:1000,numerator:1,denominator:30000}}};
   assert!(Completion::new(manifest.clone(),summary(),0).unwrap().publish(&dir,&grid,false).is_err());
   let retained=decode_windows(&std::fs::read(dir.join("capture.json")).unwrap(),id).unwrap();assert_eq!(serde_json::to_value(&retained.source).unwrap(),frozen_source);assert!(retained.finalized_project().is_err());
   grid.video.video.width=encoded.0;grid.video.video.height=encoded.1;Completion::new(manifest,summary(),0).unwrap().publish(&dir,&grid,false).unwrap();
   let saved=decode_windows(&std::fs::read(dir.join("capture.json")).unwrap(),id).unwrap();assert_eq!(serde_json::to_value(&saved.source).unwrap(),frozen_source);
   let project=saved.finalized_project().unwrap();assert_eq!((project.video.width,project.video.height),encoded);assert_eq!(project.video.cursor,CursorMode::Embedded);assert!(project.cursor.is_none());assert!(project.clicks.is_empty()&&project.zooms.is_empty());assert_eq!(std::fs::read(dir.join("screen.mp4")).unwrap(),b"immutable media");std::fs::remove_dir_all(dir).unwrap();
  }
 }

 #[test] fn automatic_end_accepts_stop_during_finalization_without_inventing_an_ack(){
  use std::{sync::mpsc,time::{Instant,Duration}};
  for cause in ["closed","geometry","source"]{
   let now=Instant::now();let mut state=lifecycle::Lifecycle::new(now+Duration::from_secs(2));state.observe((2,2),(2,2),now,||Ok(0)).unwrap();state.arm(2_997_000).unwrap();
   let mut summary=summary();state.cutoff=Some(summary.cutoff);state.closed=cause=="closed";summary.closed=state.closed;state.geometry_changed=cause=="geometry";summary.geometry_changed=state.geometry_changed;if cause=="source"{summary.source_availability=Some(SourceAvailabilityEnd::Unavailable);}
   let(_tx,rx)=mpsc::sync_channel(3);let mut handled=false;let(ack,finished)=state.feed_end(&rx,&mut handled,30,30).unwrap();assert_eq!(ack,None);assert!(finished);assert!(!handled);
   // The feed has finished automatically; a later requested Stop keeps the same cutoff.
   state.control(3,Ok(3),||panic!("Late Stop cannot replace the automatic cutoff")).unwrap();assert_eq!(state.cutoff,Some(summary.cutoff));assert!(summary.check_stop_ack(true,ack).is_ok());assert!(summary.clock(0).is_ok());
  }
 }

}
