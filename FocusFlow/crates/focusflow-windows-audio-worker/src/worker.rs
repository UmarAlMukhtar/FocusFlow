use crate::{Result,format::Format,packet::Track,protocol::{ValidatedInit,TrackRequest,ClockStamp,EventV1,TerminalSummary},wav::FinalizeOps};
use std::{ffi::OsString,time::Instant};
#[derive(Debug,Clone,Copy,PartialEq,Eq)]pub enum Control{Live,Stop,ParentEof}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]pub enum Wake{Stop,Audio,Timeout}
#[derive(Debug,Clone,Copy,PartialEq,Eq)]pub enum PacketPoll{Accepted,Empty,Stopped}
#[derive(Debug,Clone)]pub enum NativeFault{DeviceInvalidated{hresult:i32,message:String},Fatal{message:String}}
impl NativeFault{pub fn fatal(s:impl Into<String>)->Self{Self::Fatal{message:s.into()}}pub fn classify(hresult:i32,s:impl Into<String>)->Self{let message=format!("{} (HRESULT 0x{:08X})",s.into(),hresult as u32);if matches!(hresult as u32,0x88890004|0x88890026){Self::DeviceInvalidated{hresult,message}}else{Self::Fatal{message}}}pub fn recoverable(&self)->bool{matches!(self,Self::DeviceInvalidated{hresult,..} if matches!(*hresult as u32,0x88890004|0x88890026))}pub fn message(&self)->&str{match self{Self::DeviceInvalidated{message,..}|Self::Fatal{message}=>message}}}
struct DefaultFinalize;impl FinalizeOps for DefaultFinalize{}
pub trait WorkerOps{
fn check_control(&mut self)->Result<Control>;
fn open_exact(&mut self,request:&TrackRequest)->std::result::Result<Format,NativeFault>;
fn start(&mut self)->std::result::Result<(),NativeFault>;
fn wait(&mut self,ms:u32)->std::result::Result<Wake,NativeFault>;
fn packet(&mut self,t:&mut Track)->std::result::Result<PacketPoll,NativeFault>;
/// Every returned result, including an error, guarantees actual capture/COM ownership drop.
/// A blocked native Stop/drop does not return; its parent must contain the exact child.
fn stop_and_drop(&mut self)->Result<()>;
fn stamp(&mut self)->Result<ClockStamp>;
fn now(&self)->Instant{Instant::now()}
fn pause(&mut self,ms:u32){std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));}
fn emit(&mut self,e:&EventV1)->Result<()>;
fn finalize_ops(&mut self)->Box<dyn FinalizeOps+Send>{Box::new(DefaultFinalize)}
}
pub struct WorkerExit{pub code:i32,pub terminal:Option<TerminalSummary>}
pub fn pipe_control(stop:bool,available:u32,eof:bool)->Result<Control>{if available>0{Err("Unexpected trailing control bytes".into())}else if eof{Ok(Control::ParentEof)}else if stop{Ok(Control::Stop)}else{Ok(Control::Live)}}

/// Arguments exclude argv[0], matching the application entrypoint. Malformed worker mode cannot launch the app.
pub fn dispatch(mut args:impl Iterator<Item=OsString>)->Option<i32>{
    if args.next().as_deref()!=Some(std::ffi::OsStr::new("--private-owned-audio-worker")){return None;}
    let values:Vec<_>=args.take(4).collect();
    if values.len()!=3 || values.iter().any(|s|s.to_str().is_none_or(|s|s.is_empty()||s.contains('\0'))){return Some(64);}
    #[cfg(windows)]
    {Some(crate::native::dispatch_worker(values[0].to_str().unwrap(),values[1].to_str().unwrap(),values[2].to_str().unwrap()))}
    #[cfg(not(windows))]
    {eprintln!("Owned Windows audio worker is unsupported on this platform");Some(69)}
}
use crate::{protocol::{Owner,Event,STARTUP_MS,RECOVERY_MS,STORAGE_MS},writer::{WriterHandle,WriterOutcome,spawn_writer_with_finalize}};
use std::time::Duration;
struct End{cause:String,stop:bool}
impl End{fn failure(s:impl Into<String>)->Self{Self{cause:s.into(),stop:false}}}
impl From<String> for End{fn from(s:String)->Self{Self::failure(s)}}
impl From<NativeFault> for End{fn from(e:NativeFault)->Self{Self::failure(e.message())}}
struct Work{
    init:ValidatedInit,owner:Owner,wire:bool,writer:Option<WriterHandle>,
    completed:Option<WriterOutcome>,track:Option<Track>,started:Option<Instant>,episode:u32,startup_deadline:Instant,
}
impl Work{
    fn emit(&mut self,ops:&mut impl WorkerOps,event:Event)->Result<()>{
        if !self.wire{return Err("Worker protocol is unavailable".into());}
        let frame=EventV1{version:1,nonce:self.init.nonce().into(),kind:self.init.request().kind,event};
        let result=(||{
            let stamp=ops.stamp()?;
            self.owner.message(ops.now(),stamp,&frame).map_err(|e|e.to_string())?;
            // Validate the same bounded frame even for injected OS writers.
            crate::protocol::encode_event(&frame).map_err(|e|e.to_string())?;
            ops.emit(&frame)
        })();
        if result.is_err(){self.wire=false;let _=self.owner.protocol_failure(ops.now());}
        result
    }
    fn control(&mut self,ops:&mut impl WorkerOps)->std::result::Result<(),End>{
        let control=match ops.check_control(){Ok(control)=>control,Err(error)=>{
            self.wire=false;let _=self.owner.protocol_failure(ops.now());return Err(End::failure(error));
        }};
        match control{
            Control::Live=>Ok(()),Control::Stop=>Err(End{cause:"Parent Stop observed; admitted raw tail retained unmeasured".into(),stop:true}),
            Control::ParentEof=>Err(End::failure("Parent control EOF; partial retained")),
        }
    }
    fn check_writer(&mut self)->std::result::Result<(),End>{
        if let Some(w)=&mut self.writer{
            if let Some(out)=w.poll_completion()?{
                let reason=format!("Writer ended during capture; prefix retained: {}",out.reason);
                self.completed=Some(out);return Err(End::failure(reason));
            }
        }
        Ok(())
    }
    fn lifetime(&self,ops:&impl WorkerOps)->std::result::Result<(),End>{
        if self.started.is_some_and(|at|ops.now().saturating_duration_since(at)>=Duration::from_secs(86_400)){
            return Err(End::failure("Audio 24h wall lifetime reached; prefix retained"));
        }
        Ok(())
    }
    fn prepare(&mut self,ops:&mut impl WorkerOps)->std::result::Result<(),End>{
        self.control(ops)?;
        let format=ops.open_exact(self.init.request())?;
        self.control(ops)?;
        format.validate()?;
        let dir=&self.init.request().output_dir;
        crate::wav::reject_links(dir)?;
        if dir.canonicalize().map_err(|e|e.to_string())?!=*dir || !dir.is_dir(){return Err(End::failure("Worker output directory must already be canonical"));}
        self.writer=Some(spawn_writer_with_finalize(&dir.join("source.partial.wav"),&dir.join("evidence.partial.json"),format,ops.finalize_ops())?);
        loop{
            self.control(ops)?;
            if ops.now()>=self.startup_deadline{return Err(End::failure("Writer creation exceeded startup deadline"));}
            if self.writer.as_mut().ok_or("Writer not owned".to_string())?.poll_created()?{break;}
            ops.pause(1);
        }
        self.track=Some(Track::new(format,self.writer.as_mut().ok_or("Writer not owned".to_string())?.take_sender()?)?);
        self.control(ops)?;
        self.check_writer()?;
        ops.start()?;
        self.started=Some(ops.now());
        self.control(ops)?;
        self.check_writer()?;
        self.emit(ops,Event::Ready{format})?;
        Ok(())
    }
    fn capture(&mut self,ops:&mut impl WorkerOps)->End{
        let result=(||->std::result::Result<(),End>{loop{
            self.control(ops)?;self.check_writer()?;self.lifetime(ops)?;
            let wake=match ops.wait(100){Ok(w)=>w,Err(f) if f.recoverable()=>{self.recover(ops,f)?;continue;},Err(f)=>return Err(f.into())};
            if wake==Wake::Stop{return Err(End{cause:"Parent Stop observed; admitted raw tail retained unmeasured".into(),stop:true});}
            self.control(ops)?;self.check_writer()?;self.lifetime(ops)?;
            for _ in 0..8{
                self.control(ops)?;self.check_writer()?;self.lifetime(ops)?;
                match ops.packet(self.track.as_mut().ok_or("Track not prepared".to_string())?){
                    Ok(PacketPoll::Accepted)=>{},Ok(PacketPoll::Empty)=>break,
                    Ok(PacketPoll::Stopped)=>return Err(End{cause:"Parent Stop observed; admitted raw tail retained unmeasured".into(),stop:true}),
                    Err(f) if f.recoverable()=>{self.recover(ops,f)?;break;},Err(f)=>return Err(f.into()),
                }
            }
        }})();
        match result{Err(end)=>end,Ok(())=>End::failure("Capture unexpectedly returned")}
    }
    fn recover(&mut self,ops:&mut impl WorkerOps,fault:NativeFault)->std::result::Result<(),End>{
        // Called only after a failed no-buffer operation; Release/queue/validation are always Fatal.
        self.track.as_ref().ok_or("Track not prepared".to_string())?.check_recovery()?;
        let first_failure=ops.stamp()?;
        let deadline=ops.now()+Duration::from_millis(RECOVERY_MS);
        self.episode=self.episode.checked_add(1).ok_or("Recovery episode overflow".to_string())?;
        self.control(ops)?;self.lifetime(ops)?;
        self.emit(ops,Event::RecoveryStarted{episode:self.episode,first_failure})?;
        self.control(ops)?;
        ops.stop_and_drop()?;
        self.recovery_boundary(ops,deadline)?;
        let mut last=fault.message().to_string();
        for attempt in 0..2{
            self.recovery_boundary(ops,deadline)?;
            match ops.open_exact(self.init.request()){
                Ok(format)=>{
                    self.recovery_boundary(ops,deadline)?;
                    if format!=self.track.as_ref().ok_or("Track not prepared".to_string())?.format{return Err(End::failure("Recovered exact endpoint changed mix format; prefix retained"));}
                    self.control(ops)?;
                    match ops.start(){
                        Ok(())=>{},
                        Err(error) if error.recoverable()=>{
                            last=error.message().into();
                            self.recovery_boundary(ops,deadline)?;
                            ops.stop_and_drop()?;
                            self.recovery_boundary(ops,deadline)?;
                            if attempt==0{ops.pause(100);self.recovery_boundary(ops,deadline)?;}
                            continue;
                        },
                        Err(error)=>return Err(error.into()),
                    }
                    self.recovery_boundary(ops,deadline)?;
                    self.track.as_mut().ok_or("Track not prepared".to_string())?.recover(format)?;
                    return self.emit(ops,Event::Recovered{episode:self.episode,generation:self.track.as_ref().ok_or("Track not prepared".to_string())?.generation}).map_err(End::from);
                }
                Err(error)=>{
                    last=error.message().into();
                    self.recovery_boundary(ops,deadline)?;
                    // Failed construction owns no native object; consume any adapter partial state once.
                    ops.stop_and_drop()?;
                    self.recovery_boundary(ops,deadline)?;
                    if attempt==0{ops.pause(100);self.recovery_boundary(ops,deadline)?;}
                }
            }
        }
        Err(End::failure(format!("Exact endpoint recovery exhausted two attempts: {last}")))
    }
    fn recovery_boundary(&mut self,ops:&mut impl WorkerOps,deadline:Instant)->std::result::Result<(),End>{
        self.control(ops)?;self.lifetime(ops)?;self.check_writer()?;
        if ops.now()>=deadline{return Err(End::failure("Exact endpoint recovery exceeded five-second deadline; prefix retained"));}
        Ok(())
    }
    fn finish(mut self,ops:&mut impl WorkerOps,mut end:End)->WorkerExit{
        let announce=(||->Result<()>{
            let observed=ops.stamp()?;
            if end.stop{self.owner.stop(ops.now()).map_err(|e|e.to_string())?;self.emit(ops,Event::StopAck{observed})}
            else{self.emit(ops,Event::Ending{cause:bounded(&end.cause),observed})}
        })();
        if announce.is_err(){self.wire=false;}
        // Announce before possibly blocking Stop/drop. Returning from this operation certifies release.
        if let Err(error)=ops.stop_and_drop(){end.cause=format!("{}; native stop/drop: {error}",end.cause);}
        let released=ops.stamp();
        let storage_deadline=ops.now()+Duration::from_millis(STORAGE_MS);
        match released{Ok(at)=>{let _=self.emit(ops,Event::CaptureReleased{at});},Err(_)=>self.wire=false}
        let accepted=self.track.as_ref().map_or(0,|t|t.accepted_frames);
        // Only after actual capture/COM release may the sole Track sender close and storage drain.
        drop(self.track.take());
        if let Some(w)=&mut self.writer{w.close_sender();}
        let mut cancelled=false;
        while self.writer.is_some()&&self.completed.is_none(){
            match self.writer.as_mut().ok_or("Writer not owned").and_then(|w| w.poll_completion().map_err(|_|"Writer completion failed")){
                Ok(Some(out))=>{self.completed=Some(out);break;},Ok(None)=>{},Err(_)=>{self.wire=false;break;}
            }
            match ops.check_control(){Ok(Control::Live|Control::Stop)=>{},_=>{cancelled=true;self.wire=false;}}
            if ops.now()>=storage_deadline{cancelled=true;self.wire=false;}
            // A timeout/EOF cannot interrupt storage syscalls. Retain ownership until parent force or real join.
            ops.pause(1);
        }
        if ops.now()>=storage_deadline{cancelled=true;self.wire=false;}
        match ops.check_control(){Ok(Control::Live|Control::Stop)=>{},_=>{cancelled=true;self.wire=false;}}
        let summary=if let Some(mut out)=self.completed.take(){
            if cancelled{out.cancel_storage();}
            let out=out.reconcile(accepted,Some(&end.cause));
            TerminalSummary{accepted_frames:accepted,written_complete_frames:out.written_complete_frames,synchronized_frames:out.synchronized_frames,usable:out.usable,retained_partial:out.retained_partial,cause:bounded(&out.reason),evidence_present:out.evidence_present}
        }else{TerminalSummary{accepted_frames:accepted,written_complete_frames:0,synchronized_frames:None,usable:None,retained_partial:false,cause:bounded(&end.cause),evidence_present:false}};
        if self.emit(ops,Event::Terminal{outcome:summary.clone()}).is_ok(){WorkerExit{code:0,terminal:Some(summary)}}else{WorkerExit{code:1,terminal:None}}
    }
}
fn bounded(s:&str)->String{let mut end=s.len().min(1024);while !s.is_char_boundary(end){end-=1;}s[..end].replace('\0',"?")}
/// One production operation order; tests only control OS results, clocks and real storage faults.
pub fn run_worker(ops:&mut impl WorkerOps,init:ValidatedInit)->WorkerExit{
    let now=ops.now();let deadline=now+Duration::from_millis(STARTUP_MS);
    let mut owner=match Owner::new(deadline,init.nonce().into(),init.request().kind){Ok(o)=>o,Err(_)=>return WorkerExit{code:1,terminal:None}};
    if owner.assigned(now).is_err(){return WorkerExit{code:1,terminal:None};}
    let mut work=Work{init,owner,wire:true,writer:None,completed:None,track:None,started:None,episode:0,startup_deadline:deadline};
    let end=match work.prepare(ops){Ok(())=>work.capture(ops),Err(end)=>end};
    work.finish(ops,end)
}
