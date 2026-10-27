//! App-owned Job and gated WGC worker. No frame data crosses the control pipe.
use super::*;
use super::super::{CapturePlan, StartReservation};
use owned_feed::{Geometry, OwnedCount, OwnedPixels, Source, Selector, complete_cells, write_frame};
use std::{ffi::{OsStr,OsString}, io::Write, os::windows::{io::AsRawHandle,process::CommandExt}, path::PathBuf,
 process::{Child,Command,Stdio}, sync::{Arc,Mutex,mpsc::{self,Receiver,SyncSender},atomic::{AtomicBool,Ordering}},thread,time::{Duration,Instant}};
use ::windows::{core::{PCWSTR,BOOL}, Win32::{Foundation::{CloseHandle,DuplicateHandle,HANDLE,HWND,WAIT_OBJECT_0},System::{JobObjects::*,Performance::*,Threading::*,WinRT::{RoInitialize,RoUninitialize,RO_INIT_MULTITHREADED}},UI::WindowsAndMessaging::GetWindowThreadProcessId}};
use windows_capture::{capture::{Context,GraphicsCaptureApiHandler},frame::Frame,graphics_capture_api::InternalCaptureControl,settings::*,window::Window};
#[cfg(test)]
pub(in crate::recorder) mod qualification;
type Result<T> = std::result::Result<T,String>;
const MODE:&str="--focusflow-owned-wgc";
const POLL:Duration=Duration::from_millis(5);
const STAGE:Duration=Duration::from_secs(2);
// Owned handles are process-wide kernel references, safe to move between threads; never inherited.
struct Handle(usize);
impl Handle {fn raw(&self)->HANDLE{HANDLE(self.0 as *mut _)}}
impl Drop for Handle {fn drop(&mut self){unsafe{let _=CloseHandle(self.raw());}}}
fn err(e:impl std::fmt::Display)->String{e.to_string()}
// QPC writes only valid stack outputs; timestamps retain their native integer domains.
fn qpc()->Result<i64>{let mut n=0;unsafe{QueryPerformanceCounter(&mut n).map_err(err)?;}if n<0{return Err("Negative QPC".into());}Ok(n)}
fn frequency()->Result<i64>{let mut n=0;unsafe{QueryPerformanceFrequency(&mut n).map_err(err)?;}if n<=0{return Err("Invalid QPC frequency".into());}Ok(n)}
fn control(byte:u8)->Result<()> {let mut b=[0];io::stdin().read_exact(&mut b).map_err(err)?;if b!=[byte]{return Err("Ownership/start gate cancelled or invalid".into());}Ok(())}
fn emit(message:Message)->Result<()> {let bytes=serde_json::to_vec(&message).map_err(err)?;if bytes.len()>4096{return Err("Worker message too large".into());}let mut out=io::stdout().lock();out.write_all(&bytes).and_then(|_|out.write_all(b"\n")).and_then(|_|out.flush()).map_err(err)}
// Borrowed process and owned Job handles remain open throughout each kernel query.
fn in_job(process:HANDLE,job:HANDLE)->Result<()> {let mut inside=BOOL(0);unsafe{IsProcessInJob(process,Some(job),&mut inside).map_err(err)?;}if !inside.as_bool(){return Err("Process did not inherit the app-owned Job".into());}Ok(())}
fn verify_job(child:&Child,name:&str)->Result<()> {let wide:Vec<u16>=name.encode_utf16().chain([0]).collect();let job=Handle(unsafe{OpenJobObjectW(0x0004,false,PCWSTR(wide.as_ptr())).map_err(err)?}.0 as usize);in_job(HANDLE(child.as_raw_handle()),job.raw())}
// SECURITY_ATTRIBUTES=None creates a non-inheritable handle; the UTF-16 name and limits outlive these calls.
fn create_job(name:&str)->Result<Handle>{let wide:Vec<u16>=name.encode_utf16().chain([0]).collect();unsafe{
 let job=Handle(CreateJobObjectW(None,PCWSTR(wide.as_ptr())).map_err(err)?.0 as usize);
 let mut info=JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();info.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
 SetInformationJobObject(job.raw(),JobObjectExtendedLimitInformation,&info as *const _ as _,std::mem::size_of_val(&info) as u32).map_err(err)?;Ok(job)
}}
// WinRT initialization and uninitialization stay balanced on this worker thread.
struct Apartment;
impl Apartment {fn initialize()->Result<Self>{unsafe{RoInitialize(RO_INIT_MULTITHREADED).map_err(err)?;}Ok(Self)}}
impl Drop for Apartment {fn drop(&mut self){unsafe{RoUninitialize();}}}
// HWND is only queried, never owned or destroyed; process identity is rechecked before admission.
fn verify_identity(window:u64,pid:u32)->Result<Window>{
 let hwnd=HWND(usize::try_from(window).map_err(err)? as *mut _);let mut actual=0;unsafe{GetWindowThreadProcessId(hwnd,Some(&mut actual));}
 let w=Window::from_raw_hwnd(hwnd.0);
 if actual==0||actual!=pid||!w.is_valid(){return Err("Selected window closed or changed process; select it again".into());}Ok(w)
}
// Keep only the Send window variant; the upstream enum also contains a thread-bound picker HWND.
struct PreparedWindow {item: ::windows_capture::GraphicsCaptureItem,window:Window}
impl From<PreparedWindow> for GraphicsCaptureItemType {fn from(value:PreparedWindow)->Self{Self::Window((value.item,value.window))}}
fn item_geometry(item:&PreparedWindow)->Result<Geometry>{
 let size=item.item.Size().map_err(err)?;Geometry::new(u32::try_from(size.Width).map_err(err)?,u32::try_from(size.Height).map_err(err)?).map_err(err)
}
fn verify_source(window:u64,pid:u32,item:&PreparedWindow,g:Geometry)->Result<()> {
 verify_identity(window,pid)?;let actual=item_geometry(item)?;
 if (actual.width,actual.height)!=(g.width,g.height){return Err("Selected window changed size before admission; select it again".into());}Ok(())
}

pub(in super::super) struct Session {child:Child,job:Handle,encoder:Option<Handle>,messages:Receiver<Result<Message>>,manifest:Option<WindowsCaptureManifestV2>,finished:bool}
impl Session {
 pub(in super::super) fn prepare(dir:&Path,plan:&CapturePlan,defaults:LegacyDefaults,ffmpeg:&Path,exe:&Path)->RecorderResult<Self>{
  let w=plan.window_capture.as_ref().ok_or_else(||recovery("Window plan missing"))?;let b=plan.bounds;
  let name=format!("Local\\FocusFlowOwned-{}-{}",std::process::id(),qpc().map_err(recovery)?);let job=create_job(&name).map_err(recovery)?;
  let child=Command::new(exe).arg(MODE).arg(w.hwnd.to_string()).arg(w.process_id.to_string()).arg(dir.join("screen.mp4")).arg(ffmpeg).arg(&name)
   .creation_flags(CREATE_NO_WINDOW.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(recovery)?;
  let (tx,messages)=mpsc::sync_channel(8);let mut session=Self{child,job,encoder:None,messages,manifest:None,finished:false};
  let stdout=session.child.stdout.take().ok_or_else(||recovery("Worker stdout missing"))?;
  thread::Builder::new().name("wgc-control-reader".into()).spawn(move||{let mut reader=stdout;for _ in 0..8 {let message=read_message(&mut reader);let terminal=matches!(message,Ok(Message::Terminal{..}|Message::Error{..})|Err(_));if tx.send(message).is_err()||terminal{return;}}let _=tx.send(Err("Too many worker protocol messages".into()));}).map_err(recovery)?;
  // The child cannot open WGC or spawn its encoder before verified assignment and gate release.
  unsafe{AssignProcessToJobObject(session.job.raw(),HANDLE(session.child.as_raw_handle())).map_err(recovery)?;}
  in_job(HANDLE(session.child.as_raw_handle()),session.job.raw()).map_err(recovery)?;
  session.send(1)?;
  let Message::Prepared{width,height}=session.receive(Instant::now()+Duration::from_secs(8))? else{return Err(recovery("Worker preparation protocol failed"));};
  session.manifest=Some(incomplete(&super::super::session_id_from_dir(dir)?,w.hwnd as u64,width,height,Bounds{x:b.x,y:b.y,width:b.width,height:b.height},defaults)?);Ok(session)
 }
 fn send(&mut self,b:u8)->RecorderResult<()> {self.child.stdin.as_mut().ok_or_else(||recovery("Worker control pipe missing"))?.write_all(&[b]).map_err(recovery)}
 fn receive(&self,deadline:Instant)->RecorderResult<Message>{match self.messages.recv_timeout(deadline.saturating_duration_since(Instant::now())).map_err(recovery)?.map_err(recovery)?{Message::Error{message}=>Err(recovery(message)),m=>Ok(m)}}
 pub(in super::super) fn arm(&mut self,dir:&Path,reservation:&mut StartReservation<'_>)->RecorderResult<()> {
  reservation.publish_windows_incomplete(dir,self.manifest.as_ref().ok_or_else(||recovery("Prepared source missing"))?)?;
  self.send(2)?;let deadline=Instant::now()+Duration::from_secs(5);
  let Message::Encoder{pid}=self.receive(deadline)? else{return Err(recovery("Missing exact encoder ownership"));};
  // Worker retains Child and waits for gate 4, so the PID cannot be recycled before this handle opens.
  let handle=Handle(unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,false,pid).map_err(recovery)?}.0 as usize);
  in_job(handle.raw(),self.job.raw()).map_err(recovery)?;self.encoder=Some(handle);
  #[cfg(test)] qualification::arm_observation(self,dir).map_err(recovery)?;
  self.send(4)?;
  if !matches!(self.receive(deadline)?,Message::Ready{}){return Err(recovery("Missing WGC startup acknowledgement"));}Ok(())
 }
 pub(in super::super) fn pid(&self)->u32{self.child.id()}
 pub(in super::super) fn ended(&mut self)->RecorderResult<bool>{self.child.try_wait().map(|s|s.is_some()).map_err(recovery)}
 pub(in super::super) fn finish(mut self)->RecorderResult<Completion>{
  let deadline=Instant::now()+Duration::from_secs(12);let normal_deadline=deadline-STAGE;
  let result=(||{
   let sent=if self.child.try_wait().map_err(recovery)?.is_none(){self.send(3).is_ok()}else{false};
   let mut ack=None;let summary=loop{match self.receive(normal_deadline)?{Message::Ack{cutoff} if ack.is_none()=>ack=Some(cutoff),Message::Terminal{summary}=>break summary,_=>return Err(recovery("Invalid terminal protocol order"))}};
   if sent&&!summary.closed&&!summary.geometry_changed&&ack!=Some(summary.cutoff){return Err(recovery("Installed Stop acknowledgement missing or inconsistent"));}
   let status=wait_child(&mut self.child,normal_deadline).map_err(recovery)?;
   if !self.encoder.as_ref().is_some_and(|h|unsafe{WaitForSingleObject(h.raw(),0)}==WAIT_OBJECT_0){return Err(recovery("Encoder exact-handle completion not observed"));}
   Completion::new(self.manifest.clone().ok_or_else(||recovery("Prepared source missing"))?,summary,status)
  })();
  if result.is_err(){self.cleanup(deadline);}else{self.finished=true;self.child.stdin.take();}result
 }
 fn cleanup(&mut self,deadline:Instant){
  self.child.stdin.take();unsafe{let _=TerminateJobObject(self.job.raw(),5);}let _=self.child.kill();
  while Instant::now()<deadline {let worker=matches!(self.child.try_wait(),Ok(Some(_)));let encoder=self.encoder.as_ref().is_none_or(|h|unsafe{WaitForSingleObject(h.raw(),0)}==WAIT_OBJECT_0);if worker&&encoder{self.finished=true;return;}thread::sleep(POLL);}
  self.finished=true;
 }
}
impl Drop for Session {fn drop(&mut self){if !self.finished{self.cleanup(Instant::now()+STAGE);}}}
fn wait_child(child:&mut Child,deadline:Instant)->Result<i32>{loop{if let Some(status)=child.try_wait().map_err(err)?{return Ok(status.code().unwrap_or(-1));}if Instant::now()>=deadline{return Err("Owned process completion deadline exceeded".into());}thread::sleep(POLL);}}
struct Encoder(Child);
impl Drop for Encoder {fn drop(&mut self){if !matches!(self.0.try_wait(),Ok(Some(_))){let _=self.0.kill();let _=wait_child(&mut self.0,Instant::now()+STAGE);}}}
fn encoder_command(ffmpeg:&Path,g:Geometry,path:&Path)->Command{
 let mut c=Command::new(ffmpeg);c.creation_flags(CREATE_NO_WINDOW.0).args(["-hide_banner","-loglevel","error","-nostdin","-n","-f","rawvideo","-pixel_format","bgra","-video_size"])
  .arg(format!("{}x{}",g.encoded_width,g.encoded_height)).args(["-framerate","30","-i","pipe:0","-map","0:v:0","-an","-c:v","libx264","-preset","veryfast","-crf","23","-pix_fmt","yuv420p","-bf","0","-fps_mode","passthrough","-video_track_timescale","30000","-movflags","+faststart","-f","mp4"]).arg(path).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());c
}
fn drain(mut input:impl Read+Send+'static)->Result<Receiver<Result<String>>>{let(tx,rx)=mpsc::sync_channel(1);thread::Builder::new().name("wgc-encoder-diagnostic".into()).spawn(move||{let result=(||{let mut kept=Vec::new();let mut b=[0;4096];loop{let n=match input.read(&mut b){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>r.map_err(err)?};if n==0{break;}kept.extend_from_slice(&b[..n.min(1024-kept.len())]);}Ok(String::from_utf8_lossy(&kept).into_owned())})();let _=tx.send(result);}).map_err(err)?;Ok(rx)}
fn probe_profile(ffmpeg:&Path,job:&str)->Result<()> {
 let dir=std::env::temp_dir().join(format!("focusflow-profile-{}-{}",std::process::id(),qpc()?));std::fs::create_dir(&dir).map_err(err)?;
 let result=(||{let mut encoder=Encoder(encoder_command(ffmpeg,Geometry::new(2,2)?,&dir.join("probe.mp4")).spawn().map_err(err)?);verify_job(&encoder.0,job)?;
  let diagnostic=drain(encoder.0.stderr.take().ok_or("Profile stderr missing")?)?;
  encoder.0.stdin.take().ok_or("Profile stdin missing")?.write_all(&[0;16]).map_err(err)?;
  if wait_child(&mut encoder.0,Instant::now()+STAGE)?!=0{return Err(format!("Installed FFmpeg cannot encode the required H.264 profile: {}",diagnostic.recv_timeout(STAGE).map_err(err)??));}Ok(())})();
 let _=std::fs::remove_dir_all(dir);result
}
#[derive(Clone,Default)]
struct Shared {cutoff:Option<i64>,closed:bool,geometry_changed:bool,failure:Option<String>,#[cfg(test)] trace:qualification::Trace}
#[derive(Clone)]
struct Flags {tx:SyncSender<Source>,shared:Arc<Mutex<Shared>>,live:Arc<OwnedCount>,geometry:Geometry,#[cfg(test)] window:u64,#[cfg(test)] pid:u32}
struct Capture {flags:Flags,seq:u64,#[cfg(test)] rejected_seen:bool}
impl GraphicsCaptureApiHandler for Capture {
 type Flags=Flags;type Error=Box<dyn std::error::Error+Send+Sync>;
 fn new(ctx:Context<Flags>)->std::result::Result<Self,Self::Error>{Ok(Self{flags:ctx.flags,seq:0,#[cfg(test)] rejected_seen:false})}
 fn on_frame_arrived(&mut self,frame:&mut Frame,control:InternalCaptureControl)->std::result::Result<(),Self::Error>{
  #[cfg(test)] let callback_qpc=qpc().ok();
  let result=(||->Result<()>{
   let native=frame.timestamp().map_err(err)?.Duration;
   #[cfg(test)] let content=frame.content_size().map_err(err)?;
   if !self.flags.geometry.matches(frame.content_size().map_err(err)?,(frame.width(),frame.height())){
    let mut state=self.flags.shared.lock().map_err(err)?;
    #[cfg(test)] state.trace.push(qualification::Event{kind:"geometry",native,content_width:content.0,content_height:content.1,texture_width:frame.width(),texture_height:frame.height(),..Default::default()})?;
    if state.cutoff.is_none(){state.cutoff=Some(qpc()?);state.geometry_changed=true;}drop(state);return Ok(());
   }
   let mut buffer=frame.buffer().map_err(err)?;if buffer.color_format()!=ColorFormat::Bgra8{return Err("WGC frame is not BGRA".into());}
   let pitch=buffer.row_pitch() as usize;let owned=OwnedPixels::copy(buffer.as_raw_buffer(),pitch,self.flags.geometry,self.flags.live.clone())?;
   #[cfg(test)] let identity_before=qpc().ok();
   #[cfg(test)] let (paint,nonce,corners)=match qualification::identity(&owned,self.flags.geometry){
    Ok(value)=>value,Err(error)=>{
     if !self.rejected_seen{self.rejected_seen=true;
      let observed=qualification::rejected(&owned,self.flags.geometry,self.seq,native,callback_qpc,identity_before,content,(frame.width(),frame.height()),self.flags.window,self.flags.pid,&error);
      if let Ok(mut state)=self.flags.shared.lock(){if state.trace.rejected.is_none(){state.trace.rejected=Some(observed);}}
     }
     return Err(error);
    }
   };
   #[allow(unused_mut)] let mut state=self.flags.shared.lock().map_err(err)?;if state.cutoff.is_some(){return Ok(());}
   let published=qpc()?;self.flags.tx.try_send(Source{seq:self.seq,native,published,pixels:Arc::new(owned)}).map_err(|_|"Source snapshot queue overflow/disconnected")?;
   #[cfg(test)] state.trace.push(qualification::Event{kind:"source",seq:self.seq,native,qpc:published,paint,nonce,corners,content_width:content.0,content_height:content.1,texture_width:frame.width(),texture_height:frame.height(),..Default::default()})?;
   self.seq=self.seq.checked_add(1).ok_or("Snapshot sequence overflow")?;drop(state);Ok(())
  })();
  if let Err(e)=result{if let Ok(mut state)=self.flags.shared.lock(){state.failure=Some(e.clone());}control.stop();return Err(e.into());}if self.flags.shared.lock().map_err(|_|"Capture state poisoned")?.geometry_changed{control.stop();}Ok(())
 }
 fn on_closed(&mut self)->std::result::Result<(),Self::Error>{let mut state=self.flags.shared.lock().map_err(|_|"Capture state poisoned")?;if state.cutoff.is_none(){state.cutoff=Some(qpc()?);state.closed=true;}Ok(())}
}
#[derive(Clone,Default)]
struct Progress {began:Option<Instant>,complete:u32,#[cfg(test)] writes:qualification::Writes,#[cfg(test)] supervisor_terminated:Option<bool>}
fn write_outputs(mut pipe:impl Write,rx:Receiver<Source>,progress:Arc<Mutex<Progress>>)->Result<()> {
 while let Ok(source)=rx.recv(){progress.lock().map_err(err)?.began=Some(Instant::now());
 #[cfg(test)] qualification::write_started(source.seq)?;
 let mut written=0;let result=write_frame(&mut pipe,&source.pixels.bytes,&mut written);
 #[cfg(test)] progress.lock().map_err(err)?.writes.push(source.seq,written,result.is_ok())?;
 result.map_err(err)?;let mut p=progress.lock().map_err(err)?;p.complete+=1;p.began=None;}drop(pipe);Ok(())
}
struct Supervisor {stop:Arc<AtomicBool>,done:Receiver<Result<()>>}
impl Supervisor {
 fn start(child:&Child,progress:Arc<Mutex<Progress>>)->Result<Self>{
  // Duplicate the exact child handle; PID lookup cannot target a recycled or foreign process.
  let mut raw=HANDLE::default();unsafe{DuplicateHandle(GetCurrentProcess(),HANDLE(child.as_raw_handle()),GetCurrentProcess(),&mut raw,PROCESS_TERMINATE.0,false,Default::default()).map_err(err)?;}
  let handle=Handle(raw.0 as usize);let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();let(tx,done)=mpsc::sync_channel(1);
  thread::Builder::new().name("wgc-writer-supervisor".into()).spawn(move||{let result=loop{
   if stopped.load(Ordering::SeqCst){break Ok(());}
   let stalled=progress.lock().map(|p|p.began.is_some_and(|t|t.elapsed()>Duration::from_secs(1))).unwrap_or(true);
   if stalled{let result=unsafe{TerminateProcess(handle.raw(),6)};
    #[cfg(test)] if let Ok(mut p)=progress.lock(){p.supervisor_terminated=Some(result.is_ok());}
    break Err(format!("Encoder writer stalled; termination returned {result:?}"));}thread::sleep(POLL);
  };let _=tx.send(result);}).map_err(err)?;Ok(Self{stop,done})
 }
 fn finish(&self)->Result<()>{self.stop.store(true,Ordering::SeqCst);self.done.recv_timeout(STAGE).map_err(err)?}
}
impl Drop for Supervisor {fn drop(&mut self){self.stop.store(true,Ordering::SeqCst);}}
fn worker(window:u64,pid:u32,path:&Path,ffmpeg:&Path,job:&str)->Result<()> {
 control(1)?;
 #[cfg(test)] qualification::initialize(path)?;
 let _apartment=Apartment::initialize()?;
 let captured:GraphicsCaptureItemType=verify_identity(window,pid)?.try_into().map_err(err)?;
 let GraphicsCaptureItemType::Window((item,window_handle))=captured else{return Err("Expected a native window item".into());};
 let item=PreparedWindow{item,window:window_handle};let g=item_geometry(&item)?;probe_profile(ffmpeg,job)?;emit(Message::Prepared{width:g.width as u32,height:g.height as u32})?;control(2)?;
 verify_source(window,pid,&item,g)?;let freq=frequency()?;
 let mut encoder=Encoder(encoder_command(ffmpeg,g,path).spawn().map_err(err)?);verify_job(&encoder.0,job)?;
 let stderr=drain(encoder.0.stderr.take().ok_or("Encoder stderr missing")?)?;
 emit(Message::Encoder{pid:encoder.0.id()})?;control(4)?;
 #[cfg(test)] qualification::park(path)?;
 // Revalidate immediately before admission; preparation never authorizes a replacement HWND.
 verify_source(window,pid,&item,g)?;
 let progress=Arc::new(Mutex::new(Progress{#[cfg(test)] writes:qualification::Writes::allocated(),..Default::default()}));let supervisor=Supervisor::start(&encoder.0,progress.clone())?;
 let(writer_tx,writer_rx)=mpsc::sync_channel(2);let(done_tx,done_rx)=mpsc::sync_channel(1);let pipe=encoder.0.stdin.take().ok_or("Encoder stdin missing")?;let writer_progress=progress.clone();
 thread::Builder::new().name("wgc-raw-writer".into()).spawn(move||{let _=done_tx.send(write_outputs(pipe,writer_rx,writer_progress));}).map_err(err)?;
 let(control_tx,control_rx)=mpsc::sync_channel(1);thread::Builder::new().name("wgc-parent-control".into()).spawn(move||{let _=control_tx.send(control(3));}).map_err(err)?;
 let shared=Arc::new(Mutex::new(Shared{#[cfg(test)] trace:qualification::Trace::allocated(),..Default::default()}));let(tx,rx)=mpsc::sync_channel(8);let admission=qpc()?;
 let settings=Settings::new(item,CursorCaptureSettings::WithCursor,DrawBorderSettings::Default,SecondaryWindowSettings::Default,MinimumUpdateIntervalSettings::Default,DirtyRegionSettings::Default,ColorFormat::Bgra8,Flags{tx,shared:shared.clone(),live:Arc::new(OwnedCount::default()),geometry:g,#[cfg(test)] window,#[cfg(test)] pid});
 let capture=Capture::start_free_threaded(settings).map_err(err)?;emit(Message::Ready{})?;
 let mut selector=Selector::new(admission,freq)?;let mut selected=0;let started=Instant::now();let mut handled=false;let mut state_at_end=Shared::default();
 let feed=(||->Result<()>{loop{
  match done_rx.try_recv(){Ok(r)=>{r?;return Err("Writer ended before input was closed".into());},Err(mpsc::TryRecvError::Disconnected)=>return Err("Writer lost".into()),Err(mpsc::TryRecvError::Empty)=>{}}
  let mut ack=None;let mut output=None;let finished;
  {
   let mut state=shared.lock().map_err(err)?;if let Some(failure)=&state.failure{return Err(failure.clone());}
   if !handled {match control_rx.try_recv(){Ok(r)=>{r?;if state.cutoff.is_none(){state.cutoff=Some(qpc()?);}ack=state.cutoff;handled=true;},Err(mpsc::TryRecvError::Disconnected)=>return Err("Parent control lost".into()),Err(mpsc::TryRecvError::Empty)=>{}}}
   // Snapshot draining and decisions share the publication lock; all I/O follows unlock.
   while let Ok(source)=rx.try_recv(){selector.push(source)?;}
   let decision=qpc()?;
   if let Some(epoch)=selector.epoch {
    let cells=complete_cells(epoch,state.cutoff.unwrap_or(decision).min(decision),freq)?;
    if cells>owned_feed::MAX_FRAMES{return Err("Recording reached the 24-hour duration limit".into());}
    if selected>cells{return Err("Selected output exceeds installed cutoff".into());}
    if selected<cells{output=Some(selector.select(selected,decision)?);
     #[cfg(test)] state.trace.push(qualification::Event{kind:"decision",ordinal:selected,seq:output.as_ref().unwrap().seq,qpc:decision,..Default::default()})?;
     selected+=1;}
    finished=state.cutoff.is_some()&&selected==cells;
   }else{if state.cutoff.is_some()||started.elapsed()>STAGE{return Err("No admitted complete frame; restore the window and retry".into());}finished=false;}
   if capture.is_finished()&&state.cutoff.is_none(){return Err("WGC ended without installed cutoff".into());}
   state_at_end=state.clone();
  }
  if let Some(cutoff)=ack{emit(Message::Ack{cutoff})?;}
  if let Some(source)=output{writer_tx.try_send(source).map_err(|_|"Output queue overflow/disconnected")?;}
  if finished{break;}thread::sleep(Duration::from_millis(2));
 }Ok(())})();
 let mut failure=feed.err();let(capture_tx,capture_rx)=mpsc::sync_channel(1);
 thread::spawn(move||{let result=if capture.is_finished(){capture.wait()}else{capture.stop()};let _=capture_tx.send(result.map_err(err));});
 if failure.is_some(){let _=encoder.0.kill();}
 if let Err(e)=capture_rx.recv_timeout(STAGE).map_err(err).and_then(|r|r){failure.get_or_insert(e);let _=encoder.0.kill();}
 drop(writer_tx);
 if let Err(e)=done_rx.recv_timeout(STAGE).map_err(err).and_then(|r|r){failure.get_or_insert(e);let _=encoder.0.kill();}
 #[cfg(test)] let mut encoder_exit=None;
 let code=match wait_child(&mut encoder.0,Instant::now()+STAGE){Ok(code)=>{#[cfg(test)] {encoder_exit=Some(code);}code},Err(e)=>{failure.get_or_insert(e);let _=encoder.0.kill();-1}};
 if let Err(e)=supervisor.finish(){failure.get_or_insert(e);}
 #[cfg(test)] let mut non_encoder_failure=failure.is_some();
 if code!=0{failure.get_or_insert(format!("Encoder exited {code}"));}
 let diagnostic=stderr.recv_timeout(Duration::from_millis(100)).map_err(err).and_then(|r|r);
 #[cfg(test)] {non_encoder_failure|=diagnostic.is_err();}
 if let Err(e)=diagnostic{failure.get_or_insert(e);}
 match shared.try_lock(){Ok(state)=>{#[cfg(test)] {non_encoder_failure|=state.failure.is_some();}if let Some(e)=&state.failure{failure.get_or_insert(e.clone());}state_at_end=state.clone();},Err(_)=>{#[cfg(test)] {non_encoder_failure=true;}failure.get_or_insert("Capture final state unavailable".into());}}
 #[cfg(test)] qualification::flush(path,&shared,&progress,g,freq,admission,selector.epoch,selected,&failure,encoder_exit,non_encoder_failure)?;
 if let Some(error)=failure{return Err(error);}
 let written=progress.try_lock().map_err(err)?.complete;
 let summary=Summary{frequency:freq,admission,epoch:selector.epoch.ok_or("No epoch")?,cutoff:state_at_end.cutoff.ok_or("No cutoff")?,closed:state_at_end.closed,selected,written,encoder_exit:code,geometry_changed:state_at_end.geometry_changed};summary.clock(0)?;
 emit(Message::Terminal{summary})
}
pub(crate) fn dispatch(mut args:impl Iterator<Item=OsString>)->Option<i32>{
 if args.next().as_deref()!=Some(OsStr::new(MODE)){return None;}
 let result=(||->Result<()>{let values:Vec<_>=args.take(6).collect();if values.len()!=5{return Err("Invalid owned WGC worker arguments".into());}
  let number=|i:usize|->Result<u64>{values[i].to_str().ok_or("Invalid numeric worker argument")?.parse().map_err(err)};
  let window=number(0)?;let pid=u32::try_from(number(1)?).map_err(err)?;
  worker(window,pid,&PathBuf::from(&values[2]),&PathBuf::from(&values[3]),values[4].to_str().ok_or("Invalid Job name")?)
 })();Some(match result{Ok(())=>0,Err(mut message)=>{while message.len()>1024{message.pop();}let _=emit(Message::Error{message});1}})
}
