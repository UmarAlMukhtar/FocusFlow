//! Exact child ownership and bounded protocol polling; never infer exit from a timeout or PID.
use crate::{format::Format, pending::{ObservedExit, PendingRegistry, RetainedChild},
    protocol::{self, ClockStamp, Event, InitV1, Owner, State, TerminalSummary, Tick, TrackKind, TrackRequest, WavInventory}, Result};
use std::{path::Path, sync::{Arc, Mutex, atomic::{AtomicU64,Ordering}}, time::Instant};

pub enum PipeRead { Data(usize), Empty, Eof }
/// Only exact-resource OS operations vary. Owner, decoder, deadlines, and session ordering do not.
pub trait ChildOps: Send {
    fn assign_job(&mut self) -> Result<()>;
    fn send_init(&mut self, frame: &[u8]) -> Result<()>;
    fn read_stdout(&mut self, bytes: &mut [u8]) -> Result<PipeRead>;
    fn clock(&mut self) -> Result<(Instant, ClockStamp)>;
    fn observe_exit(&mut self) -> Result<Option<ObservedExit>>;
    fn signal_stop(&mut self) -> Result<()>;
    fn close_control(&mut self);
    fn force(&mut self) -> Result<()>;
}
pub(crate) struct Bundle { pub(crate) id: u64, pub(crate) child: Box<dyn ChildOps+Send> }
#[derive(Clone)]
pub struct SharedRegistry(pub(crate) Arc<Mutex<PendingRegistry<Bundle>>>);
impl SharedRegistry {
    pub fn new() -> Self { Self(Arc::new(Mutex::new(PendingRegistry::new()))) }
    pub fn len(&self) -> usize { self.0.lock().unwrap_or_else(|e|e.into_inner()).len() }
    pub fn replacement_allowed(&self) -> bool { self.0.lock().unwrap_or_else(|e|e.into_inner()).replacement_allowed() }
    /// One exact-handle observation per retained bundle; never terminates again or waits.
    pub fn reap(&self) -> usize {
        self.reap_with(|_, _| {})
    }
    pub(crate) fn reap_with(&self, mut observed: impl FnMut(u64, ObservedExit)) -> usize {
        let mut registry=self.0.lock().unwrap_or_else(|e|e.into_inner());
        registry.poll(|p| {
            let exit=p.child.observe_exit().ok().flatten()?;
            observed(p.id, exit);
            Some(exit)
        });
        registry.len()
    }
}
impl Default for SharedRegistry { fn default()->Self {Self::new()} }
pub(crate) fn native_registry() -> SharedRegistry {
    static REGISTRY: std::sync::OnceLock<SharedRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(SharedRegistry::new).clone()
}
pub fn reap_pending() -> usize { native_registry().reap() }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitObservation { NotStarted, Exited, CleanupPending }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackOutcome {
    pub kind: TrackKind,
    pub terminal: Option<TerminalSummary>,
    pub exit: ExitObservation,
    pub exit_code: Option<i32>,
    /// Producer-validated inventory only; later import must fully validate the fixed-path asset.
    pub usable: Option<WavInventory>,
    pub failure: Option<String>,
}
#[derive(Debug, Clone)]
pub struct TrackPoll { pub phase: State, pub notice: Option<String>, pub outcome: Option<TrackOutcome> }
pub struct OwnedTrack {
    ownership: RetainedChild<Bundle>,
    registry: SharedRegistry,
    id: u64,
    request: TrackRequest,
    nonce: String,
    owner: Owner,
    bytes: Vec<u8>,
    eof: bool,
    unavailable: bool,
    observed_exit: Option<ObservedExit>,
    signaled: bool,
    failure: Option<String>,
    notice: Option<String>,
    terminal: Option<TerminalSummary>,
    ready_format: Option<Format>,
    outcome: Option<TrackOutcome>,
}
impl OwnedTrack {
    /// Starts a gated child and returns without waiting for Ready.
    pub fn begin(exe:&Path, request:TrackRequest, deadline:Instant)->Result<Self> {
        #[cfg(windows)]
        { Self::begin_native(exe,request,deadline,native_registry()) }
        #[cfg(not(windows))]
        { let _=(exe,request,deadline); Err("Owned audio workers require Windows".into()) }
    }
    #[cfg(windows)]
    pub(crate) fn begin_private_render(permit:&crate::proof::guard::ProofPermit,request:TrackRequest,deadline:Instant)->Result<Self> {
        Self::begin_native_mode(permit.executable(),request,deadline,native_registry(),"--private-audio-render")
    }
    /// OS-result seam; live begin and injected tests enter the same gate/poll/cleanup sequence.
    pub fn begin_with(request:TrackRequest, deadline:Instant, nonce:String, registry:SharedRegistry,
        spawn:impl FnOnce()->Result<Box<dyn ChildOps+Send>>)->Result<Self> {
        let init=protocol::encode_init(&InitV1{version:1,nonce:nonce.clone(),request:request.clone()}).map_err(|e|e.to_string())?;
        let owner=Owner::new(deadline,nonce.clone(),request.kind).map_err(|e|e.to_string())?;
        if !registry.replacement_allowed() {return Err("Previous audio child cleanup is still unconfirmed".into());}
        static NEXT:AtomicU64=AtomicU64::new(1);
        let id=NEXT.fetch_add(1,Ordering::Relaxed);
        let child=spawn()?;
        let mut track=Self{ownership:RetainedChild::new(Bundle{id,child}),registry,id,request,nonce,owner,
            bytes:Vec::with_capacity(protocol::FRAME_BYTES),eof:false,unavailable:false,observed_exit:None,signaled:false,failure:None,notice:None,
            terminal:None,ready_format:None,outcome:None};
        let gate=(|| {
            track.ownership.owned.as_mut().unwrap().child.assign_job()?;
            let (now,_)=track.ownership.owned.as_mut().unwrap().child.clock()?;
            track.owner.assigned(now).map_err(|e|e.to_string())?;
            track.ownership.owned.as_mut().unwrap().child.send_init(&init)
        })();
        if let Err(error)=gate {
            let now=track.ownership.owned.as_mut().unwrap().child.clock().map(|c|c.0).unwrap_or_else(|_|Instant::now());
            track.fail(now,format!("Audio startup gate failed: {error}"));
        }
        Ok(track)
    }
    pub fn kind(&self)->TrackKind {self.request.kind}
    pub fn phase(&self)->State {self.owner.state}
    pub fn failure(&self)->Option<&str> {self.failure.as_deref()}
    /// Raw Terminal/first notice remain diagnostics; only the reducer can authorize completion.
    pub(crate) fn completed_protocol(&self)->bool {
        !self.unavailable && self.owner.terminal().is_some()
            && self.outcome.as_ref().is_some_and(|o|o.exit==ExitObservation::Exited && o.exit_code==Some(0))
    }
    fn note(&mut self, reason:String) {
        if self.failure.is_none() {
            let reason:String=reason.chars().take(1024).collect();
            self.notice=Some(reason.clone());self.failure=Some(reason);
        }
    }
    /// Direct event first. Even signal failure fixes the original containment deadline.
    pub fn request_stop(&mut self, now:Instant)->Result<()> {
        if self.signaled || self.observed_exit.is_some() || self.ownership.owned.is_none() {return Ok(());}
        self.signaled=true;
        let result=self.ownership.owned.as_mut().unwrap().child.signal_stop();
        self.owner.stop(now).map_err(|e|e.to_string())?;
        if let Err(error)=&result {self.note(format!("Audio stop signal failed: {error}"));}
        result
    }
    fn effect(&mut self, tick:Tick) {
        match tick {
            Tick::None=>(),
            Tick::Force=>{
                self.unavailable=true;
                self.note("Audio worker deadline exceeded; forced cleanup requested, partial files retained".into());
                if self.observed_exit.is_some() {return;}
                if let Some(bundle)=self.ownership.owned.as_mut() {
                    bundle.child.close_control();
                    if let Err(e)=bundle.child.force() {self.note(format!("Audio force request failed: {e}"));}
                }
            }
            Tick::RetainPending=>self.retain(),
        }
    }
    fn retain(&mut self) {
        self.ownership.retain_pending(&mut self.registry.0.lock().unwrap_or_else(|e|e.into_inner()));
        self.owner.state=State::CleanupPending;
        self.outcome=Some(self.result(ExitObservation::CleanupPending,None));
    }
    fn fail(&mut self, now:Instant, reason:String) {
        self.unavailable=true;
        self.note(reason);
        if self.observed_exit.is_none() {
            let _=self.request_stop(now);
            if let Ok(tick)=self.owner.protocol_failure(now) {self.effect(tick);}
        } else {let _=self.owner.protocol_failure(now);}
    }
    pub fn cancel_storage(&mut self, now:Instant)->Result<()> {
        self.unavailable=true;
        self.note("Audio storage cancelled; partial files retained".into());
        let _=self.request_stop(now);
        let tick=self.owner.cancel_storage(now).map_err(|e|e.to_string())?;
        self.effect(tick);Ok(())
    }
    fn result(&self,exit:ExitObservation,exit_code:Option<i32>)->TrackOutcome {
        TrackOutcome{kind:self.kind(),terminal:self.terminal.clone(),exit,exit_code,usable:None,failure:self.failure.clone()}
    }
    fn exited(&mut self, now:Instant, exit:ObservedExit) {
        let _=self.owner.tick(now,true);
        let mut outcome=self.result(ExitObservation::Exited,exit.code);
        if exit.code==Some(0) {
            if let Some(summary)=self.owner.terminal() {
                if let Some(inventory)=&summary.usable {
                    match self.validate_inventory(inventory) {
                        Ok(())=>outcome.usable=Some(inventory.clone()),
                        Err(e)=>{self.note(e);outcome.failure=self.failure.clone();}
                    }
                }
            } else {self.note("Audio child exited without a complete valid Terminal".into());outcome.failure=self.failure.clone();}
        } else {self.note("Audio child did not exit cleanly; partial files retained".into());outcome.failure=self.failure.clone();}
        self.ownership.owned.take();self.outcome=Some(outcome);
    }
    fn validate_inventory(&self, expected:&WavInventory)->Result<()> {
        // The owned writer performed sync/full scan before Terminal. Import must revalidate assets.
        let format=self.ready_format.ok_or("Missing audio Ready format")?;
        if format.rate!=expected.rate || format.channels!=expected.channels
            || format.bits!=expected.bits || format.encoding!=expected.encoding {
            return Err("Audio Terminal does not match Ready format; partial retained".into());
        }
        Ok(())
    }
    /// At most 16 KiB, four complete events and one exact-child observation. No wait/sleep loop.
    pub fn poll(&mut self, now:Instant)->TrackPoll {
        if self.ownership.owned.is_none() && self.owner.state==State::CleanupPending {
            let id=self.id;
            let exit=self.ownership.poll_pending(&mut self.registry.0.lock().unwrap_or_else(|e|e.into_inner()),
                |p| if p.id==id {p.child.observe_exit().ok().flatten()} else {None});
            if let Some(exit)=exit {self.exited(now,exit);}
        } else if self.ownership.owned.is_some() {
            self.read_events(now);
            if self.observed_exit.is_none() {
                match self.ownership.owned.as_mut().unwrap().child.observe_exit() {
                    Ok(exit)=>self.observed_exit=exit,
                    Err(e)=>self.fail(now,format!("Audio exact-child observation failed: {e}")),
                }
            }
            if let Some(exit)=self.observed_exit {
                // Preserve unread final frames across bounded polls after an already observed exit.
                if self.eof || self.unavailable {self.exited(now,exit);}
                else if self.owner.tick(now,false)==Ok(Tick::Force) {
                    self.note("Audio deadline expired while draining final protocol".into());
                    self.exited(now,exit);
                }
            } else if let Ok(tick)=self.owner.tick(now,false) {self.effect(tick);}

        }
        TrackPoll{phase:self.owner.state,notice:self.notice.take(),outcome:self.outcome.clone()}
    }
    fn read_events(&mut self, now:Instant) {
        if self.eof || matches!(self.owner.state,State::ForceExit|State::CleanupPending|State::Exited) {return;}
        let mut read=0;let mut events=0;
        loop {
            if events>=4 {break;}
            if let Some(end)=self.bytes.iter().position(|b|*b==b'\n') {
                let frame:Vec<u8>=self.bytes.drain(..=end).collect();events+=1;
                let stamp=self.ownership.owned.as_mut().unwrap().child.clock();
                let result:Result<()>=(|| {
                    let (after,stamp)=stamp?;
                    let message=protocol::decode_event_frame(&frame,&self.nonce,self.kind()).map_err(|e|e.to_string())?;
                    self.owner.message(after,stamp,&message).map_err(|e|e.to_string())?;
                    match message.event {
                        Event::Ready{format}=>self.ready_format=Some(format),
                        Event::Ending{cause,..}=>self.note(cause),
                        Event::Terminal{outcome}=>self.terminal=Some(outcome),
                        _=>(),
                    }
                    Ok(())
                })();
                if let Err(error)=result {self.fail(now,format!("Audio protocol failed: {error}"));break;}
                continue;
            }
            if self.bytes.len()>=protocol::FRAME_BYTES {self.fail(now,"Audio protocol frame exceeds budget".into());break;}
            if read>=16384 {break;}
            let mut buf=[0u8;protocol::FRAME_BYTES];
            let limit=(protocol::FRAME_BYTES-self.bytes.len()).min(16384-read);
            match self.ownership.owned.as_mut().unwrap().child.read_stdout(&mut buf[..limit]) {
                Ok(PipeRead::Data(n)) if n>0 && n<=limit=>{read+=n;self.bytes.extend_from_slice(&buf[..n]);},
                Ok(PipeRead::Empty)=>{
                    if self.observed_exit.is_some() {
                        // An exited child cannot produce future frames; an empty retained pipe is drained.
                        self.eof=true;
                        if !self.bytes.is_empty() || self.owner.state!=State::AwaitingExit {
                            self.fail(now,"Audio child exited without a complete Terminal".into());
                        }
                    }
                    break;
                },
                Ok(PipeRead::Eof)=>{
                    self.eof=true;
                    if !self.bytes.is_empty() || self.owner.state!=State::AwaitingExit {
                        self.fail(now,"Audio stdout ended without a complete Terminal".into());
                    }
                    break;
                }
                Ok(PipeRead::Data(_))=>{self.fail(now,"Invalid audio pipe read count".into());break;},
                Err(e)=>{self.eof=true;self.fail(now,format!("Audio pipe failed: {e}"));break;}
            }
        }
    }
}
impl Drop for OwnedTrack {
    fn drop(&mut self) {
        if self.observed_exit.is_some() {self.ownership.owned.take();return;}
        if self.ownership.owned.is_some() {
            let now=Instant::now();let _=self.request_stop(now);
            if let Ok(tick)=self.owner.cancel_storage(now) {self.effect(tick);}
            // No blocking Drop: the registry takes the original bundle, even before forced observation expires.
            self.retain();
        }
    }
}

#[cfg(windows)]
pub(crate) mod windows_child {
    use super::*;
    use crate::native::{Event as StopEvent,Handle};
    use std::{io::{Read,Write},os::windows::{io::AsRawHandle,process::CommandExt},process::{Child,Command,Stdio}};
    use windows::{core::{BOOL,PCWSTR,HRESULT},Win32::{Foundation::*,System::{JobObjects::*,Performance::{QueryPerformanceCounter,QueryPerformanceFrequency},Threading::*}}};
    fn err(e:impl std::fmt::Display)->String {e.to_string()}
    pub(crate) struct WindowsChild { child:Child,job:Handle,stop:StopEvent, transport:bool, _lifetime:Option<Box<dyn Send>> }
    impl OwnedTrack {
        pub(super) fn begin_native(exe:&Path,request:TrackRequest,deadline:Instant,registry:SharedRegistry)->Result<Self> {
            Self::begin_native_mode(exe,request,deadline,registry,"--private-owned-audio-worker")
        }
        pub(super) fn begin_native_mode(exe:&Path,mut request:TrackRequest,deadline:Instant,registry:SharedRegistry,mode:&str)->Result<Self> {
            request.validate().map_err(err)?;
            crate::wav::reject_links(&request.output_dir)?;
            let parent=request.output_dir.parent().ok_or("Audio output requires an owned session parent")?;
            let canonical=parent.canonicalize().map_err(err)?;
            let name=request.output_dir.file_name().ok_or("Audio child output name missing")?;
            request.output_dir=canonical.join(name);
            // SAFETY: CoCreateGuid creates a collision-resistant OS GUID and requires no COM apartment.
            let nonce=format!("{:?}",unsafe{windows::Win32::System::Com::CoCreateGuid().map_err(err)?});
            let child_request=request.clone();let child_nonce=nonce.clone();
            Self::begin_with(request,deadline,nonce,registry,|| {
                // create_dir is deliberately not recursive: existing outputs are never reused/deleted.
                std::fs::create_dir(&child_request.output_dir).map_err(|e|format!("Create fresh audio output directory: {e}"))?;
                crate::wav::reject_links(&child_request.output_dir)?;
                if child_request.output_dir.canonicalize().map_err(err)?!=child_request.output_dir {
                    return Err("Audio child output directory must be canonical".into());
                }
                WindowsChild::spawn(exe,&child_nonce,mode)
            })
        }
    }
    impl WindowsChild {
        fn spawn(exe:&Path,nonce:&str,mode:&str)->Result<Box<dyn ChildOps+Send>> {
            Self::spawn_with(nonce,false,None,|job_name,stop_name| {
                let mut command=Command::new(exe);
                command.arg(mode).arg(job_name).arg(stop_name).arg(nonce);
                command
            })
        }
        pub(crate) fn spawn_transport(_permit:&crate::proof::guard::approval::TransportPermit,script:&Path,lifetime:Box<dyn Send>)->Result<Box<dyn ChildOps+Send>> {
            // SAFETY: fresh name only; no apartment, endpoint or audio operation.
            let nonce=format!("{:?}",unsafe{windows::Win32::System::Com::CoCreateGuid().map_err(err)?});
            Self::spawn_with(&nonce,true,Some(lifetime),|_,_| {
                let mut command=Command::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
                command.env_clear().env("SystemRoot",r"C:\Windows").env("WINDIR",r"C:\Windows")
                    .arg("-NoProfile").arg("-NonInteractive").arg("-File").arg(script);
                command
            })
        }
        pub(crate) fn spawn_startup(
            _permit: &crate::proof::guard::approval::TransportPermit,
            launch: crate::proof::guard::helper::startup::StartupLaunch,
        ) -> Result<Box<dyn ChildOps + Send>> {
            let (script, lifetime) = launch.into_parts();
            // SAFETY: fresh Job namespace only; no apartment or audio operation.
            let nonce = format!("{:?}", unsafe { windows::Win32::System::Com::CoCreateGuid().map_err(err)? });
            Self::spawn_with(&nonce, true, Some(lifetime), |_, _| {
                let mut command = Command::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
                command.env_clear().env("SystemRoot", r"C:\Windows").env("WINDIR", r"C:\Windows")
                    .arg("-NoProfile").arg("-NonInteractive");
                match script.as_deref() {
                    Some(path) => { command.arg("-File").arg(path); }
                    None => { command.arg("-Command").arg(crate::proof::guard::helper::startup::COMMAND); }
                }
                command
            })
        }
        pub(crate) fn spawn_discovery(
            record: crate::proof::guard::helper::AdmissionRecord,
            nonce: &str,
        ) -> Result<Box<dyn ChildOps + Send>> {
            let (exe, workspace) = record.paths();
            Self::spawn_with(nonce, true, Some(Box::new(record)), |job, stop| {
                let mut command = Command::new(exe);
                command
                    .env_clear()
                    .env("SystemRoot", r"C:\Windows")
                    .env("WINDIR", r"C:\Windows")
                    .env("GITHUB_WORKSPACE", workspace)
                    .arg("--private-audio-discover")
                    .arg(job)
                    .arg(stop)
                    .arg(nonce);
                command
            })
        }
        fn spawn_with(nonce:&str,transport:bool,lifetime:Option<Box<dyn Send>>,command:impl FnOnce(&str,&str)->Command)->Result<Box<dyn ChildOps+Send>> {
            let job_name=format!("Local\\FocusFlowAudio-{nonce}");
            let stop_name=format!("{job_name}-stop");
            let wide:Vec<u16>=job_name.encode_utf16().chain([0]).collect();
            // SAFETY: newly owned non-inheritable named Job; kill-on-close is configured before spawn.
            let job=unsafe {
                SetLastError(ERROR_SUCCESS);
                let raw=CreateJobObjectW(None,PCWSTR(wide.as_ptr())).map_err(err)?;
                let exists=GetLastError()==ERROR_ALREADY_EXISTS;
                let job=Handle::take(raw);
                if exists {return Err("Owned audio Job name collision".into());}
                let mut limits=JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(job.raw(),JobObjectExtendedLimitInformation,&limits as *const _ as _,std::mem::size_of_val(&limits) as u32).map_err(err)?;
                job
            };
            let stop=StopEvent::create(true,Some(&stop_name))?;
            let child=command(&job_name,&stop_name)
                .creation_flags(CREATE_NO_WINDOW.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .spawn().map_err(err)?;
            Ok(Box::new(Self{child,job,stop,transport,_lifetime:lifetime}))
        }
    }
    impl ChildOps for WindowsChild {
        fn assign_job(&mut self)->Result<()> {
            // SAFETY: this independent Job and std Child's original exact process handle stay owned.
            unsafe {
                let child=HANDLE(self.child.as_raw_handle());
                AssignProcessToJobObject(self.job.raw(),child).map_err(err)?;
                let mut inside=BOOL(0);
                IsProcessInJob(child,Some(self.job.raw()),&mut inside).map_err(err)?;
                if !inside.as_bool(){return Err("Parent could not verify audio Job assignment".into());}
            }
            Ok(())
        }
        fn send_init(&mut self,frame:&[u8])->Result<()> {
            let stdin=self.child.stdin.as_mut().ok_or("Missing audio Init pipe")?;
            if self.transport {
                if frame.is_empty()||frame.len()>4096{return Err("Transport Init cap exceeded".into());}
                // SAFETY: exact owned pipe; transport alone uses nonblocking writes after Job assignment.
                unsafe { windows::Win32::System::Pipes::SetNamedPipeHandleState(HANDLE(stdin.as_raw_handle()),Some(&windows::Win32::System::Pipes::PIPE_NOWAIT),None,None).map_err(err)?; }
                if stdin.write(frame).map_err(err)?!=frame.len(){return Err("Incomplete nonblocking transport gate".into());}
                Ok(())
            } else {stdin.write_all(frame).map_err(err)}
        }
        fn read_stdout(&mut self,bytes:&mut[u8])->Result<PipeRead> {
            let stdout=self.child.stdout.as_mut().ok_or("Missing audio stdout pipe")?;
            let mut available=0;
            // SAFETY: this exact stdout handle has one reader; only the observed available bytes are read.
            let peek=unsafe{windows::Win32::System::Pipes::PeekNamedPipe(HANDLE(stdout.as_raw_handle()),None,0,None,Some(&mut available),None)};
            if let Err(e)=peek {
                if e.code()==HRESULT::from_win32(ERROR_BROKEN_PIPE.0) {return Ok(PipeRead::Eof);}
                return Err(err(e));
            }
            if available==0 {return Ok(PipeRead::Empty);}
            let count=bytes.len().min(available as usize);
            match stdout.read(&mut bytes[..count]).map_err(err)? {0=>Ok(PipeRead::Eof),n=>Ok(PipeRead::Data(n))}
        }
        fn clock(&mut self)->Result<(Instant,ClockStamp)> {
            let mut qpc=0;let mut frequency=0;
            // SAFETY: writable counters; this read occurs after complete protocol frame acquisition.
            unsafe {QueryPerformanceCounter(&mut qpc).map_err(err)?;QueryPerformanceFrequency(&mut frequency).map_err(err)?;}
            if qpc<0 || frequency<=0 {return Err("Invalid parent QPC clock".into());}
            Ok((Instant::now(),ClockStamp{qpc:qpc as u64,frequency:frequency as u64}))
        }
        fn observe_exit(&mut self)->Result<Option<ObservedExit>> {
            self.child.try_wait().map(|s|s.map(|s|ObservedExit{code:s.code()})).map_err(err)
        }
        fn signal_stop(&mut self)->Result<()> {self.stop.signal()}
        fn close_control(&mut self) {self.child.stdin.take();}
        fn force(&mut self)->Result<()> {
            // SAFETY: only this track is in this owned Job; video and sibling handles are never supplied.
            let job=unsafe{TerminateJobObject(self.job.raw(),71)};
            let fallback=self.child.kill();
            match (job,fallback) {(Err(a),Err(b))=>Err(format!("Job termination: {a}; exact-child fallback: {b}")),_=>Ok(())}
        }
    }
}
