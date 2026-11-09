use focusflow_windows_audio_worker::{Result, format::{Format,Encoding}, packet::{Track,SILENT,frame_cap}, protocol::{self,TrackKind,TrackRequest,InitV1,ValidatedInit,Event,EventV1,ClockStamp,Owner,Tick}, worker::{self,WorkerOps,Control,Wake,PacketPoll,NativeFault,WorkerExit}, wav::FinalizeOps};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,Condvar,mpsc},time::{Instant,Duration},fs::{self,File},thread};
const NONCE:&str="12345678-1234-1234-1234-123456789abc";
fn format()->Format { Format{rate:48000,channels:1,bits:16,valid_bits:16,align:2,encoding:Encoding::Pcm} }
struct Dir(PathBuf);
impl Dir {fn new()->Self {static N:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);let p=std::env::temp_dir().join(format!("focusflow-worker-{}-{}",std::process::id(),N.fetch_add(1,std::sync::atomic::Ordering::SeqCst)));fs::create_dir(&p).unwrap();Self(p)} }
impl Drop for Dir {fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
fn init(p:&Path)->ValidatedInit {protocol::decode_init_frame(&protocol::encode_init(&InitV1{version:1,nonce:NONCE.into(),request:TrackRequest{kind:TrackKind::Microphone,endpoint_id:"exact-id".into(),explicitly_enabled:true,output_dir:p.into()}}).unwrap(),NONCE).unwrap()}
#[derive(Default)]struct Shared {calls:Vec<String>,events:Vec<EventV1>,native_live:bool,opens:usize,packets:usize,acquisitions:usize,releases:usize,wait_packets:Vec<usize>}
struct Ops {shared:Arc<Mutex<Shared>>,fault:Option<&'static str>,fault_used:bool,base:Instant,ms:u64,controls:usize,stop_at:usize,empty:bool,invalidate:usize,reopen_failures:usize,changed:bool,late:bool,wall:bool,cross:bool,finalize:Option<Box<dyn FinalizeOps+Send>>,owner:Arc<Mutex<Owner>>,interrupt:Option<(&'static str,Control)>,append_entered:Option<mpsc::Receiver<()>>,released_tx:Option<mpsc::SyncSender<()>>,invalidate_after:usize,recovery_start_failures:usize,append_each:Option<mpsc::Receiver<()>>,discontinuous:bool}
impl Ops {fn new()->Self {let base=Instant::now();let mut owner=Owner::new(base+Duration::from_secs(8),NONCE.into(),TrackKind::Microphone).unwrap();owner.assigned(base).unwrap();Self{shared:Default::default(),fault:None,fault_used:false,base,ms:0,controls:0,stop_at:24,empty:false,invalidate:0,reopen_failures:0,changed:false,late:false,wall:false,cross:false,finalize:None,owner:Arc::new(Mutex::new(owner)),interrupt:None,append_entered:None,released_tx:None,invalidate_after:0,recovery_start_failures:0,append_each:None,discontinuous:false}}
fn call(&mut self,name:&str)->Result<()> {self.shared.lock().unwrap().calls.push(name.into());if self.fault==Some(name)&&!self.fault_used {self.fault_used=true;Err(format!("injected {name}"))}else{Ok(())}}
fn native(&mut self,name:&str)->std::result::Result<(),NativeFault> {self.call(name).map_err(NativeFault::fatal)} }
impl WorkerOps for Ops {
fn check_control(&mut self)->Result<Control>{self.call("control")?;
if let Some((point,control))=self.interrupt {
 let s=self.shared.lock().unwrap();
 let hits=match point {
 "before-open"=>s.opens==0,
 "after-open"=>s.native_live&&!s.calls.iter().any(|c|c=="start"),
 "post-start"=>s.calls.iter().any(|c|c=="start")&&!s.calls.iter().any(|c|c=="ready"),
 "running"=>s.calls.iter().any(|c|c=="ready"),
 _=>false,
 };drop(s);
 if hits {self.call(point)?;if control==Control::Stop{self.owner.lock().unwrap().stop(self.now()).unwrap();}return Ok(control);}
}
if self.shared.lock().unwrap().calls.iter().any(|c|c=="ready"){self.controls+=1;}if self.controls>=self.stop_at {self.owner.lock().unwrap().stop(self.now()).unwrap();Ok(Control::Stop)}else{Ok(Control::Live)}}
fn open_exact(&mut self,request:&TrackRequest)->std::result::Result<Format,NativeFault>{self.native("open")?;assert_eq!(request.endpoint_id,"exact-id");let mut s=self.shared.lock().unwrap();s.opens+=1;if s.opens>1&&self.reopen_failures>0 {self.reopen_failures-=1;return Err(NativeFault::fatal("reopen unavailable"));}s.native_live=true;if s.opens>1&&self.changed{return Ok(Format{rate:44100,..format()});}if s.opens>1&&self.late{self.ms+=5000;}Ok(format())}
fn start(&mut self)->std::result::Result<(),NativeFault>{self.native("start")?;if self.shared.lock().unwrap().opens>1&&self.recovery_start_failures>0{self.recovery_start_failures-=1;return Err(NativeFault::classify(0x88890026u32 as i32,"restart invalidated"));}Ok(())}
fn wait(&mut self,ms:u32)->std::result::Result<Wake,NativeFault>{assert!(ms<=100);self.native("wait")?;let mut s=self.shared.lock().unwrap();let n=s.acquisitions;s.wait_packets.push(n);s.acquisitions=0;if self.wall{self.ms+=86_400_000;}else{self.ms+=u64::from(ms);}Ok(Wake::Audio)}
fn packet(&mut self,t:&mut Track)->std::result::Result<PacketPoll,NativeFault>{self.native("packet")?;let mut s=self.shared.lock().unwrap();assert!(s.native_live);s.packets+=1;if self.invalidate>0&&s.packets>=self.invalidate_after{self.invalidate-=1;return Err(NativeFault::classify(0x88890004u32 as i32,"invalidated"));}if self.empty{return Ok(PacketPoll::Empty);}s.acquisitions+=1;let d=s.packets as u64;if self.cross{t.accepted_frames=frame_cap(format()).unwrap()-1;}let result=t.admit(if self.cross{2}else{1},SILENT|if self.discontinuous{1}else{0},d,d*1000,None);s.releases+=1;drop(s);if let Some(rx)=self.append_entered.take(){rx.recv_timeout(Duration::from_secs(5)).unwrap();}if let Some(rx)=&self.append_each{rx.recv_timeout(Duration::from_secs(5)).unwrap();}self.native("release")?;result.map_err(NativeFault::fatal)?;Ok(PacketPoll::Accepted)}
fn stop_and_drop(&mut self)->Result<()>{let had=self.shared.lock().unwrap().native_live;if !had{return Ok(());}let stop=self.call("stop");let drop_result=self.call("drop");self.shared.lock().unwrap().native_live=false;stop.and(drop_result)}
fn stamp(&mut self)->Result<ClockStamp>{Ok(ClockStamp{qpc:self.ms,frequency:1000})}
fn now(&self)->Instant{self.base+Duration::from_millis(self.ms)}
fn pause(&mut self,ms:u32){self.ms+=u64::from(ms);thread::yield_now();}
fn emit(&mut self,e:&EventV1)->Result<()>{let name=match e.event{Event::Ready{..}=>"ready",Event::CaptureReleased{..}=>"released",Event::Terminal{..}=>"terminal",_=>"event"};self.call(name)?;if matches!(e.event,Event::CaptureReleased{..}){assert!(!self.shared.lock().unwrap().native_live);}self.owner.lock().unwrap().message(self.now(),ClockStamp{qpc:self.ms,frequency:1000},e).map_err(|e|e.to_string())?;self.shared.lock().unwrap().events.push(e.clone());if matches!(e.event,Event::CaptureReleased{..}){if let Some(tx)=self.released_tx.take(){tx.send(()).unwrap();}}Ok(())}
fn finalize_ops(&mut self)->Box<dyn FinalizeOps+Send>{self.finalize.take().unwrap_or_else(||Box::new(DefaultFinalize))}
}
struct DefaultFinalize;impl FinalizeOps for DefaultFinalize {}
fn run(ops:&mut Ops,dir:&Dir)->WorkerExit{worker::run_worker(ops,init(&dir.0))}
#[test]fn fault_table_uses_one_shared_sequence_and_owned_cleanup(){for fault in ["control","open","start","ready","wait","packet","release","stop","drop","released","terminal"]{let dir=Dir::new();let mut ops=Ops::new();ops.fault=Some(fault);let out=run(&mut ops,&dir);let s=ops.shared.lock().unwrap();assert!(!s.calls.is_empty(),"fault table must enter shared control gate: {fault}");assert!(!s.native_live,"{fault}");assert!(s.calls.iter().filter(|x|*x=="stop").count()<=1,"{fault}");assert!(s.calls.iter().filter(|x|*x=="drop").count()<=1,"{fault}");assert!(s.events.iter().filter(|x|matches!(x.event,Event::Ready{..})).count()<=1);if let Some(released)=s.calls.iter().position(|x|x=="released"){assert!(!s.calls[released+1..].iter().any(|x|matches!(x.as_str(),"open"|"start"|"wait"|"packet"|"stop"|"drop")));}if fault=="control"{assert_eq!(s.opens,0);}if matches!(fault,"ready"|"released"|"terminal"){assert_ne!(out.code,0);assert!(out.terminal.is_none());}}}
#[test]fn creation_failure_never_starts_native(){let dir=Dir::new();fs::write(dir.0.join("source.partial.wav"),b"old").unwrap();let mut ops=Ops::new();let out=run(&mut ops,&dir);assert!(!ops.shared.lock().unwrap().calls.iter().any(|x|x=="start"));assert!(out.terminal.unwrap().usable.is_none());assert_eq!(fs::read(dir.0.join("source.partial.wav")).unwrap(),b"old");}
#[test]fn stop_first_drain_is_bounded_and_releases_once(){let dir=Dir::new();let mut ops=Ops::new();ops.stop_at=40;let out=run(&mut ops,&dir);let s=ops.shared.lock().unwrap();assert_eq!(s.packets,s.releases);assert!(s.wait_packets.iter().all(|n|*n<=8));assert!(s.calls.iter().any(|x|x=="wait"));assert!(out.terminal.as_ref().unwrap().accepted_frames<=s.releases as u64);assert!(s.releases as u64-out.terminal.as_ref().unwrap().accepted_frames<=1);}
#[test]fn stop_ack_precedes_drop_then_release_without_post_release_capture(){let dir=Dir::new();let mut ops=Ops::new();ops.empty=true;let out=run(&mut ops,&dir);assert_eq!(out.code,0);let s=ops.shared.lock().unwrap();let ack=s.events.iter().position(|x|matches!(x.event,Event::StopAck{..})).unwrap();let released=s.events.iter().position(|x|matches!(x.event,Event::CaptureReleased{..})).unwrap();assert!(ack<released);let stop=s.calls.iter().position(|x|x=="stop").unwrap();let drop=s.calls.iter().position(|x|x=="drop").unwrap();let rel=s.calls.iter().position(|x|x=="released").unwrap();assert!(stop<drop&&drop<rel);}
#[test]fn no_packets_is_not_silence(){let dir=Dir::new();let mut ops=Ops::new();ops.empty=true;let out=run(&mut ops,&dir).terminal.unwrap();assert_eq!(out.accepted_frames,0);assert!(out.usable.is_none());assert!(dir.0.join("evidence.partial.json").exists());}
#[test]fn budget_rejects_whole_crossing_packet(){let dir=Dir::new();let mut ops=Ops::new();ops.cross=true;let out=run(&mut ops,&dir).terminal.unwrap();assert!(out.usable.is_none());assert_eq!(out.written_complete_frames,0);assert!(out.cause.contains("budget"));}
#[test]fn wall_budget_is_from_successful_start(){let dir=Dir::new();let mut ops=Ops::new();ops.wall=true;let out=run(&mut ops,&dir).terminal.unwrap();assert_eq!(ops.shared.lock().unwrap().packets,0);assert!(out.cause.contains("24h"));}
#[test]fn recovery_original_id_two_attempts_five_seconds(){for mode in 0..4 {let dir=Dir::new();let mut ops=Ops::new();ops.invalidate=1;ops.stop_at=90;ops.reopen_failures=if mode==0{1}else if mode==1{2}else{0};ops.changed=mode==2;ops.late=mode==3;let out=run(&mut ops,&dir);let s=ops.shared.lock().unwrap();assert!(s.opens<=3);let recovered=s.events.iter().filter(|x|matches!(x.event,Event::Recovered{..})).count();assert_eq!(recovered,usize::from(mode==0));assert_eq!(out.terminal.is_some(),mode!=3);}}
#[test]fn only_exact_safe_hresult_is_recoverable(){for code in [0x88890004u32,0x88890026,0x88890005,0x80004005]{let f=NativeFault::classify(code as i32,"raw");assert_eq!(f.recoverable(),matches!(code,0x88890004|0x88890026));assert!(f.message().contains(&format!("{code:08X}")));}}
#[test]fn strict_early_dispatch_does_not_fall_through(){use std::ffi::OsString;assert_eq!(worker::dispatch([OsString::from("ordinary")].into_iter()),None);assert_eq!(worker::dispatch([OsString::from("--private-owned-audio-worker")].into_iter()),Some(64));}
#[cfg(unix)]#[test]fn unbuffered_gate_retains_trailing_byte_from_same_pipe_delivery(){use std::{os::unix::net::UnixStream,io::{Read,Write}};let dir=Dir::new();let (mut a,mut b)=UnixStream::pair().unwrap();let wire=protocol::encode_init(&InitV1{version:1,nonce:NONCE.into(),request:init(&dir.0).request().clone()}).unwrap();a.write_all(&[wire.as_slice(),b"x"].concat()).unwrap();let _=protocol::read_init(&mut b,NONCE).unwrap();b.set_nonblocking(true).unwrap();let mut tail=[0];assert_eq!(b.read(&mut tail).unwrap(),1);assert_eq!(tail,[b'x']);assert!(worker::pipe_control(false,1,false).is_err());assert!(worker::pipe_control(true,1,false).is_err());assert_eq!(worker::pipe_control(true,0,false).unwrap(),Control::Stop);assert_eq!(worker::pipe_control(true,0,true).unwrap(),Control::ParentEof);assert_eq!(worker::pipe_control(false,0,true).unwrap(),Control::ParentEof);}
struct BlockFinalize{gate:Arc<(Mutex<bool>,Condvar)>,entered:mpsc::SyncSender<()>}
impl FinalizeOps for BlockFinalize{fn sync(&mut self,f:&File)->Result<()>{self.entered.send(()).unwrap();let(lock,cv)=&*self.gate;let mut open=lock.lock().unwrap();while !*open{open=cv.wait(open).unwrap();}f.sync_all().map_err(|e|e.to_string())}}
#[test]
fn storage_hang_forces_after_120s_simulated_policy_then_releases_and_joins() {
    let dir=Dir::new();let mut ops=Ops::new();ops.stop_at=18;
    let owner=ops.owner.clone();let shared=ops.shared.clone();
    let gate=Arc::new((Mutex::new(false),Condvar::new()));let(tx,rx)=mpsc::sync_channel(1);
    ops.finalize=Some(Box::new(BlockFinalize{gate:gate.clone(),entered:tx}));
    let input=init(&dir.0);let join=thread::spawn(move||worker::run_worker(&mut ops,input));
    let entered=rx.recv_timeout(Duration::from_secs(5));
    let (native_live,released,terminal)={let s=shared.lock().unwrap();(s.native_live,
        s.events.iter().any(|x|matches!(x.event,Event::CaptureReleased{..})),
        s.events.iter().any(|x|matches!(x.event,Event::Terminal{..})))};
    let effect=owner.lock().unwrap().tick(Instant::now()+Duration::from_secs(121),false);
    // Release and join before assertions, including when the coordination timeout failed.
    *gate.0.lock().unwrap()=true;gate.1.notify_all();let out=join.join().unwrap();
    entered.unwrap();assert!(!native_live);assert!(released);assert!(!terminal);
    assert_eq!(effect.unwrap(),Tick::Force);assert_ne!(out.code,0);assert!(out.terminal.is_none());
}

#[test]
fn control_boundaries_block_open_start_and_ready_and_stop_without_speculative_drain() {
    for (point,control) in [("before-open",Control::Stop),("after-open",Control::Stop),
        ("post-start",Control::Stop),("running",Control::Stop),("running",Control::ParentEof)] {
        let dir=Dir::new();let mut ops=Ops::new();ops.interrupt=Some((point,control));
        let out=run(&mut ops,&dir);let s=ops.shared.lock().unwrap();
        assert_eq!(s.packets,0,"{point}");
        assert_eq!(s.events.iter().filter(|e|matches!(e.event,Event::Ready{..})).count(),usize::from(point=="running"));
        if point=="before-open"{assert_eq!(s.opens,0);}
        if point=="after-open"{assert!(!s.calls.iter().any(|c|c=="start"));}
        if control==Control::ParentEof{assert!(out.terminal.is_none());assert!(s.events.iter().any(|e|matches!(e.event,Event::Ending{..})));}
        else{assert!(s.events.iter().any(|e|matches!(e.event,Event::StopAck{..})));}
    }
}
struct BlockAppend {gate:Arc<(Mutex<bool>,Condvar)>,entered:Option<mpsc::SyncSender<()>>}
impl FinalizeOps for BlockAppend {
    fn before_append(&mut self) {
        if let Some(tx)=self.entered.take(){
            tx.send(()).unwrap();let(lock,cv)=&*self.gate;let mut open=lock.lock().unwrap();
            while !*open{open=cv.wait(open).unwrap();}
        }
    }
}
#[test]
fn queue_full_preserves_exact_admitted_prefix_and_joins_actual_writer() {
    let dir=Dir::new();let mut ops=Ops::new();ops.stop_at=1000;
    let gate=Arc::new((Mutex::new(false),Condvar::new()));
    let(entered_tx,entered_rx)=mpsc::sync_channel(1);let(released_tx,released_rx)=mpsc::sync_channel(1);
    ops.finalize=Some(Box::new(BlockAppend{gate:gate.clone(),entered:Some(entered_tx)}));
    ops.append_entered=Some(entered_rx);ops.released_tx=Some(released_tx);
    let shared=ops.shared.clone();let input=init(&dir.0);
    let join=thread::spawn(move||worker::run_worker(&mut ops,input));
    let released=released_rx.recv_timeout(Duration::from_secs(5));
    // Always unblock and join the real operation even if a coordination assertion fails.
    *gate.0.lock().unwrap()=true;gate.1.notify_all();let out=join.join().unwrap();released.unwrap();
    let terminal=out.terminal.unwrap();let s=shared.lock().unwrap();
    assert_eq!(s.packets,10);assert_eq!(s.releases,10); // one held + eight queued + one rejected
    assert_eq!(terminal.accepted_frames,9);assert_eq!(terminal.written_complete_frames,9);
    assert_eq!(terminal.usable.unwrap().frames,9);assert!(terminal.cause.contains("queue unavailable"));
    let evidence:serde_json::Value=serde_json::from_slice(&fs::read(dir.0.join("evidence.partial.json")).unwrap()).unwrap();
    assert_eq!(evidence["raw_tail"]["wav_first"],8);assert_eq!(evidence["exclusive_end"],"Unmeasured");
}
#[test]
fn successful_recovery_changes_generation_before_next_admission_and_retains_raw_prefix() {
    let dir=Dir::new();let mut ops=Ops::new();ops.invalidate=1;ops.invalidate_after=2;ops.stop_at=28;
    let out=run(&mut ops,&dir);assert_eq!(out.code,0);
    let s=ops.shared.lock().unwrap();assert!(s.events.iter().any(|e|matches!(e.event,Event::Recovered{episode:1,generation:1})));
    let evidence:serde_json::Value=serde_json::from_slice(&fs::read(dir.0.join("evidence.partial.json")).unwrap()).unwrap();
    assert_eq!(evidence["domains"][0]["first"]["generation"],0);
    assert_eq!(evidence["domains"][1]["first"]["generation"],1);
    assert_eq!(evidence["domains"][1]["first"]["wav_first"],1);
    assert_eq!(evidence["first_sample"],"Unmeasured");
}
#[test]
fn restart_invalidation_consumes_one_of_two_attempts_without_extending_deadline() {
    let dir=Dir::new();let mut ops=Ops::new();ops.invalidate=1;ops.recovery_start_failures=1;ops.stop_at=40;
    let out=run(&mut ops,&dir);assert_eq!(out.code,0);let s=ops.shared.lock().unwrap();
    assert_eq!(s.opens,3);assert_eq!(s.events.iter().filter(|e|matches!(e.event,Event::Recovered{..})).count(),1);
}
#[test]
fn disabled_wrong_nonce_unknown_and_oversized_gates_never_enter_worker() {
    let dir=Dir::new();let request=init(&dir.0).request().clone();
    for mode in 0..4 {
        let mut value=serde_json::to_value(InitV1{version:1,nonce:NONCE.into(),request:request.clone()}).unwrap();
        match mode{0=>value["request"]["explicitly_enabled"]=false.into(),1=>value["nonce"]="00000000-0000-0000-0000-000000000000".into(),2=>value["extra"]=true.into(),_=>value["request"]["endpoint_id"]="x".repeat(4096).into()}
        let mut bytes=serde_json::to_vec(&value).unwrap();bytes.push(b'\n');
        assert!(protocol::decode_init_frame(&bytes,NONCE).is_err());
    }
}
#[test]
fn empty_recovery_generations_consume_domain_budget_without_double_counting_next_packet() {
    let(tx,rx)=mpsc::sync_channel(8);let mut track=Track::new(format(),tx).unwrap();
    track.admit(1,SILENT,0,1,None).unwrap();rx.recv().unwrap();
    for _ in 1..1024{track.recover(format()).unwrap();}
    assert!(track.check_recovery().is_err());
    // The generation reserved one domain, so its first actual packet does not consume a second.
    track.admit(1,SILENT,0,1,None).unwrap();let packet=rx.recv().unwrap();
    assert_eq!(packet.generation,1023);assert_eq!(packet.wav_first,1);assert!(packet.segment_start);
    assert!(track.recover(format()).is_err());assert_eq!(track.accepted_frames,2);
}
#[test]
fn source_caps_reject_whole_crossing_packets_with_only_counters() {
    for format in [format(),Format{rate:192000,channels:2,bits:32,valid_bits:32,align:8,encoding:Encoding::Float}] {
        let(tx,rx)=mpsc::sync_channel(8);let mut track=Track::new(format,tx).unwrap();
        let cap=frame_cap(format).unwrap();track.accepted_frames=cap-1;
        assert!(track.admit(2,SILENT,0,1,None).is_err());assert_eq!(track.accepted_frames,cap-1);
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn empty_recovery_episodes_are_bounded_before_next_open_or_event() {
    let dir=Dir::new();let mut ops=Ops::new();ops.invalidate=1025;ops.stop_at=100000;
    let out=run(&mut ops,&dir);assert_eq!(out.code,0);
    let s=ops.shared.lock().unwrap();assert_eq!(s.opens,1024);
    assert_eq!(s.events.iter().filter(|e|matches!(e.event,Event::RecoveryStarted{..})).count(),1023);
    assert_eq!(s.events.iter().filter(|e|matches!(e.event,Event::Recovered{..})).count(),1023);
    let terminal=out.terminal.unwrap();assert_eq!(terminal.accepted_frames,0);
    assert!(terminal.cause.contains("budget"));assert!(terminal.usable.is_none());
    let evidence:serde_json::Value=serde_json::from_slice(&fs::read(dir.0.join("evidence.partial.json")).unwrap()).unwrap();
    assert_eq!(evidence["domains"].as_array().unwrap().len(),0);
}

#[test]
fn protocol_control_faults_at_each_startup_boundary_preserve_containment() {
    for point in ["before-open","after-open","post-start","running"] {
        let dir=Dir::new();let mut ops=Ops::new();ops.fault=Some(point);
        ops.interrupt=Some((point,Control::Live));let out=run(&mut ops,&dir);
        let s=ops.shared.lock().unwrap();assert!(!s.native_live);assert_eq!(s.packets,0);
        assert_eq!(out.code,1);assert!(out.terminal.is_none());
        assert!(!s.events.iter().any(|e|matches!(e.event,Event::CaptureReleased{..}|Event::Terminal{..})));
        assert_eq!(s.events.iter().filter(|e|matches!(e.event,Event::Ready{..})).count(),usize::from(point=="running"));
        if point=="before-open"{assert_eq!(s.opens,0);}
        if point=="after-open"{assert!(!s.calls.iter().any(|c|c=="start"));}
    }
}

struct ObserveAppend(mpsc::SyncSender<()>);
impl FinalizeOps for ObserveAppend {fn before_append(&mut self){self.0.send(()).unwrap();}}
#[test]
fn admitted_domain_budget_prevents_recovery_event_and_reopen_in_actual_runner() {
    let dir=Dir::new();let mut ops=Ops::new();let(tx,rx)=mpsc::sync_channel(1);
    ops.finalize=Some(Box::new(ObserveAppend(tx)));ops.append_each=Some(rx);
    ops.discontinuous=true;ops.invalidate=1;ops.invalidate_after=1025;ops.stop_at=100000;
    let out=run(&mut ops,&dir);assert_eq!(out.code,0);let s=ops.shared.lock().unwrap();
    assert_eq!(s.opens,1);assert!(!s.events.iter().any(|e|matches!(e.event,Event::RecoveryStarted{..}|Event::Recovered{..})));
    let terminal=out.terminal.unwrap();assert_eq!(terminal.accepted_frames,1024);
    assert_eq!(terminal.written_complete_frames,1024);assert_eq!(terminal.usable.unwrap().frames,1024);
    assert!(terminal.cause.contains("budget"));
    let evidence:serde_json::Value=serde_json::from_slice(&fs::read(dir.0.join("evidence.partial.json")).unwrap()).unwrap();
    assert_eq!(evidence["domains"].as_array().unwrap().len(),1024);
}
