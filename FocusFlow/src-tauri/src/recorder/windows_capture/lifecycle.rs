//! Shared callback/control transitions; no native frame or process ownership.
use crate::recorder::owned_feed::{CaptureGate,FrameAction,Geometry};
use std::{io::Read,time::Instant};
type Result<T> = std::result::Result<T,String>;
pub(super) struct Lifecycle {
 pub gate:CaptureGate,pub cutoff:Option<i64>,pub closed:bool,pub geometry_changed:bool,pub failure:Option<String>,pub stop_ack:Option<i64>,pub gate2_at:Option<Instant>,
}
impl Lifecycle {
 pub fn new(deadline:Instant)->Self{Self{gate:CaptureGate::new(deadline),cutoff:None,closed:false,geometry_changed:false,failure:None,stop_ack:None,gate2_at:None}}
 pub fn terminal(&self)->bool{self.cutoff.is_some()||self.closed||self.geometry_changed||self.failure.is_some()}
 pub fn require_live(&self)->Result<()>{if self.terminal(){Err(self.failure.clone().unwrap_or_else(||"Window capture ended before admission; restore the window and retry".into()))}else{Ok(())}}
 pub fn geometry(&self)->Result<Geometry>{self.require_live()?;self.gate.geometry().ok_or_else(||"Window dimensions have not frozen".into())}
 pub fn observe(&mut self,content:(i32,i32),texture:(u32,u32),now:Instant,clock:impl FnOnce()->Result<i64>)->Result<FrameAction>{
  if self.terminal(){return Ok(FrameAction::Discard);}
  let action=match self.gate.observe(content,texture,now){Ok(action)=>action,Err(error)=>{self.failure=Some(error.into());return Err(error.into());}};
  if matches!(action,FrameAction::GeometryChanged){let now=match clock(){Ok(now)=>now,Err(error)=>{self.failure=Some(error.clone());return Err(error);}};self.cutoff=Some(now);self.geometry_changed=true;}Ok(action)
 }
 pub fn arm(&mut self,admission:i64)->Result<()>{self.require_live()?;self.gate.arm(admission).map_err(str::to_owned)}
 pub fn feed_end(&mut self,control:&std::sync::mpsc::Receiver<Result<()>>,handled:&mut bool,selected:u32,cells:u32)->Result<(Option<i64>,bool)>{
  if let Some(error)=&self.failure{return Err(error.clone());}
  let mut ack=None;
  if !*handled{
   // Stop is installed before channel publication; consume it before allowing the feed to finish.
   if let Some(cutoff)=self.stop_ack.take(){ack=Some(cutoff);*handled=true;}
   else{match control.try_recv(){Ok(result)=>{result?;return Err("Stop result arrived without a shared acknowledgement".into());},Err(std::sync::mpsc::TryRecvError::Disconnected)=>return Err("Parent control lost".into()),Err(std::sync::mpsc::TryRecvError::Empty)=>{}}}
  }
  Ok((ack,self.cutoff.is_some()&&selected==cells))
 }
 pub fn control(&mut self,expected:u8,result:Result<u8>,clock:impl FnOnce()->Result<i64>)->Result<()>{
  let result:Result<()>=(||{
   let byte=result?;
   if byte==3&&!matches!(self.gate,CaptureGate::Armed{..}){return Err("Parent cancelled window capture before admission".into());}
   if byte!=expected{return Err("Ownership/start gate cancelled or invalid".into());}
   if byte==2{self.gate2_at=Some(Instant::now());}
   if byte==3{if self.cutoff.is_none(){self.cutoff=Some(clock()?);}self.stop_ack=self.cutoff;}Ok(())
  })();
  if let Err(error)=&result{self.failure.get_or_insert(error.clone());}result
 }
}
// Latch errors/cancellation under the publication lock before publishing even queued earlier gates.
pub(super) fn ordered_controls(input:&mut impl Read,mut observe:impl FnMut(u8,Result<u8>)->Result<()>,mut publish:impl FnMut(Result<()>)->bool){
 for expected in [2,4,3]{let mut byte=[0];let read=input.read_exact(&mut byte).map(|_|byte[0]).map_err(|e|format!("Parent control ended: {e}"));let result=observe(expected,read);let terminal=result.is_err();if !publish(result)||terminal{return;}}
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum StartupStep {Freeze,Probe,Prepared,Gate2,SpawnEncoder,VerifyEncoder,EncoderMessage,Gate4,Park,Supervisor,Writer,Arm}
pub(super) trait StartupOps {
 fn step(&mut self,step:StartupStep)->Result<()>;
 fn failed(&mut self,error:&str);
 fn close_encoder(&mut self);
 fn kill_encoder(&mut self);
 fn wait_encoder(&mut self)->Result<()>;
}
// One fixed sequence serves the real worker and injected operations; no fake sequence can skip a gate.
pub(super) fn prepare(ops:&mut impl StartupOps)->Result<()> {
 use StartupStep::*;let mut encoder_created=false;
 for step in [Freeze,Probe,Prepared,Gate2,SpawnEncoder,VerifyEncoder,EncoderMessage,Gate4,Park,Supervisor,Writer,Arm]{
  if let Err(error)=ops.step(step){ops.failed(&error);if encoder_created{ops.close_encoder();ops.kill_encoder();if let Err(cleanup)=ops.wait_encoder(){return Err(format!("{error}; encoder cleanup: {cleanup}"));}}return Err(error);}
  if step==SpawnEncoder{encoder_created=true;}
 }Ok(())
}
// The exact capture is consumed once even when preparation fails after native start.
pub(super) fn with_capture<C:Send+'static,T>(capture:C,run:impl FnOnce(&C)->Result<T>,stop:impl FnOnce(C)->Result<()>+Send+'static,timeout:std::time::Duration)->Result<T>{
 let result=run(&capture);
 let owned=std::sync::Arc::new(std::sync::Mutex::new(Some((capture,stop))));let pending=owned.clone();
 let(tx,rx)=std::sync::mpsc::sync_channel(1);
 let spawned=std::thread::Builder::new().name("wgc-owned-stop".into()).spawn(move||{
  let result=pending.lock().map_err(|_|"Capture cleanup ownership poisoned".to_owned()).and_then(|mut slot|slot.take().ok_or_else(||"Capture cleanup already consumed".to_owned())).and_then(|(capture,stop)|stop(capture));let _=tx.send(result);
 });
 if let Err(error)=spawned{
  // A native fallback may block; the existing parent watchdog/Job is still authoritative.
  if let Some((capture,stop))=owned.lock().map_err(|e|e.to_string())?.take(){stop(capture)?;}
  return Err(format!("Could not start capture cleanup: {error}"));
 }
 let cleanup=rx.recv_timeout(timeout).map_err(|_|"WGC cleanup timeout; parent Job containment required; preserve incomplete files".to_owned()).and_then(|result|result);
 match(result,cleanup){(Err(error),Err(cleanup))=>Err(format!("{error}; capture cleanup: {cleanup}")),(result,Ok(()))=>result,(Ok(_),Err(cleanup))=>Err(cleanup)}
}
#[cfg(test)]
mod tests {
 use super::*;
 use std::time::{Duration,Instant};
 #[test] fn metadata_only_until_arm_and_terminal_is_absorbing(){
  let now=Instant::now();let mut state=Lifecycle::new(now+Duration::from_secs(2));
  assert!(matches!(state.observe((322,272),(336,279),now,||Ok(1)).unwrap(),FrameAction::Discard));
  assert!(state.gate.geometry().is_none());
  assert!(matches!(state.observe((322,272),(322,272),now,||Ok(1)).unwrap(),FrameAction::Discard));
  assert!(matches!(state.observe((322,272),(322,272),now,||Ok(1)).unwrap(),FrameAction::Discard));
  state.arm(3_000_000).unwrap();
  assert!(matches!(state.observe((322,272),(322,272),now,||Ok(1)).unwrap(),FrameAction::Copy{admission:3_000_000,..}));
  assert!(matches!(state.observe((336,279),(336,279),now,||Ok(4_000_000)).unwrap(),FrameAction::GeometryChanged));
  assert!(state.geometry_changed&&state.failure.is_none());assert!(state.arm(5_000_000).is_err());
  assert!(matches!(state.observe((322,272),(322,272),now,||panic!("terminal observation" )).unwrap(),FrameAction::Discard));
 }
 #[test] fn queued_gates_cannot_override_eof_or_early_stop(){
  for bytes in [vec![2,4],vec![2,4,3],vec![2,9],vec![3]] {
   let now=Instant::now();let mut state=Lifecycle::new(now+Duration::from_secs(2));
   state.observe((322,272),(322,272),now,||Ok(1)).unwrap();let mut queued=Vec::new();
   ordered_controls(&mut &bytes[..],|expected,result|state.control(expected,result,||Ok(22)),|result|{queued.push(result);true});
   assert!(state.failure.is_some());assert!(state.arm(10).is_err());assert!(state.cutoff.is_none());
  }
 }
 #[test] fn normal_stop_latches_one_cutoff_and_one_result(){
  let now=Instant::now();let mut state=Lifecycle::new(now+Duration::from_secs(2));state.observe((322,272),(322,272),now,||Ok(1)).unwrap();state.arm(10).unwrap();
  let mut results=Vec::new();ordered_controls(&mut &[2,4,3][..],|expected,result|state.control(expected,result,||Ok(22)),|result|{results.push(result);true});
  assert_eq!(results.len(),3);assert!(results.iter().all(Result::is_ok));assert_eq!(state.cutoff,Some(22));assert_eq!(state.stop_ack,Some(22));assert!(state.failure.is_none());
 }
 #[test] fn metadata_errors_deadlines_and_source_end_never_arm(){
  let now=Instant::now();
  for phase in [0,1,2]{
   for failure in ["invalid metadata","source owner changed","source unconfirmed","on_closed","parent EOF","parent Stop","unexpected byte"]{
    let mut state=Lifecycle::new(now+Duration::from_secs(2));
    if phase>0{state.observe((322,272),(322,272),now,||Ok(1)).unwrap();}if phase>1{state.arm(10).unwrap();}
    match failure {
     "invalid metadata"=>{if phase==0{assert!(state.observe((0,272),(322,272),now,||Ok(20)).is_err());}else{assert!(matches!(state.observe((0,272),(322,272),now,||Ok(20)).unwrap(),FrameAction::GeometryChanged));}},
     "on_closed"=>{state.closed=true;state.cutoff=Some(20);},
     "source owner changed"|"source unconfirmed"=>state.cutoff=Some(20),
     "parent EOF"=>{assert!(state.control(2,Err("EOF".into()),||Ok(20)).is_err());},
     "parent Stop"=>{let result=state.control(if phase==2{3}else{2},Ok(3),||Ok(20));assert_eq!(result.is_ok(),phase==2);},
     _=>{assert!(state.control(2,Ok(9),||Ok(20)).is_err());}
    }
    assert!(state.require_live().is_err(),"{phase}: {failure}");assert!(state.arm(30).is_err());
    assert!(matches!(state.observe((322,272),(322,272),now,||panic!("terminal callback")).unwrap(),FrameAction::Discard));
   }
  }
  for callback in [false,true]{let mut state=Lifecycle::new(now);assert!(state.geometry().is_err());if callback{assert!(state.observe((322,272),(322,272),now,||Ok(0)).is_err());}assert!(state.arm(0).is_err());}
 }
 #[test] fn owned_error_scope_consumes_capture_once_for_every_post_start_error(){
  use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
  // These injected Result errors exercise the production cleanup scope, not native API/Job behavior.
  for failure in ["invalid metadata","no callbacks","warm deadline","source owner changed","source unconfirmed","on_closed","EOF","early Stop","unexpected byte","capability probe","encoder spawn","encoder Job verification","gate4 timeout","writer setup","supervisor setup","emit"]{
   let consumed=Arc::new(AtomicUsize::new(0));let count=consumed.clone();
   let result:Result<()>=with_capture(42,|capture|{assert_eq!(*capture,42);Err(failure.into())},move|capture|{assert_eq!(capture,42);count.fetch_add(1,Ordering::SeqCst);Ok(())},Duration::from_secs(1));
   assert_eq!(result,Err(failure.into()));assert_eq!(consumed.load(Ordering::SeqCst),1);
  }
  let consumed=Arc::new(AtomicUsize::new(0));let count=consumed.clone();let result=with_capture(42,|_|Ok(()),move|_|{count.fetch_add(1,Ordering::SeqCst);Err("native stop error".into())},Duration::from_secs(1));assert_eq!(result,Err("native stop error".into()));assert_eq!(consumed.load(Ordering::SeqCst),1);
 }
 #[test] fn capture_cleanup_error_preserves_the_preparation_failure(){
  let result:Result<()>=with_capture(42,|_|Err("probe failed".into()),|_|Err("native stop failed".into()),Duration::from_secs(1));let error=result.unwrap_err();assert!(error.contains("probe failed")&&error.contains("native stop failed"));
 }
 #[test] fn simulated_blocked_stop_requests_parent_containment(){
  let(release_tx,release_rx)=std::sync::mpsc::channel();let(done_tx,done_rx)=std::sync::mpsc::channel();
  let result=with_capture(42,|_|Ok(()),move|capture|{assert_eq!(capture,42);release_rx.recv().unwrap();done_tx.send(()).unwrap();Ok(())},Duration::from_millis(5));
  assert!(result.unwrap_err().contains("parent Job containment required"));release_tx.send(()).unwrap();done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
 }

 struct FakeStartup {state:Lifecycle,now:Instant,fault:Option<(StartupStep,&'static str)>,visited:Vec<StartupStep>,encoder:bool,created:u32,closed:u32,killed:u32,waited:u32,ready:u32,gate2:bool,park_released:bool,failed:bool}
 impl FakeStartup {fn new(fault:Option<(StartupStep,&'static str)>)->Self{let now=Instant::now();Self{state:Lifecycle::new(now+Duration::from_secs(2)),now,fault,visited:vec![],encoder:false,created:0,closed:0,killed:0,waited:0,ready:0,gate2:false,park_released:false,failed:false}}}
 impl StartupOps for FakeStartup {
  fn step(&mut self,step:StartupStep)->Result<()> {
   use StartupStep::*;assert!(!self.failed,"No operation, especially Ready, may run after failure");self.visited.push(step);self.state.require_live()?;
   if let Some((at,cause))=self.fault{if at==step{return match cause{
    "invalid metadata"=>self.state.observe((0,272),(322,272),self.now,||Ok(0)).map(|_|()),
    "no callbacks"=>self.state.geometry().map(|_|()),
    "warm deadline"=>self.state.observe((322,272),(322,272),self.now+Duration::from_secs(2),||Ok(0)).map(|_|()),
    "EOF"=>self.state.control(2,Err("Parent EOF".into()),||Ok(9)),
    "early Stop"=>self.state.control(2,Ok(3),||Ok(9)),
    "unexpected byte"=>self.state.control(2,Ok(9),||Ok(9)),
    "on_closed"=>{self.state.cutoff=Some(9);self.state.closed=true;self.state.require_live()},
    _=>Err(cause.into()),
   };}}
   match step {
    Freeze=>{assert!(matches!(self.state.observe((322,272),(336,279),self.now,||Ok(0))?,FrameAction::Discard));assert!(matches!(self.state.observe((322,272),(322,272),self.now,||Ok(0))?,FrameAction::Discard));},
    Probe|Prepared=>{assert!(self.state.geometry().is_ok());},
    Gate2=>{self.state.control(2,Ok(2),||Ok(0))?;self.gate2=true;},
    SpawnEncoder=>{assert!(self.gate2);assert!(self.state.geometry().is_ok());assert!(!self.encoder);self.encoder=true;self.created+=1;},
    VerifyEncoder|EncoderMessage|Gate4|Park|Supervisor|Writer=>{assert!(self.encoder);if step==Gate4{self.state.control(4,Ok(4),||Ok(0))?;}if step==Park{self.park_released=true;}if matches!(step,Supervisor|Writer){assert!(self.park_released,"Writer/supervisor must not start before park releases");}},
    Arm=>{self.state.arm(3_000_000)?;let selector=crate::recorder::owned_feed::Selector::new(3_000_000,3_000_000)?;assert!(selector.epoch.is_none());self.ready+=1;},
   }
   if step!=Arm{assert!(matches!(self.state.observe((322,272),(322,272),self.now,||Ok(0))?,FrameAction::Discard),"Pre-arm callback policy must never authorize pixels");}Ok(())
  }
  fn failed(&mut self,error:&str){self.failed=true;self.state.failure.get_or_insert(error.into());}
  fn close_encoder(&mut self){assert!(self.encoder);self.closed+=1;}
  fn kill_encoder(&mut self){assert!(self.encoder);self.killed+=1;}
  fn wait_encoder(&mut self)->Result<()>{assert!(self.encoder);self.waited+=1;self.encoder=false;Ok(())}
 }
 #[test] fn real_startup_sequence_injects_faults_at_reachable_owned_operations(){
  use StartupStep::*;
  // All operations below are simulated. The sequencing/cleanup calls are the production prepare().
  let cases=[(Freeze,"invalid metadata"),(Freeze,"no callbacks"),(Freeze,"warm deadline"),(Freeze,"source owner changed"),(Prepared,"source unconfirmed"),(Prepared,"on_closed"),(Prepared,"EOF"),(Prepared,"early Stop"),(Gate2,"unexpected byte"),(Probe,"failed capability probe"),(SpawnEncoder,"encoder spawn"),(VerifyEncoder,"encoder Job verification"),(Gate4,"gate4 timeout"),(Writer,"writer setup"),(Supervisor,"supervisor setup"),(Prepared,"Prepared emit"),(EncoderMessage,"Encoder emit"),(Arm,"Ready emit"),(Park,"park failure"),(Gate4,"on_closed"),(Arm,"source owner changed")];
  for (step,cause) in cases {
   let mut ops=FakeStartup::new(Some((step,cause)));let cleanups=std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=cleanups.clone();
   let result=with_capture(42,|capture|{assert_eq!(*capture,42);prepare(&mut ops)},move|capture|{assert_eq!(capture,42);count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);Ok(())},Duration::from_secs(1));
   assert!(result.is_err(),"{step:?}: {cause}");assert!(ops.failed);assert_eq!(ops.ready,0);assert_eq!(ops.visited.last(),Some(&step));assert!(ops.state.require_live().is_err());assert!(!ops.encoder);
   let expected=u32::from(matches!(step,VerifyEncoder|EncoderMessage|Gate4|Park|Supervisor|Writer|Arm));assert_eq!((ops.created,ops.closed,ops.killed,ops.waited),(expected,expected,expected,expected),"{step:?}: {cause}");assert_eq!(cleanups.load(std::sync::atomic::Ordering::SeqCst),1);
   if step==Park{assert!(!ops.visited.contains(&Supervisor)&&!ops.visited.contains(&Writer)&&!ops.visited.contains(&Arm));}
  }
 }
 #[test] fn real_startup_sequence_has_one_encoder_and_admits_only_after_park_and_writer(){
  use StartupStep::*;let mut ops=FakeStartup::new(None);prepare(&mut ops).unwrap();assert_eq!(ops.visited,vec![Freeze,Probe,Prepared,Gate2,SpawnEncoder,VerifyEncoder,EncoderMessage,Gate4,Park,Supervisor,Writer,Arm]);assert_eq!((ops.created,ops.ready,ops.closed,ops.killed,ops.waited),(1,1,0,0,0));assert!(ops.encoder);
 }

 #[test] fn feed_ack_precedes_finish_when_reader_pauses_before_stop_publication(){
  use std::sync::{Arc,Mutex,mpsc};use crate::recorder::owned_feed::{Selector,Source,OwnedPixels,OwnedCount,complete_cells};
  for select_final_cell in [false,true]{
   let now=Instant::now();let state=Arc::new(Mutex::new(Lifecycle::new(now+Duration::from_secs(2))));let g=Geometry::new(2,2).unwrap();
   state.lock().unwrap().observe((2,2),(2,2),now,||Ok(0)).unwrap();
   let(tx,rx)=mpsc::sync_channel(3);let(armed_tx,armed_rx)=mpsc::channel();let(paused_tx,paused_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();let reader_state=state.clone();
   let reader=std::thread::spawn(move||{let mut published=0;ordered_controls(&mut &[2,4,3][..],|expected,result|{if expected==3{armed_rx.recv_timeout(Duration::from_secs(2)).unwrap();}reader_state.lock().unwrap().control(expected,result,||Ok(20_000_000))},|result|{published+=1;if published==3{paused_tx.send(()).unwrap();release_rx.recv_timeout(Duration::from_secs(2)).unwrap();}tx.send(result).is_ok()});});
   rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();state.lock().unwrap().arm(0).unwrap();armed_tx.send(()).unwrap();paused_rx.recv_timeout(Duration::from_secs(1)).unwrap();
   let mut selector=Selector::new(0,10_000_000).unwrap();selector.push(Source{seq:0,native:10_000_000,published:10_000_000,pixels:Arc::new(OwnedPixels::copy(&[0;16],8,g,Arc::new(OwnedCount::default())).unwrap())}).unwrap();
   let mut selected=0;for ordinal in 0..if select_final_cell{29}else{30}{selector.select(ordinal,20_000_000).unwrap();selected+=1;}
   if select_final_cell{selector.select(selected,20_000_000).unwrap();selected+=1;}
   let cells=complete_cells(selector.epoch.unwrap(),20_000_000,10_000_000).unwrap();let mut handled=false;
   let decision=state.lock().unwrap().feed_end(&rx,&mut handled,selected,cells);
   // Always release the real reader even if this assertion is about to fail.
   release_tx.send(()).unwrap();reader.join().unwrap();
   let(ack,finished)=decision.unwrap();assert_eq!(ack,Some(20_000_000),"Ack must be available before the complete recording can finish");assert!(finished);assert!(handled);
   for _ in 0..3{assert_eq!(state.lock().unwrap().feed_end(&rx,&mut handled,selected,cells).unwrap(),(None,true));}
   rx.try_recv().unwrap().unwrap();assert!(matches!(rx.try_recv(),Err(mpsc::TryRecvError::Disconnected)));
   assert_eq!(state.lock().unwrap().feed_end(&rx,&mut handled,selected,cells).unwrap(),(None,true));assert_eq!(state.lock().unwrap().cutoff,Some(20_000_000));
  }
 }
 #[test] fn feed_end_never_acknowledges_startup_cancellation_or_invents_automatic_stop(){
  use std::sync::mpsc;
  for bytes in [vec![2,4],vec![2,4,3]]{let now=Instant::now();let mut state=Lifecycle::new(now+Duration::from_secs(2));state.observe((2,2),(2,2),now,||Ok(0)).unwrap();let(tx,rx)=mpsc::sync_channel(3);ordered_controls(&mut &bytes[..],|expected,result|state.control(expected,result,||Ok(9)),|result|tx.send(result).is_ok());let mut handled=false;assert!(state.feed_end(&rx,&mut handled,30,30).is_err());assert!(!handled);assert_eq!(state.stop_ack,None);assert_eq!(state.cutoff,None);}
  for cause in ["closed","geometry","source"]{let now=Instant::now();let mut state=Lifecycle::new(now+Duration::from_secs(2));state.observe((2,2),(2,2),now,||Ok(0)).unwrap();state.arm(0).unwrap();state.cutoff=Some(20_000_000);state.closed=cause=="closed";state.geometry_changed=cause=="geometry";let(_tx,rx)=mpsc::sync_channel(3);let mut handled=false;for _ in 0..3{assert_eq!(state.feed_end(&rx,&mut handled,30,30).unwrap(),(None,true));}assert!(!handled);assert_eq!(state.stop_ack,None);}
 }

}
