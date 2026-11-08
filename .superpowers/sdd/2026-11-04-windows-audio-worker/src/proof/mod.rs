pub mod guard;
pub mod render;
use crate::{owned_child::{OwnedTrack,TrackOutcome},protocol::{TrackKind,TrackRequest},session::AudioSession,Result};
use guard::ProofPermit;
use std::{path::Path,time::{Instant,Duration}};
#[derive(Debug)]
pub struct Receipts {pub capture:Vec<TrackOutcome>,pub renderer:Option<TrackOutcome>,pub microphone:&'static str,pub qualification:&'static str}
#[derive(Debug)]
pub enum ProofResult {Unavailable{reason:String},Failed{reason:String,receipts:Receipts},Collected{receipts:Receipts,qualification_pending:bool},Passed{receipts:Receipts}}
impl Receipts {fn empty()->Self {Self{capture:Vec::new(),renderer:None,microphone:"Unqualified: no independently owned generated capture endpoint",qualification:"Not independently asset/signal qualified"}}}
#[derive(Debug,Clone,PartialEq,Eq)]
pub(crate) enum ProbeError { Unavailable(String), Failed(String) }
impl From<String> for ProbeError {fn from(reason:String)->Self {Self::Failed(reason)}}
#[cfg(windows)]
impl ProbeError {fn reason(self)->String {match self {Self::Unavailable(reason)|Self::Failed(reason)=>reason}}}
trait FixtureOps {
 fn probe(&mut self,permit:&ProofPermit,id:&str)->std::result::Result<(),ProbeError>;
 fn capture(&mut self,permit:&ProofPermit,request:TrackRequest,now:Instant)->Result<AudioSession>;
 fn renderer(&mut self,permit:&ProofPermit,request:TrackRequest,deadline:Instant)->Result<OwnedTrack>;
 fn now(&self)->Instant {Instant::now()}
 fn pause(&mut self){std::thread::sleep(Duration::from_millis(10));}
}
fn run_fixture_with(permit:ProofPermit,selected:&str,output:&Path,ops:&mut impl FixtureOps)->ProofResult {
 if selected!=permit.route() || !output.is_absolute() || !permit.executable().is_absolute() {return ProofResult::Unavailable{reason:"Select the exact independently approved owned render route and absolute fresh output parent".into()};}
 if let Err(error)=ops.probe(&permit,selected) {return match error {
  ProbeError::Unavailable(reason)=>ProofResult::Unavailable{reason},
  ProbeError::Failed(reason)=>ProofResult::Failed{reason,receipts:Receipts::empty()},
 };}
 let request=|name:&str|TrackRequest{kind:TrackKind::System,endpoint_id:selected.into(),explicitly_enabled:true,output_dir:output.join(name)};
 let mut capture=match ops.capture(&permit,request("capture"),ops.now()) {Ok(s)=>s,Err(reason)=>return ProofResult::Failed{reason,receipts:Receipts::empty()}};
 let mut failure=None;
 loop {
  let poll=capture.poll(ops.now());
  if poll.failed {failure=Some("Requested capture did not reach admission".into());break;}
  if poll.ready {if let Err(e)=capture.video_admitted(){failure=Some(e);}break;}
  ops.pause();
 }
 let mut renderer=None;
 if failure.is_none() {
  match ops.renderer(&permit,request("renderer"),ops.now()+Duration::from_millis(crate::protocol::STARTUP_MS)) {Ok(track)=>renderer=Some(track),Err(e)=>failure=Some(e)}
 }
 let mut rendered=None;
 if let Some(track)=renderer.as_mut() {
  let deadline=ops.now()+Duration::from_millis(crate::protocol::STARTUP_MS)+Duration::from_secs(13);
  loop {
   let poll=track.poll(ops.now());capture.poll(ops.now());
   if let Some(outcome)=poll.outcome {rendered=Some(outcome);break;}
   if track.failure().is_some_and(|reason|reason!="fixture complete") || !capture.outcomes().is_empty() {failure=Some("Owned capture or renderer ended before fixture completion".into());break;}
   if ops.now()>=deadline {failure=Some("Renderer exceeded its finite fixture budget".into());break;}
   ops.pause();
  }
 }
 // Direct Stop reaches both owners before any cleanup polling or Drop.
 let now=ops.now();capture.signal_all_stops(now);
 if let Some(track)=renderer.as_mut() {let _=track.request_stop(now);}
 loop {
  capture.poll(ops.now());
  if let Some(track)=renderer.as_mut() {if rendered.is_none(){rendered=track.poll(ops.now()).outcome;}}
  if !capture.outcomes().is_empty() && (renderer.is_none() || rendered.is_some()) {break;}
  ops.pause();
 }
 let receipts=Receipts{capture:capture.outcomes().to_vec(),renderer:rendered,microphone:"Unqualified: no independently owned generated capture endpoint",qualification:"Not independently asset/signal qualified"};
 let clean=|o:&TrackOutcome|o.exit==crate::owned_child::ExitObservation::Exited&&o.exit_code==Some(0)&&o.terminal.is_some();
 if failure.is_none() && (!receipts.capture.iter().all(|o|clean(o)&&o.failure.is_none()&&o.usable.is_some()) || !renderer.as_ref().is_some_and(OwnedTrack::completed_protocol) || !receipts.renderer.as_ref().is_some_and(|o|clean(o)&&o.failure.as_deref()==Some("fixture complete")&&o.terminal.as_ref().is_some_and(|s|s.cause=="fixture complete"))) {
  failure=Some("Collection requires clean exact child exits, reconciled capture inventory and complete renderer".into());
 }
 match failure {Some(reason)=>ProofResult::Failed{reason,receipts},None=>ProofResult::Collected{receipts,qualification_pending:true}}
}

pub fn run_render_fixture(permit:ProofPermit,selected_render_id:&str,output:&Path)->ProofResult {run_fixture_with(permit,selected_render_id,output,&mut PlatformFixture)}

struct PlatformFixture;
#[cfg(windows)]
impl FixtureOps for PlatformFixture {
 fn probe(&mut self,permit:&ProofPermit,id:&str)->std::result::Result<(),ProbeError> {render::native::probe_exact(permit,id)}
 fn capture(&mut self,permit:&ProofPermit,request:TrackRequest,now:Instant)->Result<AudioSession> {AudioSession::begin(permit.executable(),vec![request],now)}
 fn renderer(&mut self,permit:&ProofPermit,request:TrackRequest,deadline:Instant)->Result<OwnedTrack> {OwnedTrack::begin_private_render(permit,request,deadline)}
}

#[cfg(not(windows))]
impl FixtureOps for PlatformFixture {
 fn probe(&mut self,_:&ProofPermit,_:&str)->std::result::Result<(),ProbeError> {Err(ProbeError::Unavailable("Windows required".into()))}
 fn capture(&mut self,_:&ProofPermit,_:TrackRequest,_:Instant)->Result<AudioSession> {Err("Windows required".into())}
 fn renderer(&mut self,_:&ProofPermit,_:TrackRequest,_:Instant)->Result<OwnedTrack> {Err("Windows required".into())}
}

/// Both private parent and renderer verify approval before any native resource gate.
pub fn dispatch(mut args:impl Iterator<Item=std::ffi::OsString>)->i32 {
 args.next();let mode=args.next();
 if mode.as_deref().and_then(|s|s.to_str())==Some("--private-audio-startup-probe") {
  if args.next().is_some(){return 64;}
  #[cfg(windows)] {return guard::helper::startup::dispatch();}
  #[cfg(not(windows))] {return 69;}
 }
 if mode.as_deref().and_then(|s|s.to_str())==Some("--private-audio-controller") {
  if args.next().is_some(){return 64;}return guard::helper::dispatch_controller();
 }
 if mode.as_deref().and_then(|s|s.to_str())==Some("--private-audio-discover") {
  let values:Vec<_>=args.take(4).collect();
  if values.len()!=3 || values.iter().any(|s|s.to_str().is_none_or(|s|s.is_empty()||s.contains('\0'))){return 64;}
  #[cfg(windows)] {return guard::helper::dispatch_discovery(values[0].to_str().unwrap(),values[1].to_str().unwrap(),values[2].to_str().unwrap());}
  #[cfg(not(windows))] {return 69;}
 }

 // Root/source location is data only; approval is never loaded from CLI, files or environment here.
 let source=std::env::var_os("GITHUB_WORKSPACE").map(std::path::PathBuf::from).unwrap_or_default().join(".superpowers/sdd/2026-11-04-windows-audio-worker");
 let permit=match guard::authorize_environment(&source){Ok(p)=>p,Err(reason)=>{eprintln!("Private proof denied: {reason}");return 77;}};
 let values:Vec<_>=args.take(4).collect();
 match mode.as_deref().and_then(|s|s.to_str()) {
  Some("--private-audio-render")=>{
   if values.len()!=3 || values.iter().any(|s|s.to_str().is_none_or(|s|s.is_empty()||s.contains('\0'))){return 64;}
   #[cfg(windows)] {render::native::dispatch(permit,values[0].to_str().unwrap(),values[1].to_str().unwrap(),values[2].to_str().unwrap())}
   #[cfg(not(windows))] {let _=permit;69}
  },
  Some("--private-audio-proof")=>{
   if values.len()!=2 {return 64;}
   let Some(id)=values[0].to_str() else{return 64;};
   let result=run_render_fixture(permit,id,Path::new(&values[1]));
   eprintln!("{result:?}");
   match result {ProofResult::Unavailable{..}=>69,ProofResult::Failed{..}=>71,ProofResult::Collected{..}=>78,ProofResult::Passed{..}=>0}
  },_=>64,
 }
}
