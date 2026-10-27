//! Test-only scalar observation. No release switch, frame retention, or additional publication lock.
use super::*;
use serde::Serialize;
mod pixels;
static DIRECTORY:std::sync::OnceLock<PathBuf>=std::sync::OnceLock::new();
static FIRST_WRITE:AtomicBool=AtomicBool::new(false);
pub(super) fn initialize(path:&Path)->Result<()> {DIRECTORY.set(path.parent().ok_or("Case directory missing")?.to_owned()).map_err(|_|"Duplicate observation initialization".into())}
pub(super) fn write_started(seq:u64)->Result<()> {
 if !FIRST_WRITE.swap(true,Ordering::SeqCst){std::fs::write(DIRECTORY.get().ok_or("Observation not initialized")?.join("first-write-started.json"),format!("{{\"qpc\":{},\"seq\":{seq}}}",qpc()?)).map_err(err)?;}Ok(())
}

#[derive(Default,Serialize)]
pub(in crate::recorder) struct Event {
 pub kind:&'static str,pub order:usize,pub seq:u64,pub native:i64,pub qpc:i64,pub ordinal:u32,
 pub paint:u16,pub nonce:u16,pub corners:[u8;4],
 #[serde(rename="contentWidth")]pub content_width:i32,#[serde(rename="contentHeight")]pub content_height:i32,
 #[serde(rename="textureWidth")]pub texture_width:u32,#[serde(rename="textureHeight")]pub texture_height:u32,
}
#[derive(Default)]
pub(super) struct Trace {events:Vec<Event>,overflow:bool,pub rejected:Option<Rejected>}
// Shared's production state snapshots do not copy/allocate the observation collector.
impl Clone for Trace {fn clone(&self)->Self{Self::default()}}
impl Trace {
 pub fn allocated()->Self {assert!(std::mem::size_of::<Rejected>()<1024);assert!(std::mem::size_of::<Event>()*6144+std::mem::size_of::<WriteEvent>()*2048<2*1024*1024);Self{events:Vec::with_capacity(6144),overflow:false,rejected:None}}
 pub fn push(&mut self,mut event:Event)->Result<()> {
  if self.events.len()>=6144||self.events.capacity()<6144 {self.overflow=true;return Err("Qualification source/decision trace overflow".into());}
  event.order=self.events.len();self.events.push(event);Ok(())
 }
}
#[derive(Serialize)]
pub(super) struct WriteEvent {ordinal:u32,seq:u64,bytes:usize,ok:bool}
#[derive(Default)]
pub(super) struct Writes {events:Vec<WriteEvent>,overflow:bool}
impl Clone for Writes {fn clone(&self)->Self{Self::default()}}
impl Writes {
 pub fn allocated()->Self {Self{events:Vec::with_capacity(2048),overflow:false}}
 pub fn push(&mut self,seq:u64,bytes:usize,ok:bool)->Result<()> {
  if self.events.len()>=2048 {self.overflow=true;return Err("Qualification write trace overflow".into());}
  self.events.push(WriteEvent{ordinal:self.events.len() as u32,seq,bytes,ok});Ok(())
 }
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub(super) struct Rejected {
 identity_error:&'static str,next_source_seq:u64,native:i64,callback_qpc:Option<i64>,identity_before_qpc:Option<i64>,identity_error_qpc:Option<i64>,observation_end_qpc:Option<i64>,
 content_size:(i32,i32),texture_size:(u32,u32),raw_size:(usize,usize),encoded_size:(usize,usize),
 selected_hwnd:u64,selected_pid:u32,observed_pid:u32,observed_thread:u32,is_window:bool,
 pixels:Option<pixels::Pixels>,pixel_read_error:Option<&'static str>,admitted:bool,
}
pub(super) fn rejected(p:&OwnedPixels,g:Geometry,seq:u64,native:i64,callback:Option<i64>,before:Option<i64>,content:(i32,i32),texture:(u32,u32),window:u64,pid:u32,error:&str)->Rejected {
 let identity_error_qpc=qpc().ok();
 let identity_error=match error{
  "Obscured generated marker"=>"Obscured generated marker","Ambiguous generated marker"=>"Ambiguous generated marker",
  "Generated marker geometry too small"=>"Generated marker geometry too small","Torn generated marker replicas"=>"Torn generated marker replicas",
  "Obscured or wrong generated content corners"=>"Obscured or wrong generated content corners",_=>"Unclassified identity error; see trace failure"
 };
 let samples=pixels::observe(&p.bytes,g.width,g.height,g.encoded_width);let mut observed_pid=0;
 // Observe only our selected HWND; failure to query is retained as zero PID/thread, never eligibility.
 let hwnd=HWND(window as usize as *mut _);let observed_thread=unsafe{GetWindowThreadProcessId(hwnd,Some(&mut observed_pid))};
 let is_window=unsafe{::windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(hwnd))}.as_bool();
 let (pixels,pixel_read_error)=match samples{Ok(p)=>(Some(p),None),Err(e)=>(None,Some(e))};
 Rejected{identity_error,next_source_seq:seq,native,callback_qpc:callback,identity_before_qpc:before,identity_error_qpc,observation_end_qpc:qpc().ok(),content_size:content,texture_size:texture,raw_size:(g.width,g.height),encoded_size:(g.encoded_width,g.encoded_height),selected_hwnd:window,selected_pid:pid,observed_pid,observed_thread,is_window,pixels,pixel_read_error,admitted:false}
}
pub(super) fn identity(p:&OwnedPixels,g:Geometry)->Result<(u16,u16,[u8;4])> {
 let sample=|x:usize,y:usize|->Result<u8>{let b=&p.bytes[(y*g.encoded_width+x)*4..][..3];if b[0]!=b[1]||b[1]!=b[2]{return Err("Obscured generated marker".into());}Ok(b[0])};
 let bits=|y:usize|->Result<u16>{let mut v=0;for b in 0..16 {let c=sample(20+b*8,y+4)?;if c>180{v|=1<<b}else if c>=70{return Err("Ambiguous generated marker".into());}}Ok(v)};
 if g.width<160||g.height<120{return Err("Generated marker geometry too small".into());}
 let paint=bits(40)?;let nonce=bits(56)?;if paint!=bits(72)?||nonce!=bits(88)?{return Err("Torn generated marker replicas".into());}
 let corners=[sample(8,8)?,sample(g.width-8,8)?,sample(8,g.height-8)?,sample(g.width-8,g.height-8)?];
 if corners!=[40,100,160,220]{return Err("Obscured or wrong generated content corners".into());}Ok((paint,nonce,corners))
}
pub(super) fn flush(path:&Path,shared:&Arc<Mutex<Shared>>,progress:&Arc<Mutex<Progress>>,g:Geometry,freq:i64,admission:i64,epoch:Option<i64>,selected:u32,failure:&Option<String>,encoder_exit:Option<i32>,non_encoder_failure:bool)->Result<()> {
 // Move scalar collectors out while locked; all allocation/serialization/file I/O follows unlock.
 let (trace,cutoff)={let mut s=shared.lock().map_err(err)?;(std::mem::take(&mut s.trace),s.cutoff)};
 let (writes,written,supervisor_terminated)={let mut p=progress.lock().map_err(err)?;(std::mem::take(&mut p.writes),p.complete,p.supervisor_terminated)};
 let overflow=trace.overflow||writes.overflow;
 #[derive(Serialize)] struct Receipt<'a>{frequency:i64,admission:i64,epoch:Option<i64>,cutoff:Option<i64>,selected:u32,written:u32,width:usize,height:usize,overflow:bool,events:&'a [Event],#[serde(rename="firstRejectedFrame")]first_rejected_frame:&'a Option<Rejected>,writes:&'a [WriteEvent],failure:&'a Option<String>,#[serde(rename="supervisorTerminated")]supervisor_terminated:Option<bool>,#[serde(rename="encoderExitCode")]encoder_exit:Option<i32>,#[serde(rename="nonEncoderFailure")]non_encoder_failure:bool}
 let value=Receipt{frequency:freq,admission,epoch,cutoff,selected,written,width:g.width,height:g.height,overflow,events:&trace.events,first_rejected_frame:&trace.rejected,writes:&writes.events,failure,supervisor_terminated,encoder_exit,non_encoder_failure};
 let bytes=serde_json::to_vec(&value).map_err(err)?;if bytes.len()>4*1024*1024{return Err("Qualification serialized trace exceeds 4 MiB".into());}
 std::fs::write(path.with_file_name("trace.json"),bytes).map_err(err)?;
 if overflow{return Err("Qualification trace overflow".into());}Ok(())
}
pub(in crate::recorder) struct Handles {worker:Handle,encoder:Handle}
impl Handles {
 pub(in crate::recorder) fn observe(session:&Session)->Result<Self> {
  fn duplicate(h:HANDLE,access:u32)->Result<Handle>{let mut raw=HANDLE::default();unsafe{DuplicateHandle(GetCurrentProcess(),h,GetCurrentProcess(),&mut raw,access,false,Default::default()).map_err(err)?;}Ok(Handle(raw.0 as usize))}
  let worker=duplicate(HANDLE(session.child.as_raw_handle()),PROCESS_SYNCHRONIZE.0|PROCESS_TERMINATE.0)?;let encoder=duplicate(session.encoder.as_ref().ok_or("No owned encoder")?.raw(),PROCESS_SYNCHRONIZE.0)?;
  if unsafe{WaitForSingleObject(worker.raw(),0)}==WAIT_OBJECT_0||unsafe{WaitForSingleObject(encoder.raw(),0)}==WAIT_OBJECT_0{return Err("Owned child was not alive at observation".into());}Ok(Self{worker,encoder})
 }
 pub(in crate::recorder) fn kill_worker(&self)->Result<()> {unsafe{TerminateProcess(self.worker.raw(),7).map_err(err)}}
 pub(in crate::recorder) fn verify(&self,path:&Path)->Result<()> {
  let worker=unsafe{WaitForSingleObject(self.worker.raw(),0)}==WAIT_OBJECT_0;let encoder=unsafe{WaitForSingleObject(self.encoder.raw(),0)}==WAIT_OBJECT_0;
  std::fs::write(path.join("cleanup.json"),serde_json::to_vec(&serde_json::json!({"workerSignaled":worker,"encoderSignaled":encoder,"qpc":qpc()?,"exactNoninheritableHandles":true})).map_err(err)?).map_err(err)?;
  if !worker||!encoder{return Err("Exact child cleanup not observed".into());}Ok(())
 }
}
pub(super) fn arm_observation(session:&Session,dir:&Path)->Result<()> {
 let encoder=unsafe{GetProcessId(session.encoder.as_ref().ok_or("No encoder")?.raw())};
 std::fs::write(dir.join("owned.json"),serde_json::to_vec(&serde_json::json!({"parent":std::process::id(),"worker":session.pid(),"encoder":encoder,"qpc":qpc()?})).map_err(err)?).map_err(err)?;
 {let deadline=Instant::now()+Duration::from_secs(2);while !dir.join("handles-open").exists(){if Instant::now()>=deadline{return Err("Exact observer handshake missing".into());}thread::sleep(POLL);}}
 Ok(())
}
pub(super) fn park(path:&Path)->Result<()> {
 let dir=path.parent().ok_or("No case directory")?;
 if dir.join("parent-death").exists(){std::fs::write(dir.join("parked"),b"gate4 passed; no supervisor or EOF reader started").map_err(err)?;thread::sleep(Duration::from_secs(30));return Err("Parent-death isolation escape fired".into());}Ok(())
}
