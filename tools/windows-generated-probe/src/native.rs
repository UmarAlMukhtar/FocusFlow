use std::{error::Error, fs::File, io::{Read, Write}, os::windows::io::AsRawHandle,
    path::{Path,PathBuf}, process::{Child,Command,Stdio}, sync::atomic::{AtomicU16,Ordering},
    thread, time::{Duration,Instant}};
use focusflow_generated_windows_probe::{marker,qpc_100ns,WIDTH,HEIGHT,MAX_FRAMES};
use windows::{core::w, Win32::{Foundation::*, Graphics::{Gdi::*, Dwm::*}, System::{
    LibraryLoader::GetModuleHandleW, Performance::*, JobObjects::*, Threading::GetCurrentProcessId},
    UI::{WindowsAndMessaging::*, HiDpi::*}}};
use windows_capture::{capture::{Context,GraphicsCaptureApiHandler}, encoder::{AudioSettingsBuilder,
    ContainerSettingsBuilder,VideoEncoder,VideoSettingsBuilder}, frame::Frame,
    graphics_capture_api::InternalCaptureControl,settings::*,window::Window};
type Result<T> = std::result::Result<T,Box<dyn Error+Send+Sync>>;
static ID: AtomicU16 = AtomicU16::new(0);

// These QPC APIs only write through valid stack pointers; no handle ownership is transferred.
fn qpc() -> Result<i64> { let mut t=0; unsafe { QueryPerformanceCounter(&mut t)?; } Ok(t) }
fn frequency() -> Result<i64> { let mut f=0; unsafe { QueryPerformanceFrequency(&mut f)?; } if f<=0 {return Err("invalid QPC frequency".into())} Ok(f) }
// All handles below are created in this process; no borrowed DC/brush is retained or deleted.
struct OwnedWindow(HWND);
impl Drop for OwnedWindow { fn drop(&mut self) { unsafe { let _=DestroyWindow(self.0); } } }
struct Job(HANDLE);
impl Drop for Job { fn drop(&mut self) { unsafe { let _=CloseHandle(self.0); } } }
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

unsafe fn fill(dc:HDC,r:RECT,color:u32) {
    // Brush is never selected into a DC; FillRect borrows it synchronously.
    unsafe { let b=CreateSolidBrush(COLORREF(color)); if !b.is_invalid() { FillRect(dc,&r,b); let _=DeleteObject(b.into()); } }
}
unsafe extern "system" fn wndproc(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM)->LRESULT {
    // OS invokes this for our registered class; BeginPaint/EndPaint pair owns its temporary DC.
    unsafe {
        if msg==WM_PAINT {
            let mut ps=PAINTSTRUCT::default(); let dc=BeginPaint(hwnd,&mut ps);
            fill(dc,RECT{left:0,top:0,right:640,bottom:360},0x606060);
            for (x,y,c) in [(0,0,0x0000ff),(608,0,0x00ff00),(0,328,0xff0000),(608,328,0x00ffff)] {
                fill(dc,RECT{left:x,top:y,right:x+32,bottom:y+32},c);
            }
            let id=ID.load(Ordering::Relaxed);
            for b in 0..16 { let x=32+b*32; let c=if id&(1<<b)!=0 {0xf0f0f0}else{0x0c0c0c};
                fill(dc,RECT{left:x,top:128,right:x+24,bottom:192},c);
            }
            let _=EndPaint(hwnd,&ps); return LRESULT(0);
        }
        if msg==WM_CLOSE { let _=DestroyWindow(hwnd); return LRESULT(0); }
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
fn parent(scenario:&str,out:&Path)->Result<()> {
    if !["first","static","animated","stall","close"].contains(&scenario) {return Err("unknown scenario".into())}
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
    let job=unsafe {
        let job=Job(CreateJobObjectW(None,None)?);
        let mut info=JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job.0,JobObjectExtendedLimitInformation,&info as *const _ as _,std::mem::size_of_val(&info) as u32)?;
        job
    };
    let mut worker=Worker(Command::new(std::env::current_exe()?).arg("worker").arg(format!("{}",hwnd.0 as usize))
        .arg(std::process::id().to_string()).arg(scenario).arg(out)
        .stdin(Stdio::piped()).stdout(File::create(out.join("native.jsonl"))?).stderr(File::create(out.join("worker.stderr"))?).spawn()?);
    // Child waits for one gate byte before WGC, and exits on pre-assignment EOF. Job is non-inheritable.
    unsafe { AssignProcessToJobObject(job.0,HANDLE(worker.0.as_raw_handle()))?; }
    worker.0.stdin.as_mut().ok_or("missing control pipe")?.write_all(&[1])?;
    let start=Instant::now(); let mut ready=None; let mut stop_sent=false; let mut next=Duration::ZERO;
    let duration=if scenario=="first" {Duration::from_millis(350)} else {Duration::from_secs(3)};
    let mut paints=File::create(out.join("paint.jsonl"))?;
    loop {
        if let Some(status)=worker.0.try_wait()? { if status.success() {break} return Err(format!("worker exited {status}").into()); }
        if start.elapsed()>Duration::from_secs(18) {return Err("worker deadline exceeded; requesting owned termination".into())}
        if ready.is_none() && out.join("ready").is_file() { ready=Some(Instant::now()); }
        if !stop_sent && ready.is_some_and(|t:Instant|t.elapsed()>=duration) {
            if scenario=="close" { window.take(); }
            else {worker.0.stdin.take();}
            stop_sent=true;
        }
        if window.is_some() && start.elapsed()>=next {
            let animated=scenario!="static" && scenario!="first";
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
    println!("{{\"event\":\"parentReaped\",\"scenario\":\"{scenario}\"}}"); Ok(())
}
#[derive(Clone)]
struct Flags { out:PathBuf, stall:bool }
struct Capture { encoder:Option<VideoEncoder>, count:u32, first:Option<i64>, flags:Flags, freq:i64 }
impl Capture {
    fn finish(&mut self)->Result<()> {
        if let Some(e)=self.encoder.take() { println!("{{\"event\":\"finishBegin\",\"qpc\":{}}}",qpc()?); e.finish()?;
            println!("{{\"event\":\"finishComplete\",\"qpc\":{},\"enqueued\":{}}}",qpc()?,self.count); }
        Ok(())
    }
}
impl GraphicsCaptureApiHandler for Capture {
    type Flags=Flags; type Error=Box<dyn Error+Send+Sync>;
    fn new(ctx:Context<Flags>)->Result<Self> {
        Ok(Self { encoder:Some(VideoEncoder::new(VideoSettingsBuilder::new(WIDTH as u32,HEIGHT as u32).frame_rate(30).bitrate(8_000_000),AudioSettingsBuilder::default().disabled(true),ContainerSettingsBuilder::default(),ctx.flags.out.join("screen.mp4"))?),count:0,first:None,flags:ctx.flags,freq:frequency()? })
    }
    fn on_frame_arrived(&mut self,frame:&mut Frame,control:InternalCaptureControl)->Result<()> {
        let before=qpc()?; let timestamp=frame.timestamp()?.Duration;
        if self.count>=MAX_FRAMES {control.stop(); return Err("native frame receipt cap reached".into())}
        let (w,h)=(frame.width(),frame.height());
        if (w,h)!=(WIDTH as u32,HEIGHT as u32) {return Err(format!("unexpected geometry {w}x{h}; no coordinate inference").into())}
        let id={ let buffer=frame.buffer()?; let mut packed=Vec::new(); let p=buffer.as_nopadding_buffer(&mut packed); marker(p,4) };
        if self.flags.stall && self.count==15 {
            println!("{{\"event\":\"stallBegin\",\"qpc\":{}}}",qpc()?);
            thread::sleep(Duration::from_millis(350));
            println!("{{\"event\":\"stallEnd\",\"qpc\":{}}}",qpc()?);
        }
        let submit_before=qpc()?;
        self.encoder.as_mut().ok_or("encoder already finished")?.send_frame(frame)?;
        let after=qpc()?; let first=*self.first.get_or_insert(timestamp);
        let delta=timestamp.checked_sub(first).ok_or("native delta overflow")?;
        let seq=self.count; self.count=self.count.checked_add(1).ok_or("sequence overflow")?;
        println!("{{\"event\":\"enqueued\",\"seq\":{seq},\"native100ns\":{timestamp},\"first100ns\":{first},\"delta100ns\":{delta},\"width\":{w},\"height\":{h},\"id\":{},\"qpcBefore\":{before},\"qpcSubmitBefore\":{submit_before},\"qpcAfter\":{after},\"qpcBefore100ns\":{},\"qpcAfter100ns\":{}}}",id.map(|x|x.to_string()).unwrap_or("null".into()),qpc_100ns(before,self.freq).ok_or("QPC overflow")?,qpc_100ns(after,self.freq).ok_or("QPC overflow")?);
        std::io::stdout().flush()?; Ok(())
    }
    fn on_closed(&mut self)->Result<()> {println!("{{\"event\":\"sourceClosed\"}}"); self.finish()}
}
fn worker(hwnd:usize,pid:u32,scenario:&str,out:&Path)->Result<()> {
    let mut gate=[0]; std::io::stdin().read_exact(&mut gate)?; if gate!=[1] {return Err("invalid ownership gate".into())}
    let hwnd=HWND(hwnd as *mut _); let mut owner=0;
    // Borrow only the generated parent's window, validated before capture and never enumerating windows.
    unsafe { GetWindowThreadProcessId(hwnd,Some(&mut owner)); }
    if owner!=pid || owner==unsafe{GetCurrentProcessId()} {return Err("fixture owner mismatch".into())}
    let window=Window::from_raw_hwnd(hwnd.0);
    if window.title()? != "FocusFlow generated probe fixture" || !window.is_valid() {return Err("fixture not available".into())}
    let freq=frequency()?;
    println!("{{\"event\":\"startup\",\"qpc\":{},\"frequency\":{freq},\"pid\":{},\"ownerPid\":{pid},\"cursorRequest\":\"WithoutCursor\",\"configuredFps\":30}}",qpc()?,std::process::id());
    geometry(hwnd)?;
    // Explicit WithoutCursor is runtime-checked by pinned WGC; unsupported capability is an error.
    let settings=Settings::new(window,CursorCaptureSettings::WithoutCursor,DrawBorderSettings::Default,SecondaryWindowSettings::Default,MinimumUpdateIntervalSettings::Default,DirtyRegionSettings::Default,ColorFormat::Rgba8,Flags{out:out.into(),stall:scenario=="stall"});
    let capture=Capture::start_free_threaded(settings)?;
    println!("{{\"event\":\"captureStarted\",\"qpc\":{}}}",qpc()?);
    // This gate marks startup completion, never the first compositor or saved-frame timestamp.
    std::fs::write(out.join("ready"),b"capture-started\n")?;
    let (tx,rx)=std::sync::mpsc::channel();
    thread::spawn(move||{let _=std::io::stdin().read(&mut [0]);let _=tx.send(());});
    let start=Instant::now();
    while !capture.is_finished() && rx.try_recv().is_err() && start.elapsed()<Duration::from_secs(5) {thread::sleep(Duration::from_millis(10));}
    let callback=capture.callback();
    if capture.is_finished() {capture.wait()?;} else {capture.stop()?;}
    let mut callback=callback.lock(); callback.finish()?;
    if callback.count==0 {return Err("no compositor frames received".into())}
    Ok(())
}
pub fn run()->Result<()> {
    let args:Vec<_>=std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("worker") if args.len()==6=>worker(args[2].parse()?,args[3].parse()?,&args[4],Path::new(&args[5])),
        Some("run") if args.len()==4=>parent(&args[2],Path::new(&args[3])),
        _=>Err("usage: probe run first|static|animated|stall|close NEW_OUTPUT_DIR".into())
    }
}
