use std::{error::Error, fs::File, io::{Read, Write}, os::windows::io::AsRawHandle,
    path::Path, process::{Child,Command,Stdio}, sync::{Arc,Mutex,mpsc::{self,SyncSender,Receiver},atomic::{AtomicU16,AtomicBool,AtomicUsize,Ordering}},
    thread, time::{Duration,Instant}};
use focusflow_generated_windows_probe::{marker,stamp_tag,valid_corners,admitted,complete_cells,write_frame,finish_supervisor,SupervisorResult,OwnedPixels,OwnedCount,Source,Selector,WIDTH,HEIGHT,MAX_FRAMES};
use windows::{core::{w,PCWSTR,BOOL}, Win32::{Foundation::*, Graphics::{Gdi::*, Dwm::*}, System::{
    LibraryLoader::GetModuleHandleW, Performance::*, JobObjects::*, Threading::{GetCurrentProcessId,OpenProcess,PROCESS_TERMINATE,TerminateProcess}},
    UI::{WindowsAndMessaging::*, HiDpi::*}}};
use windows_capture::{capture::{Context,GraphicsCaptureApiHandler}, frame::Frame,
    graphics_capture_api::InternalCaptureControl,settings::*,window::Window};
type Result<T> = std::result::Result<T,Box<dyn Error+Send+Sync>>;
static ID: AtomicU16 = AtomicU16::new(0);

// These QPC APIs only write through valid stack pointers; no handle ownership is transferred.
fn qpc() -> Result<i64> { let mut t=0; unsafe { QueryPerformanceCounter(&mut t)?; } Ok(t) }
fn frequency() -> Result<i64> { let mut f=0; unsafe { QueryPerformanceFrequency(&mut f)?; } if f<=0 {return Err("invalid QPC frequency".into())} Ok(f) }
// All handles below are created in this process; no borrowed DC/brush is retained or deleted.
struct OwnedWindow(HWND);
impl Drop for OwnedWindow { fn drop(&mut self) { unsafe { let _=DestroyWindow(self.0); } } }
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle { fn drop(&mut self) { unsafe { let _=CloseHandle(self.0); } } }
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) { return; }
        let _=self.0.kill();
        let deadline=Instant::now()+Duration::from_secs(2);
        while Instant::now()<deadline {
            if matches!(self.0.try_wait(), Ok(Some(_))) { return; }
            thread::sleep(Duration::from_millis(10));
        }
        // The non-inherited Job handle closes next, killing remaining owned processes.
        eprintln!("worker termination not observed before cleanup deadline");
    }
}

static PAINT_FAILED:AtomicBool=AtomicBool::new(false);
thread_local! {static CANVAS:std::cell::RefCell<Option<Canvas>>=const {std::cell::RefCell::new(None)};}
struct Canvas {dc:HDC,bitmap:HBITMAP,previous:HGDIOBJ}
impl Drop for Canvas {
 fn drop(&mut self){
  // Restore the original selected object before deleting our bitmap and memory DC.
  unsafe{SelectObject(self.dc,self.previous);let _=DeleteObject(self.bitmap.into());let _=DeleteDC(self.dc);}
 }
}
impl Canvas {
 fn new(target:HDC)->Result<Self>{
  // Compatible DC/bitmap are owned here; failures release already-created resources.
  unsafe {
   let dc=CreateCompatibleDC(Some(target));if dc.is_invalid(){return Err("CreateCompatibleDC failed".into());}
   let bitmap=CreateCompatibleBitmap(target,WIDTH as i32,HEIGHT as i32);
   if bitmap.is_invalid(){let _=DeleteDC(dc);return Err("CreateCompatibleBitmap failed".into());}
   let previous=SelectObject(dc,bitmap.into());
   if previous.is_invalid(){let _=DeleteObject(bitmap.into());let _=DeleteDC(dc);return Err("SelectObject failed".into());}
   Ok(Self{dc,bitmap,previous})
  }
 }
 fn fill(&self,r:RECT,color:u32)->Result<()>{
  // FillRect borrows a brush; it is never selected and is deleted on either result.
  unsafe {let brush=CreateSolidBrush(COLORREF(color));if brush.is_invalid(){return Err("brush failed".into());}
   let ok=FillRect(self.dc,&r,brush);let _=DeleteObject(brush.into());if ok==0{return Err("FillRect failed".into());}}
  Ok(())
 }
 fn paint(&self,target:HDC)->Result<()>{
  self.fill(RECT{left:0,top:0,right:640,bottom:360},0x606060)?;
  for (x,y,c) in [(0,0,0x0000ff),(608,0,0x00ff00),(0,328,0xff0000),(608,328,0x00ffff)] {
   self.fill(RECT{left:x,top:y,right:x+32,bottom:y+32},c)?;
  }
  let id=ID.load(Ordering::Relaxed);
  for b in 0..16 {let x=32+b*32;self.fill(RECT{left:x,top:128,right:x+24,bottom:192},if id&(1<<b)!=0{0xf0f0f0}else{0x0c0c0c})?;}
  // Both DCs remain valid and the whole owned bitmap is copied in one checked operation.
  unsafe{BitBlt(target,0,0,640,360,Some(self.dc),0,0,SRCCOPY)?;}Ok(())
 }
}
unsafe extern "system" fn wndproc(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM)->LRESULT {
 // OS owns this callback; BeginPaint/EndPaint pair borrows one valid paint DC.
 unsafe {
  if msg==WM_PAINT {
   let mut ps=PAINTSTRUCT::default();let dc=BeginPaint(hwnd,&mut ps);
   let result:Result<()>=CANVAS.with(|slot|{let mut slot=slot.borrow_mut();if slot.is_none(){*slot=Some(Canvas::new(dc)?);}
    slot.as_ref().ok_or("canvas unavailable")?.paint(dc)});
   if result.is_err(){PAINT_FAILED.store(true,Ordering::SeqCst);}
   let _=EndPaint(hwnd,&ps);return LRESULT(0);
  }
  if msg==WM_CLOSE{let _=DestroyWindow(hwnd);return LRESULT(0);}
  DefWindowProcW(hwnd,msg,wp,lp)
 }
}
fn geometry(hwnd:HWND)->Result<()> {
    let before=qpc()?;
    let mut client=RECT::default(); let mut rect=RECT::default(); let mut dwm=RECT::default();
    let mut origin=POINT::default();
    // HWND remains owned by the parent for this query; output pointers are valid stack storage.
    unsafe {
        GetClientRect(hwnd,&mut client)?; GetWindowRect(hwnd,&mut rect)?;
        if !ClientToScreen(hwnd,&mut origin).as_bool() {return Err("ClientToScreen failed".into())}
        let dwm_ok=DwmGetWindowAttribute(hwnd,DWMWA_EXTENDED_FRAME_BOUNDS,&mut dwm as *mut _ as _,std::mem::size_of::<RECT>() as u32).is_ok();
        println!("{{\"event\":\"geometry\",\"qpcBefore\":{before},\"qpcAfter\":{},\"client\":[{},{},{},{}],\"window\":[{},{},{},{}],\"origin\":[{},{}],\"dwmOk\":{dwm_ok},\"dwm\":[{},{},{},{}],\"dpi\":{}}}",qpc()?,client.left,client.top,client.right,client.bottom,rect.left,rect.top,rect.right,rect.bottom,origin.x,origin.y,dwm.left,dwm.top,dwm.right,dwm.bottom,GetDpiForWindow(hwnd));
    } Ok(())
}
fn parent(scenario:&str,out:&Path,ffmpeg:&Path)->Result<()> {
    if !["first","static","animated","stall","close","blocked","exit-midframe","parent-death"].contains(&scenario) {return Err("unknown scenario".into())}
    // Process exit also closes the Job if the parent itself blocks inside a native call.
    thread::spawn(|| { thread::sleep(Duration::from_secs(23)); std::process::exit(3); });
    std::fs::create_dir(out)?;
    // This process owns the class and window until capture has stopped; no external HWND is selected.
    let hwnd=unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        let instance=GetModuleHandleW(None)?;
        let class=WNDCLASSW{lpfnWndProc:Some(wndproc),hInstance:instance.into(),lpszClassName:w!("FocusFlowGeneratedOnly"),..Default::default()};
        if RegisterClassW(&class)==0 {return Err("RegisterClassW failed".into())}
        CreateWindowExW(WINDOW_EX_STYLE::default(),w!("FocusFlowGeneratedOnly"),w!("FocusFlow generated probe fixture"),WS_POPUP|WS_VISIBLE,80,80,WIDTH as i32,HEIGHT as i32,None,None,Some(instance.into()),None)?
    };
    let mut window=Some(OwnedWindow(hwnd));
    geometry(hwnd)?;
    let job_name=format!("Local\\FocusFlowGenerated-{}-{}",std::process::id(),qpc()?);
    let wide:Vec<u16>=job_name.encode_utf16().chain([0]).collect();
    let job=unsafe {
        let job=OwnedHandle(CreateJobObjectW(None,PCWSTR(wide.as_ptr()))?);
        let mut info=JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job.0,JobObjectExtendedLimitInformation,&info as *const _ as _,std::mem::size_of_val(&info) as u32)?;
        job
    };
    let mut worker=Worker(Command::new(std::env::current_exe()?).arg("worker").arg(format!("{}",hwnd.0 as usize))
        .arg(std::process::id().to_string()).arg(scenario).arg(out).arg(ffmpeg).arg(&job_name)
        .stdin(Stdio::piped()).stdout(File::create(out.join("native.jsonl"))?).stderr(File::create(out.join("worker.stderr"))?).spawn()?);
    println!("{{\"event\":\"workerOwned\",\"pid\":{}}}",worker.0.id());
    // Child waits for one gate byte before WGC, and exits on pre-assignment EOF. Job is non-inheritable.
    unsafe { AssignProcessToJobObject(job.0,HANDLE(worker.0.as_raw_handle()))?; }
    worker.0.stdin.as_mut().ok_or("missing control pipe")?.write_all(&[1])?;
    let start=Instant::now(); let mut ready=None; let mut stop_sent=false; let mut next=Duration::ZERO;
    let duration=if scenario=="first" {Duration::from_millis(350)} else {Duration::from_secs(3)};
    let mut paints=File::create(out.join("paint.jsonl"))?;
    loop {
        if let Some(status)=worker.0.try_wait()? { if status.success() {break} return Err(format!("worker exited {status}").into()); }
        if start.elapsed()>Duration::from_secs(18) {return Err("worker deadline exceeded; requesting owned termination".into())}
        if ready.is_none() && out.join("ready").is_file() { ready=Some(Instant::now()); next=Duration::ZERO; }
        if PAINT_FAILED.load(Ordering::SeqCst){return Err("generated offscreen painting failed".into());}
        if !stop_sent && scenario!="parent-death" && ready.is_some_and(|t:Instant|t.elapsed()>=duration) {
            let request=qpc()?;let reason=if scenario=="close"{"close"}else{"stop"};
            writeln!(worker.0.stdin.as_mut().ok_or("control pipe missing")?,"stop {request} {reason}")?;
            println!("{{\"event\":\"stopRequested\",\"qpc\":{request}}}");
            stop_sent=true;
        }
        if scenario=="close" && stop_sent && out.join("stop-ack").is_file() && window.is_some(){
            println!("{{\"event\":\"destroyAfterAck\",\"qpc\":{}}}",qpc()?);window.take();
        }
        if window.is_some() && start.elapsed()>=next {
            let animated=!["static","first","blocked","exit-midframe"].contains(&scenario);
            if animated || next==Duration::ZERO {
                let elapsed=ready.map_or(Duration::ZERO,|t:Instant|t.elapsed());
                let id=u16::try_from((elapsed.as_millis()/16).min(599))?;
                ID.store(if animated{id}else{0},Ordering::Relaxed);
                let a=qpc()?;
                unsafe { let _=InvalidateRect(Some(hwnd),None,false); let _=UpdateWindow(hwnd); }
                writeln!(paints,"{{\"id\":{},\"qpcBefore\":{a},\"qpcAfter\":{}}}",ID.load(Ordering::Relaxed),qpc()?)?;
            }
            next=start.elapsed()+Duration::from_millis(16);
        }
        let mut msg=MSG::default();
        unsafe { while PeekMessageW(&mut msg,None,0,0,PM_REMOVE).as_bool() { let _=TranslateMessage(&msg); DispatchMessageW(&msg); } }
        thread::sleep(Duration::from_millis(2));
    }
    CANVAS.with(|slot|{slot.borrow_mut().take();});
    println!("{{\"event\":\"parentReaped\",\"scenario\":\"{scenario}\"}}"); Ok(())
}
static RECEIPT_BYTES:AtomicUsize=AtomicUsize::new(0);
fn event(line:String)->Result<()> {
 if RECEIPT_BYTES.fetch_add(line.len()+1,Ordering::SeqCst)+line.len()+1>2*1024*1024{return Err("native receipt cap".into());}
 let mut out=std::io::stdout().lock();writeln!(out,"{line}")?;out.flush()?;Ok(())
}
fn quoted(s:&str)->String {
 let mut out=String::from("\"");for c in s.chars(){match c {'"'=>out.push_str("\\\""),'\\'=>out.push_str("\\\\"),'\n'=>out.push_str("\\n"),'\r'=>out.push_str("\\r"),'\t'=>out.push_str("\\t"),c if c<' '=>out.push_str(&format!("\\u{:04x}",c as u32)),c=>out.push(c)}}out.push('"');out
}
struct Receipt{file:File,bytes:usize}
impl Receipt {
 fn line(&mut self,s:String)->Result<()> {self.bytes=self.bytes.checked_add(s.len()+1).ok_or("receipt overflow")?;
  if self.bytes>2*1024*1024{return Err("selection receipt cap".into());}writeln!(self.file,"{s}")?;self.file.flush()?;Ok(())}
}
#[derive(Clone,Default)]
struct Shared {queued:usize,source_high:usize,cutoff:Option<i64>,closed:bool,failure:Option<String>}
#[derive(Clone)]
struct Flags {tx:SyncSender<Source>,shared:Arc<Mutex<Shared>>,live:Arc<OwnedCount>,admission:i64,freq:i64,stall:bool}
struct Capture {flags:Flags,count:u32}
impl Capture {
 fn copy_frame(&mut self,frame:&mut Frame)->Result<()> {
  let callback=qpc()?;let native=frame.timestamp()?.Duration;
  if self.count>=MAX_FRAMES{return Err("native snapshot cap reached".into());}
  let (w,h)=(frame.width(),frame.height());
  if (w,h)!=(WIDTH as u32,HEIGHT as u32){return Err(format!("unexpected geometry {w}x{h}").into());}
  if self.flags.stall&&self.count==15 {
   event(format!("{{\"event\":\"stallBegin\",\"qpc\":{}}}",qpc()?))?;
   thread::sleep(Duration::from_millis(350));
   event(format!("{{\"event\":\"stallEnd\",\"qpc\":{}}}",qpc()?))?;
  }
  let begin=qpc()?;
  let mut buffer=frame.buffer()?;let pitch=buffer.row_pitch() as usize;
  if buffer.color_format()!=ColorFormat::Bgra8{return Err("unexpected pixel format".into());}
  let mut owned=OwnedPixels::copy(buffer.as_raw_buffer(),pitch,self.flags.live.clone())?;
  let id=marker(&owned.bytes,4);let corners=valid_corners(&owned.bytes);
  stamp_tag(&mut owned.bytes,self.count as u16)?;let end=qpc()?;
  // This lock serializes publication with decision sampling. copyComplete is the
  // published completion boundary, so a completed snapshot cannot hide in transit.
  let pre=!admitted(native,self.flags.admission,self.flags.freq)?;let seq=self.count;self.count+=1;
  let (complete,published)={
   let mut shared=self.flags.shared.lock().map_err(|_|"snapshot state poisoned")?;
   let complete=qpc()?;
   let published=match id {
    None=>Err("native original ID undecodable"),
    Some(id) if !corners||id>=600=>Err("native original marker/corners invalid"),
    Some(id)=>self.flags.tx.try_send(Source{seq,native,copy_complete:complete,id,pixels:Arc::new(owned)}).map_err(|_|"source snapshot queue overflow/disconnected")
   };
   if published.is_ok(){shared.queued+=1;shared.source_high=shared.source_high.max(shared.queued);}
   (complete,published)
  };
  event(format!("{{\"event\":\"snapshot\",\"seq\":{seq},\"native100ns\":{native},\"width\":{w},\"height\":{h},\"id\":{},\"cornersValid\":{corners},\"callbackQpc\":{callback},\"copyStartQpc\":{begin},\"copyEndQpc\":{end},\"copyCompleteQpc\":{complete},\"preRoll\":{pre}}}",id.map_or("null".into(),|x|x.to_string())))?;
  published?;Ok(())
 }
}
impl GraphicsCaptureApiHandler for Capture {
 type Flags=Flags;type Error=Box<dyn Error+Send+Sync>;
 fn new(ctx:Context<Flags>)->Result<Self>{Ok(Self{flags:ctx.flags,count:0})}
 fn on_frame_arrived(&mut self,frame:&mut Frame,control:InternalCaptureControl)->Result<()> {
  if let Err(e)=self.copy_frame(frame){
   if let Ok(mut state)=self.flags.shared.lock(){state.failure=Some(e.to_string());}
   control.stop();return Err(e);
  }Ok(())
 }
 fn on_closed(&mut self)->Result<()> {
  let mut state=self.flags.shared.lock().map_err(|_|"source close state poisoned")?;
  let observed=qpc()?;state.cutoff=Some(state.cutoff.map_or(observed,|old|old.min(observed)));state.closed=true;
  drop(state);
  event(format!("{{\"event\":\"sourceClosed\",\"qpc\":{observed}}}"))
 }
}
#[derive(Clone,Default)]
struct Progress {began:Option<Instant>,queued:usize,high:usize,complete:u32,done:bool}
struct Output {ordinal:u32,source:Source}
fn write_outputs(mut pipe:impl Write,rx:Receiver<Output>,ledger:Arc<Mutex<Receipt>>,progress:Arc<Mutex<Progress>>)->Result<()> {
 while let Ok(output)=rx.recv(){
  {let mut p=progress.lock().map_err(|_|"writer state poisoned")?;p.queued-=1;p.began=Some(Instant::now());}
  let begin=qpc()?;let mut bytes=0;let result=write_frame(&mut pipe,&output.source.pixels.bytes,&mut bytes);let end=qpc()?;
  ledger.lock().map_err(|_|"ledger poisoned")?.line(format!("{{\"event\":\"written\",\"ordinal\":{},\"writeBeginQpc\":{begin},\"writeEndQpc\":{end},\"bytesWritten\":{bytes}}}",output.ordinal))?;
  result?;
  let mut p=progress.lock().map_err(|_|"writer state poisoned")?;p.complete+=1;p.began=None;
 }
 drop(pipe);Ok(())
}
fn verify_job(child:&Child,name:&str)->Result<()> {
 let wide:Vec<u16>=name.encode_utf16().chain([0]).collect();let mut inside=BOOL(0);
 // Query only our named parent Job; close this handle before feeding so parent
 // death still closes the last Job handle and terminates both descendants.
 unsafe {let job=OwnedHandle(OpenJobObjectW(0x0004 /* JOB_OBJECT_QUERY */,false,PCWSTR(wide.as_ptr()))?);
  IsProcessInJob(HANDLE(child.as_raw_handle()),Some(job.0),&mut inside)?;}
 if !inside.as_bool(){return Err("encoder not in owned parent Job".into());}Ok(())
}
fn bounded_reap(child:&mut Child,kill:bool)->Result<i32>{
 if kill{let _=child.kill();}let deadline=Instant::now()+Duration::from_secs(2);
 loop {if let Some(status)=child.try_wait()?{return Ok(status.code().unwrap_or(-1));}
  if Instant::now()>=deadline{return Err("encoder reap/finalization deadline".into());}thread::sleep(Duration::from_millis(5));}
}
fn worker(hwnd:usize,pid:u32,scenario:&str,out:&Path,ffmpeg:&Path,job_name:&str)->Result<()> {
 let mut gate=[0];std::io::stdin().read_exact(&mut gate)?;if gate!=[1]{return Err("invalid ownership gate".into());}
 let hwnd=HWND(hwnd as *mut _);let mut owner=0;
 // Borrow only the generated parent's HWND; never enumerate other user windows.
 unsafe{GetWindowThreadProcessId(hwnd,Some(&mut owner));}
 if owner!=pid||owner==unsafe{GetCurrentProcessId()}{return Err("fixture owner mismatch".into());}
 let window=Window::from_raw_hwnd(hwnd.0);
 if window.title()?!="FocusFlow generated probe fixture"||!window.is_valid(){return Err("fixture unavailable".into());}
 let freq=frequency()?;
 event(format!("{{\"event\":\"startup\",\"qpc\":{},\"frequency\":{freq},\"pid\":{},\"ownerPid\":{pid},\"cursorRequest\":\"WithoutCursor\"}}",qpc()?,std::process::id()))?;
 geometry(hwnd)?;
 let fault=["blocked","exit-midframe"].contains(&scenario);
 let mut command=if fault {let mut c=Command::new(std::env::current_exe()?);c.arg("reader").arg(scenario);c}
 else {let mut c=Command::new(ffmpeg);c.args(["-hide_banner","-loglevel","warning","-nostdin","-n","-f","rawvideo","-pixel_format","bgra","-video_size","640x360","-framerate","30","-i","pipe:0","-map","0:v:0","-an","-c:v","libx264","-preset","ultrafast","-crf","0","-pix_fmt","yuv444p","-bf","0","-fps_mode","passthrough","-video_track_timescale","30000"]).arg(out.join("screen.mp4"));c};
 let mut encoder=Worker(command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()?);
 verify_job(&encoder.0,job_name)?;
 event(format!("{{\"event\":\"encoderOwned\",\"pid\":{},\"inJob\":true,\"transportFault\":{fault}}}",encoder.0.id()))?;
 let mut stderr=encoder.0.stderr.take().ok_or("missing encoder stderr")?;
 let stderr_path=out.join("encoder.stderr");let (stderr_tx,stderr_rx)=mpsc::sync_channel(1);
 thread::spawn(move||{let result:Result<()>= (||{let mut file=File::create(stderr_path)?;let mut kept=0;let mut buf=[0;4096];
  loop{let n=stderr.read(&mut buf)?;if n==0{break;}let take=n.min(65536-kept);file.write_all(&buf[..take])?;kept+=take;}Ok(())})();let _=stderr_tx.send(result.map_err(|e|e.to_string()));});
 let ledger=Arc::new(Mutex::new(Receipt{file:File::create(out.join("selection.jsonl"))?,bytes:0}));
 let progress=Arc::new(Mutex::new(Progress::default()));let(writer_tx,writer_rx)=mpsc::sync_channel(2);let(done_tx,done_rx)=mpsc::sync_channel(1);
 let pipe=encoder.0.stdin.take().ok_or("missing encoder stdin")?;let writer_progress=progress.clone();let writer_ledger=ledger.clone();
 thread::spawn(move||{let r=write_outputs(pipe,writer_rx,writer_ledger,writer_progress.clone());if let Ok(mut p)=writer_progress.lock(){p.done=true;}let _=done_tx.send(r.map_err(|e|e.to_string()));});
 let supervisor_done=Arc::new(AtomicBool::new(false));let (supervisor_tx,supervisor_rx)=mpsc::sync_channel(1);
 let job_parked=Arc::new(AtomicBool::new(false));let monitor_parked=job_parked.clone();
 let monitor_done=supervisor_done.clone();let monitor_progress=progress.clone();let monitor_path=out.join("screen.mp4");let encoder_pid=encoder.0.id();
 // Transfer this owned terminate-only handle to the supervisor; no foreign PID is targeted.
 let process_handle=unsafe{OpenProcess(PROCESS_TERMINATE,false,encoder_pid)?}.0 as usize;
 thread::spawn(move||{let handle=OwnedHandle(HANDLE(process_handle as *mut _));
  let result=loop {
   if monitor_done.load(Ordering::SeqCst){break SupervisorResult{reason:None,request_succeeded:false,observed_qpc:0};}
   if monitor_parked.load(Ordering::SeqCst){thread::sleep(Duration::from_millis(5));continue;}
   let stalled=monitor_progress.lock().map(|p|p.began.is_some_and(|t|t.elapsed()>Duration::from_secs(1))).unwrap_or(true);
   let oversized=std::fs::metadata(&monitor_path).is_ok_and(|m|m.len()>64*1024*1024);
   if stalled||oversized {
    let reason=if stalled{"writer stalled over 1 second"}else{"media exceeds 64MiB"};
    // Only this supervisor owns the one-second action. Publish its result after the call returns.
    let killed=unsafe{TerminateProcess(handle.0,4)}.is_ok();
    break SupervisorResult{reason:Some(reason),request_succeeded:killed,observed_qpc:qpc().unwrap_or(-1)};
   }thread::sleep(Duration::from_millis(5));
  };
  let _=supervisor_tx.send(result);
 });
 let (control_tx,control_rx)=mpsc::sync_channel(1);
 thread::spawn(move||{let r:Result<String>=(||{let mut bytes=Vec::new();loop{let mut b=[0];if std::io::stdin().read(&mut b)?==0{return Err("parent control EOF before explicit Stop".into());}
  if b[0]==b'\n'{break;}bytes.push(b[0]);if bytes.len()>96{return Err("oversized control request".into());}}Ok(String::from_utf8(bytes)?)})();let _=control_tx.send(r.map_err(|e|e.to_string()));});
 let shared=Arc::new(Mutex::new(Shared::default()));let live=Arc::new(OwnedCount::default());let(tx,rx)=mpsc::sync_channel(8);
 let admission=qpc()?;event(format!("{{\"event\":\"admission\",\"qpc\":{admission}}}"))?;
 let settings=Settings::new(window,CursorCaptureSettings::WithoutCursor,DrawBorderSettings::Default,SecondaryWindowSettings::Default,MinimumUpdateIntervalSettings::Default,DirtyRegionSettings::Default,ColorFormat::Bgra8,Flags{tx,shared:shared.clone(),live:live.clone(),admission,freq,stall:scenario=="stall"});
 let capture=Capture::start_free_threaded(settings)?;
 event(format!("{{\"event\":\"captureStarted\",\"qpc\":{}}}",qpc()?))?;std::fs::write(out.join("ready"),b"capture-started\n")?;
 let mut selector=Selector::new(admission,freq)?;let mut ordinal=0;let mut delivered_through=None;let started=Instant::now();let mut stopped_at=None;
 let mut close_requested=false;let mut control_handled=false;let mut supervisor_observed=None;let mut final_state=Shared::default();
 let feed:Result<()>=(||{loop {
  match supervisor_rx.try_recv(){
   Ok(result)=>{let reason=result.reason.unwrap_or("supervisor stopped before feed finished");supervisor_observed=Some(result);return Err(reason.into());},
   Err(mpsc::TryRecvError::Disconnected)=>return Err("supervisor disconnected before completion".into()),Err(mpsc::TryRecvError::Empty)=>{}
  }
  if started.elapsed()>Duration::from_secs(8){return Err("worker feed deadline".into());}
  if scenario=="parent-death" {
   let p=progress.lock().map_err(|_|"writer state poisoned")?;
   let parked=p.complete>=1&&p.queued==0&&p.began.is_none();let complete=p.complete;
   if parked {job_parked.store(true,Ordering::SeqCst);}drop(p);
   if parked {
    // This generated-only phase cannot process EOF/sourceClosed or drop encoder guards.
    // Writer is idle with stdin held open; supervisor is parked; the Job query handle is closed.
    let qpc=qpc()?;
    event(format!("{{\"event\":\"jobProofParked\",\"qpc\":{qpc},\"parkSeconds\":10,\"workerPid\":{},\"encoderPid\":{encoder_pid},\"completeWrites\":{complete},\"writerIdle\":true}}",std::process::id()))?;
    thread::sleep(Duration::from_secs(10));return Err("Job proof park escaped without observed parent termination".into());
   }
  }
  match done_rx.try_recv(){Ok(Err(e))=>return Err(format!("writer failed: {e}").into()),Ok(Ok(()))=>return Err("writer ended before feed closed".into()),Err(mpsc::TryRecvError::Disconnected)=>return Err("writer disconnected".into()),Err(mpsc::TryRecvError::Empty)=>{}}
  let mut stop_receipt=None;let mut selected=None;let mut finished=false;
  {
   let mut state=shared.lock().map_err(|_|"capture state poisoned")?;
   if let Some(error)=&state.failure{return Err(error.clone().into());}
   if !control_handled {
    match control_rx.try_recv(){
     Ok(request)=>{let request=request?;let parts:Vec<_>=request.split_whitespace().collect();
      if parts.len()!=3||parts[0]!="stop"||!["stop","close"].contains(&parts[2]){return Err("invalid explicit Stop".into());}
      let requested:i64=parts[1].parse()?;let observed=qpc()?;if requested>observed{return Err("future Stop request".into());}
      state.cutoff=Some(state.cutoff.map_or(observed,|old|old.min(observed)));stopped_at=Some(Instant::now());close_requested=parts[2]=="close";control_handled=true;
      stop_receipt=Some((requested,observed,close_requested));
     },Err(mpsc::TryRecvError::Empty)=>{},Err(mpsc::TryRecvError::Disconnected)=>return Err("control disconnected".into())
    }
   }
   // Drain/decision/frontier are atomic with callback publication; all persistence follows unlock.
   while let Ok(source)=rx.try_recv(){state.queued-=1;delivered_through=Some(source.seq);selector.push(source)?;}
   let decision=qpc()?;
   if let Some(epoch)=selector.epoch {
    let until=state.cutoff.map_or(decision,|s|s.min(decision));let cells=complete_cells(epoch,until,freq)?;
    if ordinal>cells{return Err("already selected endpoint exceeds observed cutoff".into());}
    if cells>MAX_FRAMES{return Err("output frame cap reached".into());}
    if ordinal<cells && (!fault || ordinal==0) {
     selected=Some((ordinal,selector.select(ordinal,decision)?,decision,epoch,delivered_through.ok_or("missing delivery frontier")?));ordinal+=1;
    }
    if state.cutoff.is_some()&&ordinal==cells {
     finished=!close_requested||state.closed;
     if !finished&&stopped_at.is_some_and(|t:Instant|t.elapsed()>Duration::from_secs(2)){return Err("source close callback deadline".into());}
    }
   }else if started.elapsed()>Duration::from_secs(2){return Err("first admitted snapshot deadline".into());}
   if capture.is_finished()&&!state.closed{return Err("capture ended without source-close observation".into());}
   final_state=state.clone();
  }
  if let Some((requested,observed,close))=stop_receipt {
   event(format!("{{\"event\":\"stopInstalled\",\"qpc\":{observed},\"requestQpc\":{requested},\"reason\":{}}}",quoted(if close{"close"}else{"stop"})))?;
   std::fs::write(out.join("stop-ack"),observed.to_string())?;
  }
  if let Some((ordinal,source,decision,epoch,frontier))=selected {
   ledger.lock().map_err(|_|"ledger poisoned")?.line(format!("{{\"event\":\"selected\",\"ordinal\":{ordinal},\"targetNumerator\":{},\"targetDenominator\":300000000,\"sourceSeq\":{},\"sourceNative100ns\":{},\"sourceId\":{},\"decisionQpc\":{decision},\"deliveredThroughSeq\":{frontier}}}",i128::from(epoch)*30+i128::from(ordinal)*10_000_000,source.seq,source.native,source.id))?;
   let mut p=progress.lock().map_err(|_|"writer state poisoned")?;
   writer_tx.try_send(Output{ordinal,source}).map_err(|_|"output descriptor queue overflow/disconnected")?;
   p.queued+=1;p.high=p.high.max(p.queued);
   if fault {break;}
  }
  if finished{break;}thread::sleep(Duration::from_millis(2));
 }Ok(())})();
 let mut failure=feed.err().map(|e|e.to_string());
 // Stop/wait may block in the native library: observe a bounded result, never join it.
 let (capture_tx,capture_rx)=mpsc::sync_channel(1);
 thread::spawn(move||{let result=if capture.is_finished(){capture.wait()}else{capture.stop()};let _=capture_tx.send(result.map_err(|e|e.to_string()));});
 if failure.is_some(){let _=encoder.0.kill();}
 if let Err(e)=capture_rx.recv_timeout(Duration::from_secs(2)).map_err(|_|"capture stop deadline".to_string()).and_then(|r|r){failure.get_or_insert(e);let _=encoder.0.kill();}
 drop(writer_tx);
 let finish_start=Instant::now();let mut writer_done=false;
 while finish_start.elapsed()<Duration::from_secs(2){
  match done_rx.try_recv(){Ok(result)=>{writer_done=true;if let Err(e)=result{failure.get_or_insert(format!("writer failed: {e}"));}break;},Err(mpsc::TryRecvError::Disconnected)=>{writer_done=progress.lock().map_err(|_|"writer state poisoned")?.done;break;},Err(mpsc::TryRecvError::Empty)=>{}}
  thread::sleep(Duration::from_millis(5));
 }
 let fallback_cleanup=!writer_done;
 if fallback_cleanup{failure=Some("writer completion deadline; fallback cleanup without supervisor proof".into());}
 let exit=bounded_reap(&mut encoder.0,failure.is_some());let reaped=exit.is_ok();let code=exit.unwrap_or(-1);
 if std::fs::metadata(out.join("screen.mp4")).is_ok_and(|m|m.len()>64*1024*1024){failure.get_or_insert("media exceeds 64MiB".into());}
 let supervisor=match supervisor_observed {Some(result)=>Ok(result),None=>finish_supervisor(&supervisor_done,&supervisor_rx)};
 match supervisor {
  Ok(result)=>if let Some(reason)=result.reason {
   event(format!("{{\"event\":\"supervisorTermination\",\"pid\":{encoder_pid},\"requestSucceeded\":{},\"observedQpc\":{},\"reason\":{}}}",result.request_succeeded,result.observed_qpc,quoted(reason)))?;
   if !fallback_cleanup&&failure.as_ref().is_none_or(|s|s.starts_with("writer failed:")||s==reason){failure=Some(reason.into());}
  },
  Err(error)=>{failure=Some(error.into());let _=encoder.0.kill();}
 }
 if code!=0{failure.get_or_insert(format!("encoder exit {code}"));}
 if let Err(e)=stderr_rx.recv_timeout(Duration::from_millis(100)).map_err(|_|"stderr drain unconfirmed".to_string()).and_then(|r|r){failure.get_or_insert(e);}
 // A timed-out callback must not turn bounded finalization into a blocking lock acquisition.
 match shared.try_lock(){Ok(state)=>final_state=state.clone(),Err(_)=>{failure.get_or_insert("capture state unavailable after bounded stop".into());}}
 let state=final_state;
 let p=match progress.try_lock(){Ok(p)=>p.clone(),Err(_)=>{failure.get_or_insert("writer state unavailable after bounded finish".into());Progress::default()}};
 if let Some(error)=&state.failure{failure.get_or_insert(error.clone());}
 let cells=match (selector.epoch,state.cutoff){(Some(e),Some(s))=>Some(complete_cells(e,s,freq)?),_=>None};
 if cells!=Some(p.complete)||cells==Some(0){failure.get_or_insert("incomplete output or missing cutoff".into());}
 event(format!("{{\"event\":\"terminal\",\"epoch100ns\":{},\"stopQpc\":{},\"expectedCells\":{},\"completeWrites\":{},\"ffmpegExit\":{code},\"reaped\":{reaped},\"writerDone\":{writer_done},\"supervisorFallback\":{fallback_cleanup},\"sourceQueueHigh\":{},\"futureQueueHigh\":{},\"outputQueueHigh\":{},\"ownedHigh\":{},\"failure\":{}}}",selector.epoch.map_or("null".into(),|x|x.to_string()),state.cutoff.map_or("null".into(),|x|x.to_string()),cells.map_or("null".into(),|x|x.to_string()),p.complete,state.source_high,selector.high,p.high,live.high.load(Ordering::SeqCst),failure.as_ref().map_or("null".into(),|e|quoted(e))))?;
 if let Some(error)=failure{return Err(error.into());}Ok(())
}
pub fn run()->Result<()> {
 let args:Vec<_>=std::env::args().collect();
 match args.get(1).map(String::as_str){
  Some("worker") if args.len()==8=>worker(args[2].parse()?,args[3].parse()?,&args[4],Path::new(&args[5]),Path::new(&args[6]),&args[7]),
  Some("run") if args.len()==5=>parent(&args[2],Path::new(&args[3]),Path::new(&args[4])),
  Some("reader") if args.len()==3=>{if args[2]=="blocked"{thread::sleep(Duration::from_secs(15));}else if args[2]=="exit-midframe"{std::io::stdin().read_exact(&mut [0;65536])?;}else{return Err("unknown reader fault".into());}Ok(())},
  _=>Err("usage: probe run first|static|animated|stall|close|blocked|exit-midframe|parent-death NEW_OUTPUT_DIR VERIFIED_FFMPEG".into())
 }
}
