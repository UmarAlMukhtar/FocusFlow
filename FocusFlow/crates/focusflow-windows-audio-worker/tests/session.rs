use focusflow_windows_audio_worker::{
    format::{Encoding, Format},
    owned_child::{ChildOps, ExitObservation, OwnedTrack, PipeRead, SharedRegistry},
    pending::ObservedExit,
    protocol::{self, ClockStamp, Event, EventV1, State, TerminalSummary, TrackKind, TrackRequest},
    session::AudioSession, Result,
};
use std::{collections::VecDeque, sync::{Arc,Mutex}, time::{Duration,Instant}};
const NONCE: &str = "12345678-1234-1234-1234-123456789abc";
fn format() -> Format { Format { rate:48000, channels:1, bits:16, valid_bits:16, align:2, encoding:Encoding::Pcm } }
#[derive(Default)]
struct Facts { bytes:VecDeque<u8>, eof:bool, exit:Option<i32>, fail_assign:bool, fail_signal:bool,
    observed:usize, forced:usize, closed:usize, read_bytes:usize, calls:Vec<String>, stamps:VecDeque<u64> }
struct Child { id:usize, facts:Arc<Mutex<Facts>>, trace:Arc<Mutex<Vec<String>>>, time:Arc<Mutex<(Instant,u64)>> }
impl Child { fn call(&self, name:&str) { self.trace.lock().unwrap_or_else(|e|e.into_inner()).push(format!("{}:{name}",self.id)); self.facts.lock().unwrap_or_else(|e|e.into_inner()).calls.push(name.into()); } }
impl ChildOps for Child {
    fn assign_job(&mut self)->Result<()> { self.call("assign"); if self.facts.lock().unwrap_or_else(|e|e.into_inner()).fail_assign {Err("assignment failed".into())} else {Ok(())} }
    fn send_init(&mut self, frame:&[u8])->Result<()> {self.call("init"); protocol::decode_init_frame(frame,NONCE).map_err(|e|e.to_string())?; Ok(())}
    fn read_stdout(&mut self, buf:&mut[u8])->Result<PipeRead> { self.call("read"); let mut f=self.facts.lock().unwrap_or_else(|e|e.into_inner()); let n=buf.len().min(f.bytes.len()); for b in &mut buf[..n] {*b=f.bytes.pop_front().unwrap();} f.read_bytes+=n; Ok(if n>0 {PipeRead::Data(n)} else if f.eof {PipeRead::Eof} else {PipeRead::Empty}) }
    fn clock(&mut self)->Result<(Instant,ClockStamp)> { self.call("clock"); let (now,qpc)=*self.time.lock().unwrap_or_else(|e|e.into_inner()); let qpc=self.facts.lock().unwrap_or_else(|e|e.into_inner()).stamps.pop_front().unwrap_or(qpc); Ok((now,ClockStamp{qpc,frequency:1000})) }
    fn observe_exit(&mut self)->Result<Option<ObservedExit>> {self.call("observe"); let mut f=self.facts.lock().unwrap_or_else(|e|e.into_inner()); f.observed+=1; Ok(f.exit.map(|code|ObservedExit{code:Some(code)}))}
    fn signal_stop(&mut self)->Result<()> {self.call("signal"); if self.facts.lock().unwrap_or_else(|e|e.into_inner()).fail_signal {Err("signal failed".into())} else {Ok(())}}
    fn close_control(&mut self) {self.call("close"); self.facts.lock().unwrap_or_else(|e|e.into_inner()).closed+=1;}
    fn force(&mut self)->Result<()> {self.call("force"); self.facts.lock().unwrap_or_else(|e|e.into_inner()).forced+=1; Ok(())}
}
struct Harness { now:Instant, time:Arc<Mutex<(Instant,u64)>>, trace:Arc<Mutex<Vec<String>>>, facts:Vec<Arc<Mutex<Facts>>>, registry:SharedRegistry }
impl Harness {
    fn new()->Self {let now=Instant::now(); Self{now,time:Arc::new(Mutex::new((now,1000))),trace:Arc::default(),facts:vec![],registry:SharedRegistry::new()}}
    fn at(&self,ms:u64)->Instant {let now=self.now+Duration::from_millis(ms); *self.time.lock().unwrap_or_else(|e|e.into_inner())=(now,1000+ms); now}
    fn child(&mut self)->Box<dyn ChildOps+Send> {let id=self.facts.len(); let facts=Arc::new(Mutex::new(Facts::default())); self.facts.push(facts.clone()); Box::new(Child{id,facts,trace:self.trace.clone(),time:self.time.clone()})}
    fn request(kind:TrackKind)->TrackRequest {TrackRequest{kind,endpoint_id:"exact-id".into(),explicitly_enabled:true,output_dir:std::env::temp_dir().join("focusflow-injected-only")}}
    fn track(&mut self)->OwnedTrack {let child=self.child(); OwnedTrack::begin_with(Self::request(TrackKind::Microphone),self.now+Duration::from_secs(8),NONCE.into(),self.registry.clone(),||Ok(child)).unwrap()}
    fn session(&mut self)->AudioSession {let a=self.child(); let b=self.child(); let mut children=VecDeque::from([a,b]); AudioSession::begin_with(vec![Self::request(TrackKind::Microphone),Self::request(TrackKind::System)], self.now,self.registry.clone(), ||Ok(NONCE.into()), |_|Ok(children.pop_front().unwrap())).unwrap()}
    fn event(&self,id:usize,kind:TrackKind,event:Event) { self.facts[id].lock().unwrap_or_else(|e|e.into_inner()).bytes.extend(protocol::encode_event(&EventV1{version:1,nonce:NONCE.into(),kind,event}).unwrap()); }
    fn ready(&self,id:usize,kind:TrackKind) {self.event(id,kind,Event::Ready{format:format()});}
    fn release(&self,id:usize,kind:TrackKind,qpc:u64) {self.event(id,kind,Event::StopAck{observed:ClockStamp{qpc,frequency:1000}});self.event(id,kind,Event::CaptureReleased{at:ClockStamp{qpc,frequency:1000}});}
    fn ending(&self,id:usize,kind:TrackKind,qpc:u64) {self.event(id,kind,Event::Ending{cause:"endpoint failed".into(),observed:ClockStamp{qpc,frequency:1000}});}
}
fn terminal()->TerminalSummary {TerminalSummary{accepted_frames:0,written_complete_frames:0,synchronized_frames:Some(0),usable:None,retained_partial:true,cause:"stopped".into(),evidence_present:false}}
#[test]
fn two_requested_tracks_share_startup_deadline() {let mut h=Harness::new();let mut s=h.session();h.ready(0,TrackKind::Microphone);assert!(!s.poll(h.at(7999)).ready);s.poll(h.at(8000));assert_eq!(h.facts[1].lock().unwrap_or_else(|e|e.into_inner()).forced,1);assert!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));s.poll(h.at(11000));assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1); }
#[test]
fn startup_failure_stops_every_requested_track() {let mut h=Harness::new();let mut s=h.session();h.ending(0,TrackKind::Microphone,1000);let p=s.poll(h.now);assert!(p.failed);for f in &h.facts {assert!(f.lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));}assert!(s.video_admitted().is_err());}
#[test]
fn post_admission_failure_preserves_sibling() {let mut h=Harness::new();let mut s=h.session();h.ready(0,TrackKind::Microphone);h.ready(1,TrackKind::System);assert!(s.poll(h.now).ready);s.video_admitted().unwrap();h.ending(0,TrackKind::Microphone,1000);s.poll(h.now);assert!(!h.facts[1].lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));}
#[test]
fn signal_all_precedes_any_wait() {let mut h=Harness::new();let mut s=h.session();h.trace.lock().unwrap_or_else(|e|e.into_inner()).clear();s.signal_all_stops(h.now);let trace=h.trace.lock().unwrap_or_else(|e|e.into_inner()).clone();assert_eq!(trace,vec!["0:signal","1:signal"]);s.poll(h.at(3000));let trace=h.trace.lock().unwrap_or_else(|e|e.into_inner()).clone();assert!(trace.iter().position(|x|x=="1:signal").unwrap()<trace.iter().position(|x|x=="0:observe").unwrap());}
#[test]
fn unexpected_eof_and_terminal_without_exit_are_unfinished() {let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);t.poll(h.now);t.request_stop(h.now).unwrap();h.release(0,TrackKind::Microphone,1000);h.event(0,TrackKind::Microphone,Event::Terminal{outcome:terminal()});assert!(t.poll(h.now).outcome.is_none());h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).eof=true;assert!(t.poll(h.now).outcome.is_none());assert_eq!(t.poll(h.at(120000)).phase,State::ForceExit);
 let mut h=Harness::new();let mut t=h.track();h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).eof=true;assert_eq!(t.poll(h.now).phase,State::Stopping);assert!(t.poll(h.at(3000)).outcome.is_none());}
#[test]
fn recovery_hang_uses_original_deadline() {let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);h.event(0,TrackKind::Microphone,Event::RecoveryStarted{episode:1,first_failure:ClockStamp{qpc:1000,frequency:1000}});t.poll(h.at(2000));t.request_stop(h.at(4000)).unwrap();t.request_stop(h.at(4999)).unwrap();assert_eq!(t.poll(h.at(5000)).phase,State::ForceExit);assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1);}
#[test]
fn force_once_then_pending_exact_reap() {let mut h=Harness::new();let mut t=h.track();t.poll(h.at(8000));t.poll(h.at(9999));assert_eq!(h.registry.len(),0);let p=t.poll(h.at(10000));assert!(p.outcome.as_ref().unwrap().failure.is_some(),"deadline failures retain a reason alongside cleanup ownership");assert_eq!(p.outcome.unwrap().exit,ExitObservation::CleanupPending);assert_eq!(h.registry.len(),1);t.request_stop(h.at(10001)).unwrap();t.cancel_storage(h.at(10002)).unwrap();t.poll(h.at(15000));assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1);h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(71);assert_eq!(t.poll(h.at(15001)).outcome.unwrap().exit,ExitObservation::Exited);assert_eq!(h.registry.len(),0);drop(t);assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1);}
#[test]
fn retained_live_owner_blocks_replacement() {let mut h=Harness::new();let mut t=h.track();t.poll(h.at(8000));t.poll(h.at(10000));assert!(!h.registry.replacement_allowed());let child=h.child();assert!(OwnedTrack::begin_with(Harness::request(TrackKind::System),h.at(18000),NONCE.into(),h.registry.clone(),||Ok(child)).is_err());assert_eq!(h.registry.len(),1);h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(71);assert_eq!(h.registry.reap(),0);assert!(h.registry.replacement_allowed());assert_eq!(t.poll(h.at(10001)).outcome.unwrap().exit_code,Some(71));}
#[test]
fn storage_deadline_is_not_restarted() {let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);t.poll(h.now);t.request_stop(h.now).unwrap();h.release(0,TrackKind::Microphone,1000);t.poll(h.at(1000));t.request_stop(h.at(50000)).unwrap();assert_eq!(t.poll(h.at(119999)).phase,State::StorageFinalizing);assert_eq!(t.poll(h.at(120000)).phase,State::ForceExit);}
#[test]
fn cancel_storage_retains_partial_without_usable() {let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);t.poll(h.now);t.request_stop(h.now).unwrap();h.release(0,TrackKind::Microphone,1000);t.poll(h.now);t.cancel_storage(h.at(1)).unwrap();t.cancel_storage(h.at(1000)).unwrap();let p=t.poll(h.at(2001));assert_eq!(p.phase,State::CleanupPending);assert!(p.outcome.unwrap().usable.is_none());assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1);}
#[test]
fn gate_failure_signals_all_and_retains_original_child() {let mut h=Harness::new();let a=h.child();let b=h.child();h.facts[1].lock().unwrap_or_else(|e|e.into_inner()).fail_assign=true;let mut q=VecDeque::from([a,b]);let mut s=AudioSession::begin_with(vec![Harness::request(TrackKind::Microphone),Harness::request(TrackKind::System)],h.now,h.registry.clone(),||Ok(NONCE.into()),|_|Ok(q.pop_front().unwrap())).unwrap();let trace=h.trace.lock().unwrap_or_else(|e|e.into_inner()).clone();assert!(!trace.contains(&"1:init".into()));assert!(trace.contains(&"0:signal".into())&&trace.contains(&"1:signal".into()));s.poll(h.at(3000));s.poll(h.at(5000));assert_eq!(s.outcomes().len(),2);assert_eq!(h.registry.len(),2);}
#[test]
fn spawn_failure_returns_each_failure_and_started_cleanup() {let mut h=Harness::new();let a=h.child();let mut q=Some(a);let mut s=AudioSession::begin_with(vec![Harness::request(TrackKind::Microphone),Harness::request(TrackKind::System)],h.now,h.registry.clone(),||Ok(NONCE.into()),|_|q.take().ok_or("spawn failed".into())).unwrap();assert_eq!(s.outcomes().len(),1);assert_eq!(s.outcomes()[0].exit,ExitObservation::NotStarted);assert!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));s.poll(h.at(3000));s.poll(h.at(5000));assert_eq!(s.outcomes().len(),2);}
#[test]
fn drop_signals_all_before_force_and_never_registers_twice() {let mut h=Harness::new();let s=h.session();h.trace.lock().unwrap_or_else(|e|e.into_inner()).clear();drop(s);let tr=h.trace.lock().unwrap_or_else(|e|e.into_inner()).clone();for id in 0..2 {assert!(tr.iter().position(|x|x=="1:signal").unwrap()<tr.iter().position(|x|x==&format!("{id}:force")).unwrap());}assert_eq!(h.registry.len(),2);assert_eq!(h.registry.reap(),2);for f in &h.facts {assert_eq!(f.lock().unwrap_or_else(|e|e.into_inner()).forced,1);}}
#[test]
fn poll_bounds_events_bytes_and_observes_under_flood_or_malformed_output() {let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);for _ in 0..100 {h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).bytes.extend(b"x\n");}t.poll(h.now);let f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());assert!(f.read_bytes<=16384);assert_eq!(f.observed,1);drop(f);t.poll(h.at(3000));assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1);}
#[test]
fn frame_clock_is_read_after_bytes_and_rejects_future_stamp() {let mut h=Harness::new();let mut t=h.track();h.trace.lock().unwrap_or_else(|e|e.into_inner()).clear();h.ready(0,TrackKind::Microphone);h.event(0,TrackKind::Microphone,Event::RecoveryStarted{episode:1,first_failure:ClockStamp{qpc:1001,frequency:1000}});h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).stamps.extend([1000,1001]);assert_eq!(t.poll(h.now).phase,State::Recovering);let tr=h.trace.lock().unwrap_or_else(|e|e.into_inner()).clone();assert!(tr.iter().position(|x|x=="0:read").unwrap()<tr.iter().position(|x|x=="0:clock").unwrap());drop(tr);
 let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);h.event(0,TrackKind::Microphone,Event::RecoveryStarted{episode:1,first_failure:ClockStamp{qpc:1001,frequency:1000}});assert_eq!(t.poll(h.now).phase,State::Stopping);}
#[test]
fn disabled_duplicate_or_missing_selection_never_spawns() {let mut h=Harness::new();let mut r=Harness::request(TrackKind::Microphone);r.explicitly_enabled=false;let result=AudioSession::begin_with(vec![r],h.now,h.registry.clone(),||Ok(NONCE.into()),|_|{h.child();Err("unexpected spawn".into())});assert!(result.is_err());assert!(h.facts.is_empty());let r=Harness::request(TrackKind::Microphone);assert!(AudioSession::begin_with(vec![r.clone(),r],h.now,h.registry.clone(),||Ok(NONCE.into()),|_|panic!("no duplicate spawn")).is_err());}
#[test]
fn final_admission_poll_catches_ready_then_failure() {let mut h=Harness::new();let mut s=h.session();h.ready(0,TrackKind::Microphone);h.ready(1,TrackKind::System);assert!(s.poll(h.now).ready);h.ending(1,TrackKind::System,1000);assert!(s.video_admitted().is_err());assert!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));}
#[test]
fn all_signal_errors_are_returned_after_attempting_sibling() {let mut h=Harness::new();let mut s=h.session();h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).fail_signal=true;assert_eq!(s.signal_all_stops(h.now).len(),1);assert!(h.facts[1].lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));}
#[test]
fn exited_child_retains_bounded_stdout_until_terminal_is_decoded() {
 let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);
 h.event(0,TrackKind::Microphone,Event::RecoveryStarted{episode:1,first_failure:ClockStamp{qpc:1000,frequency:1000}});
 h.event(0,TrackKind::Microphone,Event::Recovered{episode:1,generation:1});h.ending(0,TrackKind::Microphone,1000);
 h.event(0,TrackKind::Microphone,Event::CaptureReleased{at:ClockStamp{qpc:1000,frequency:1000}});
 h.event(0,TrackKind::Microphone,Event::Terminal{outcome:terminal()});
 {let mut f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());f.exit=Some(0);f.eof=true;}
 assert!(t.poll(h.now).outcome.is_none(),"four-event poll must not discard unread Terminal on observed exit");
 let p=t.poll(h.now);let outcome=p.outcome.unwrap();assert!(outcome.terminal.is_some());assert_eq!(outcome.exit_code,Some(0));
 assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).observed,1);
}
#[test]
fn four_event_poll_retains_fifth_event_and_oversize_still_observes() {
 let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);
 for episode in 1..=2 {h.event(0,TrackKind::Microphone,Event::RecoveryStarted{episode,first_failure:ClockStamp{qpc:1000,frequency:1000}});h.event(0,TrackKind::Microphone,Event::Recovered{episode,generation:episode});}
 assert_eq!(t.poll(h.now).phase,State::Recovering);assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).observed,1);
 assert_eq!(t.poll(h.now).phase,State::Running);
 let mut h=Harness::new();let mut t=h.track();h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).bytes.extend(vec![b'x';20000]);
 assert_eq!(t.poll(h.now).phase,State::Stopping);let f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());assert_eq!(f.read_bytes,4096);assert_eq!(f.observed,1);
}
#[test]
fn metadata_cancellation_signals_every_track_before_force() {
 let mut h=Harness::new();let mut s=h.session();h.trace.lock().unwrap_or_else(|e|e.into_inner()).clear();s.cancel(h.now);
 let tr=h.trace.lock().unwrap_or_else(|e|e.into_inner()).clone();assert_eq!(&tr[..2],&["0:signal","1:signal"]);
 assert!(s.video_admitted().is_err());drop(s);assert_eq!(h.registry.len(),2);
 for f in &h.facts {assert_eq!(f.lock().unwrap_or_else(|e|e.into_inner()).forced,1);}
}
#[test]
fn retained_owner_drop_never_forces_or_registers_again() {
 let mut h=Harness::new();let mut t=h.track();t.poll(h.at(8000));t.poll(h.at(10000));let count=h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).observed;
 drop(t);assert_eq!(h.registry.len(),1);let f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());assert_eq!(f.forced,1);assert_eq!(f.observed,count);drop(f);
 assert_eq!(h.registry.reap(),1);assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).observed,count+1);
}
fn one_frame_from_actual_writer()->(std::path::PathBuf,TerminalSummary) {
 use focusflow_windows_audio_worker::{writer,packet::{Track,SILENT}};
 static N:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
 let dir=std::env::temp_dir().join(format!("focusflow-parent-writer-{}-{}",std::process::id(),N.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
 std::fs::create_dir(&dir).unwrap();let mut writer=writer::spawn_writer(&dir.join("source.partial.wav"),format()).unwrap();
 let deadline=Instant::now()+Duration::from_secs(5);
 while !writer.poll_created().unwrap(){assert!(Instant::now()<deadline);std::thread::yield_now();}
 let mut track=Track::new(format(),writer.take_sender().unwrap()).unwrap();track.admit(1,SILENT,0,1000,None).unwrap();let accepted=track.accepted_frames;drop(track);
 let outcome=loop {if let Some(out)=writer.poll_completion().unwrap(){break out.reconcile(accepted,None);}assert!(Instant::now()<deadline);std::thread::yield_now();};
 assert_eq!(outcome.usable.as_ref().unwrap().frames,1);
 let summary=TerminalSummary{accepted_frames:accepted,written_complete_frames:outcome.written_complete_frames,
 synchronized_frames:outcome.synchronized_frames,usable:outcome.usable,retained_partial:outcome.retained_partial,cause:outcome.reason,evidence_present:outcome.evidence_present};
 (dir,summary)
}
#[test]
fn actual_writer_inventory_is_producer_validated_and_requires_clean_exact_exit() {
 let (dir,summary)=one_frame_from_actual_writer();
 for mode in 0..5 {
  let mut h=Harness::new();let child=h.child();let mut request=Harness::request(TrackKind::Microphone);request.output_dir=dir.clone();
  let mut t=OwnedTrack::begin_with(request,h.now+Duration::from_secs(8),NONCE.into(),h.registry.clone(),||Ok(child)).unwrap();
  h.ready(0,TrackKind::Microphone);t.poll(h.now);t.request_stop(h.now).unwrap();h.release(0,TrackKind::Microphone,1000);
  let mut wire=summary.clone();if mode==2 {wire.usable.as_mut().unwrap().rate=44100;}if mode==3 {wire.accepted_frames=2;}
  if mode!=4 {let mut bytes=serde_json::to_vec(&EventV1{version:1,nonce:NONCE.into(),kind:TrackKind::Microphone,event:Event::Terminal{outcome:wire}}).unwrap();bytes.push(b'\n');h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).bytes.extend(bytes);}
  assert!(t.poll(h.now).outcome.is_none(),"valid file and Terminal alone do not establish exact exit");
  {let mut f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());f.exit=Some(if mode==1 {7}else {0});f.eof=true;}
  let out=t.poll(h.now).outcome.unwrap();assert_eq!(out.usable.is_some(),mode==0,"mode {mode}");assert_eq!(out.exit,ExitObservation::Exited);
 }
 // Parent outcome is inventory, not an independently reread published asset; import still validates.
 std::fs::write(dir.join("source.partial.wav"),b"corrupted after producer validation").unwrap();
 assert!(!focusflow_core::audio::is_usable_wav(&dir.join("source.partial.wav")));
 std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn early_storage_eof_and_malformed_terminal_never_publish_inventory() {
 for trailing in [b"garbage\n".as_slice(),b"{\"unknown\":true}\n".as_slice()] {
  let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);t.poll(h.now);t.request_stop(h.now).unwrap();h.release(0,TrackKind::Microphone,1000);
  h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).bytes.extend(trailing);assert_eq!(t.poll(h.now).phase,State::ForceExit);
  assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,1);
 }
 let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);t.poll(h.now);t.request_stop(h.now).unwrap();h.release(0,TrackKind::Microphone,1000);t.poll(h.now);
 h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).eof=true;assert_eq!(t.poll(h.now).phase,State::ForceExit);
}
#[test]
fn preadmission_recovery_cannot_extend_shared_eight_second_barrier() {
 let mut h=Harness::new();let mut s=h.session();h.ready(0,TrackKind::Microphone);h.ready(1,TrackKind::System);s.poll(h.now);
 h.event(1,TrackKind::System,Event::RecoveryStarted{episode:1,first_failure:ClockStamp{qpc:8000,frequency:1000}});
 assert!(!s.poll(h.at(7000)).ready);assert!(s.poll(h.at(8000)).failed);
 for f in &h.facts {assert!(f.lock().unwrap_or_else(|e|e.into_inner()).calls.contains(&"signal".into()));}
}
#[test]
fn init_frame_budget_is_checked_before_os_spawn() {
 let mut h=Harness::new();let mut request=Harness::request(TrackKind::Microphone);request.endpoint_id="\n".repeat(2048);
 let mut spawned=0;let s=AudioSession::begin_with(vec![request],h.now,h.registry.clone(),||Ok(NONCE.into()),|_|{spawned+=1;Ok(h.child())}).unwrap();
 assert_eq!(spawned,0);assert_eq!(s.outcomes()[0].exit,ExitObservation::NotStarted);assert!(s.outcomes()[0].failure.as_ref().unwrap().contains("4096"));
}
#[test]
fn padded_legal_frames_obey_sixteen_kib_and_four_event_limits() {
 let mut h=Harness::new();let mut t=h.track();let events=[Event::Ready{format:format()},
 Event::RecoveryStarted{episode:1,first_failure:ClockStamp{qpc:1000,frequency:1000}},Event::Recovered{episode:1,generation:1},
 Event::RecoveryStarted{episode:2,first_failure:ClockStamp{qpc:1000,frequency:1000}},Event::Recovered{episode:2,generation:2}];
 for event in events {let mut bytes=protocol::encode_event(&EventV1{version:1,nonce:NONCE.into(),kind:TrackKind::Microphone,event}).unwrap();bytes.pop();bytes.resize(4095,b' ');bytes.push(b'\n');h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).bytes.extend(bytes);}
 assert_eq!(t.poll(h.now).phase,State::Recovering);assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).read_bytes,16384);
 assert_eq!(t.poll(h.now).phase,State::Running);
}
#[test]
fn observed_exit_without_terminal_cannot_remain_running_on_empty_pipe() {
 let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);t.poll(h.now);
 h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(0);
 assert!(t.poll(h.now).outcome.is_none());let p=t.poll(h.now);assert_eq!(p.phase,State::Exited);assert!(p.outcome.unwrap().usable.is_none());
 assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).observed,2);
}
#[test]
fn drop_after_exact_exit_never_forces_or_registers_known_dead_child() {
 let mut h=Harness::new();let mut t=h.track();h.ready(0,TrackKind::Microphone);h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(0);
 assert!(t.poll(h.now).outcome.is_none());drop(t);assert_eq!(h.registry.len(),0);assert_eq!(h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).forced,0);
}
#[test]
fn session_reports_each_owned_cleanup_phase_with_failure_notice() {
 let mut h=Harness::new();let mut s=h.session();h.ending(0,TrackKind::Microphone,1000);let p=s.poll(h.now);
 assert!(p.failed);assert_eq!(p.phases,vec![(TrackKind::Microphone,State::Stopping),(TrackKind::System,State::Stopping)]);
 assert!(p.notices.iter().any(|(kind,_)|*kind==TrackKind::Microphone));assert!(s.outcomes().is_empty());
}
#[test]
fn no_requested_tracks_never_spawn_or_expire_an_audio_admission_barrier() {
 let h=Harness::new();let mut s=AudioSession::begin_with(vec![],h.now,h.registry.clone(),||panic!("no nonce"),|_|panic!("no spawn")).unwrap();
 assert!(s.poll(h.at(9000)).ready);s.video_admitted().unwrap();assert!(s.outcomes().is_empty());
}
#[test]
fn forced_exit_inside_observation_window_finishes_without_requery_even_zero() {
 for code in [0,71] {
  let mut h=Harness::new();let mut t=h.track();assert_eq!(t.poll(h.at(8000)).phase,State::ForceExit);
  h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(code);
  let p=t.poll(h.at(8001));assert_eq!(p.phase,State::Exited);let out=p.outcome.unwrap();
  assert_eq!(out.exit,ExitObservation::Exited);assert_eq!(out.exit_code,Some(code));assert!(out.usable.is_none());assert!(out.failure.is_some());assert_eq!(h.registry.len(),0);
  for ms in [9999,10000,30000] {let p=t.poll(h.at(ms));assert_eq!(p.phase,State::Exited);assert!(p.outcome.unwrap().usable.is_none());}
  t.request_stop(h.at(30001)).unwrap();t.cancel_storage(h.at(30002)).unwrap();drop(t);
  let f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());assert_eq!(f.observed,2);assert_eq!(f.forced,1);assert_eq!(f.closed,1);assert_eq!(h.registry.len(),0);
 }
}
#[test]
fn forced_exit_at_observation_deadline_finishes_without_pending_even_zero() {
 for code in [0,71] {
  let mut h=Harness::new();let mut t=h.track();t.poll(h.at(8000));h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(code);
  let p=t.poll(h.at(10000));assert_eq!(p.phase,State::Exited);let out=p.outcome.unwrap();assert_eq!(out.exit_code,Some(code));assert_eq!(out.exit,ExitObservation::Exited);assert!(out.failure.is_some());assert!(out.usable.is_none());assert_eq!(h.registry.len(),0);
  t.poll(h.at(30000));drop(t);let f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());assert_eq!(f.observed,2);assert_eq!(f.forced,1);
 }
}
#[test]
fn unobserved_force_expiry_still_transfers_then_exact_reaps_at_original_deadline() {
 for code in [0,71] {
  let mut h=Harness::new();let mut t=h.track();t.poll(h.at(8000));assert_eq!(t.poll(h.at(9999)).phase,State::ForceExit);assert_eq!(h.registry.len(),0);
  let p=t.poll(h.at(10000));assert_eq!(p.phase,State::CleanupPending);assert_eq!(p.outcome.unwrap().exit,ExitObservation::CleanupPending);assert_eq!(h.registry.len(),1);
  t.request_stop(h.at(10000)).unwrap();t.cancel_storage(h.at(10000)).unwrap();assert_eq!(h.registry.len(),1);
  h.facts[0].lock().unwrap_or_else(|e|e.into_inner()).exit=Some(code);let p=t.poll(h.at(10001));assert_eq!(p.phase,State::Exited);let out=p.outcome.unwrap();assert_eq!(out.exit_code,Some(code));assert_eq!(out.exit,ExitObservation::Exited);assert!(out.usable.is_none());assert!(out.failure.is_some());assert_eq!(h.registry.len(),0);
  t.poll(h.at(30000));drop(t);let f=h.facts[0].lock().unwrap_or_else(|e|e.into_inner());assert_eq!(f.observed,4);assert_eq!(f.forced,1);assert_eq!(f.closed,1);
 }
}
