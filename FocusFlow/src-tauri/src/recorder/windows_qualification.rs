//! Owned generated HWND -> original Session -> original Completion -> public core import.
use super::*;
use std::{cell::RefCell, sync::mpsc, thread};
use ::windows::{core::w, Win32::{Foundation::{COLORREF,LRESULT,WPARAM},Graphics::Gdi::*,System::LibraryLoader::GetModuleHandleW,UI::{HiDpi::*,WindowsAndMessaging::*}}};
use focusflow_core::{capture::{decode_windows,windows::*},ffmpeg::probe_cfr_grid,store::ProjectStore};
type TestResult<T> = Result<T,Box<dyn std::error::Error+Send+Sync>>;
thread_local! {static CANVAS:RefCell<Option<Canvas>>=const{RefCell::new(None)};}
static PAINT_FAILED:AtomicBool=AtomicBool::new(false);
struct Canvas{dc:HDC,bitmap:HBITMAP,previous:HGDIOBJ,width:i32,height:i32,paint:u16,nonce:u16}
impl Drop for Canvas{fn drop(&mut self){unsafe{SelectObject(self.dc,self.previous);let _=DeleteObject(self.bitmap.into());let _=DeleteDC(self.dc);}}}
impl Canvas{
 fn new(target:HDC,width:i32,height:i32,nonce:u16)->TestResult<Self>{
  // Compatible memory DC/bitmap are owned only by the window thread and restored before deletion.
  unsafe{let dc=CreateCompatibleDC(Some(target));if dc.is_invalid(){return Err("CreateCompatibleDC failed".into());}
   let bitmap=CreateCompatibleBitmap(target,width,height);if bitmap.is_invalid(){let _=DeleteDC(dc);return Err("CreateCompatibleBitmap failed".into());}
   let previous=SelectObject(dc,bitmap.into());if previous.is_invalid(){let _=DeleteObject(bitmap.into());let _=DeleteDC(dc);return Err("SelectObject failed".into());}
   Ok(Self{dc,bitmap,previous,width,height,paint:1,nonce})}
 }
 fn fill(&self,x:i32,y:i32,w:i32,h:i32,luma:u32)->TestResult<()>{
  // FillRect borrows the brush; no selected brush or target DC escapes this call.
  unsafe{let brush=CreateSolidBrush(COLORREF(luma*0x010101));if brush.is_invalid(){return Err("brush failed".into());}let ok=FillRect(self.dc,&RECT{left:x,top:y,right:x+w,bottom:y+h},brush);let _=DeleteObject(brush.into());if ok==0{return Err("FillRect failed".into());}}Ok(())
 }
 fn draw(&self,target:HDC)->TestResult<()>{
  self.fill(0,0,self.width,self.height,96)?;
  for (x,y,c) in [(0,0,40),(self.width-16,0,100),(0,self.height-16,160),(self.width-16,self.height-16,220)]{self.fill(x,y,16,16,c)?;}
  for (y,v) in [(40,self.paint),(56,self.nonce),(72,self.paint),(88,self.nonce)]{for b in 0..16{self.fill(16+b*8,y,8,8,if v&(1<<b)!=0{240}else{12})?;}}
  unsafe{BitBlt(target,0,0,self.width,self.height,Some(self.dc),0,0,SRCCOPY)?;}Ok(())
 }
}
unsafe extern "system" fn wndproc(hwnd:HWND,msg:u32,wp:WPARAM,lp:LPARAM)->LRESULT{
 // The OS supplies HWND; the BeginPaint/EndPaint pair borrows a valid window DC.
 unsafe{if msg==WM_PAINT{let mut ps=PAINTSTRUCT::default();let dc=BeginPaint(hwnd,&mut ps);let result=CANVAS.with(|c|Ok::<_, &str>(c.borrow().as_ref().map_or(Ok(()),|c|c.draw(dc))));if !matches!(result,Ok(Ok(()))){PAINT_FAILED.store(true,Ordering::SeqCst);}let _=EndPaint(hwnd,&ps);return LRESULT(0);}DefWindowProcW(hwnd,msg,wp,lp)}
}
enum WindowCommand{Close,Resize}
struct GeneratedWindow{hwnd:usize,commands:mpsc::Sender<WindowCommand>,done:Option<thread::JoinHandle<TestResult<()>>>}
impl GeneratedWindow{
 fn new(w:u32,h:u32,nonce:u16,animated:bool,dir:PathBuf)->TestResult<Self>{
  let(tx,rx)=mpsc::channel();let(ready,result)=mpsc::sync_channel(1);
  let done=thread::spawn(move||->TestResult<()>{
   // Only this thread's window is created, resized and destroyed. Never sends input to user windows.
   let hwnd=unsafe{let instance=GetModuleHandleW(None)?;let class=WNDCLASSW{lpfnWndProc:Some(wndproc),hInstance:instance.into(),lpszClassName:w!("FocusFlowOwnedQualification"),..Default::default()};let _=RegisterClassW(&class);
    CreateWindowExW(WINDOW_EX_STYLE::default(),w!("FocusFlowOwnedQualification"),w!("FocusFlow generated owned content"),WS_POPUP|WS_VISIBLE,40,40,w as i32,h as i32,None,None,Some(instance.into()),None)?};
   let dc=unsafe{GetDC(Some(hwnd))};let canvas=Canvas::new(dc,w as i32,h as i32,nonce);unsafe{ReleaseDC(Some(hwnd),dc);}
   let canvas=canvas?;CANVAS.with(|slot|*slot.borrow_mut()=Some(canvas));
   unsafe{let _=InvalidateRect(Some(hwnd),None,false);let _=UpdateWindow(hwnd);}
   ready.send(hwnd.0 as usize)?;let mut paints=Vec::with_capacity(256);let start=Instant::now();let mut next=Instant::now();
   let result=(||->TestResult<()>{loop{
    match rx.try_recv(){Ok(WindowCommand::Close)=>break,Ok(WindowCommand::Resize)=>unsafe{SetWindowPos(hwnd,None,0,0,w as i32+2,h as i32+2,SWP_NOMOVE|SWP_NOZORDER)?;},Err(mpsc::TryRecvError::Disconnected)=>break,_=>{}}
    if PAINT_FAILED.load(Ordering::SeqCst){return Err("Generated painting failed".into());}
    if start.elapsed()>Duration::from_secs(40){return Err("Generated window escape deadline".into());}
    if Instant::now()>=next{
     if paints.len()>=1024{return Err("Paint history overflow".into());}
     CANVAS.with(|c|{let mut c=c.borrow_mut();let c=c.as_mut().unwrap();if animated{c.paint+=1;}paints.push((c.paint,c.nonce));});
     unsafe{let _=InvalidateRect(Some(hwnd),None,false);let _=UpdateWindow(hwnd);}next=Instant::now()+Duration::from_millis(50);
    }
    let mut message=MSG::default();unsafe{while PeekMessageW(&mut message,None,0,0,PM_REMOVE).as_bool(){let _=TranslateMessage(&message);DispatchMessageW(&message);}}thread::sleep(Duration::from_millis(2));
   }Ok(())})();
   unsafe{let _=DestroyWindow(hwnd);}CANVAS.with(|s|s.borrow_mut().take());fs::write(dir.join("paint-history.json"),serde_json::to_vec(&paints)?)?;result
  });
  Ok(Self{hwnd:result.recv_timeout(Duration::from_secs(3))?,commands:tx,done:Some(done)})
 }
 fn close(&mut self)->TestResult<()>{let _=self.commands.send(WindowCommand::Close);if let Some(h)=self.done.take(){h.join().map_err(|_|"Window thread panic")??;}Ok(())}
}
impl Drop for GeneratedWindow{fn drop(&mut self){let _=self.close();}}
fn incomplete(dir:&Path)->TestResult<()>{let m=decode_windows(&fs::read(dir.join("capture.json"))?,dir.file_name().unwrap().to_str().unwrap())?;assert!(matches!(m.state,WindowsCaptureStateV2::Incomplete{..}));assert!(!dir.join("project.json").exists());Ok(())}
fn case(root:&Path,id:&str,mode:&str,w:u32,h:u32,nonce:u16,ffmpeg:&Path,worker:&Path,state:&RecorderState)->TestResult<()>{
 let dir=root.join(id);fs::create_dir(&dir)?;let dir=dir.canonicalize()?;
 if mode=="parent-death"{fs::write(dir.join("parent-death"),b"generated isolation only")?;}
 let mut window=GeneratedWindow::new(w,h,nonce,mode!="static",dir.clone())?;
 let (window_capture,bounds)=window_capture_plan_from_hwnd(HWND(window.hwnd as *mut _),"FocusFlow generated owned content")?;
 let plan=CapturePlan{source:RecordingSource::Window{hwnd:window.hwnd.to_string(),title:window_capture.title.clone()},monitor:MonitorInfo{name:Some("generated".into()),x:bounds.x,y:bounds.y,width:bounds.width,height:bounds.height,scale_factor:1.0},bounds,backend:CaptureBackend::WindowsCapture,ffmpeg_args:vec![],window_capture:Some(window_capture)};
 let mut defaults=crate::export::ExportSettings::default().legacy_defaults()?;defaults.pan_transition_us=123456+u64::from(nonce);
 let mut reservation=StartReservation::acquire(state)?;assert!(StartReservation::acquire(state).is_err());
 let mut session=windows_capture::Session::prepare(&dir,&plan,defaults,ffmpeg,worker)?;
 assert!(!dir.join("screen.mp4").exists());assert!(!dir.join("capture.json").exists());
 if mode=="admission-close"{window.close()?;}
 if mode=="admission-resize"{window.commands.send(WindowCommand::Resize)?;thread::sleep(Duration::from_millis(150));}
 if mode=="marker-failure"{fs::create_dir(dir.join("capture.windows-v2"))?;}
 let arm=session.arm(&dir,&mut reservation);
 if ["admission-close","admission-resize","marker-failure"].contains(&mode){assert!(arm.is_err());drop(session);if mode!="marker-failure"{incomplete(&dir)?;}assert!(!dir.join("screen.mp4").exists());fs::write(dir.join("result.json"),serde_json::to_vec(&serde_json::json!({"expectedArmFailure":arm.unwrap_err().to_string()}))?)?;return Ok(());}
 arm?;assert_eq!(fs::read(dir.join("capture.windows-v2"))?,b"focusflow-windows-capture-v2\n");incomplete(&dir)?;
 let owned=windows_capture::qualification::Handles::observe(&session).map_err(|e|format!("owned observation: {e}"))?;
 fs::write(dir.join("armed"),b"real Session arm completed")?;
 drop(reservation);
 if mode=="lost-worker"{owned.kill_worker().map_err(|e|format!("kill owned worker: {e}"))?;}else if mode=="blocked"{
  // Stop after the first complete cell starts writing, before the two-entry output queue fills.
  let deadline=Instant::now()+Duration::from_secs(3);while !dir.join("first-write-started.json").is_file(){if Instant::now()>=deadline{return Err("No first writer observation".into());}thread::sleep(Duration::from_millis(1));}
 }else{thread::sleep(Duration::from_millis(1200));}
 if mode=="close"{window.close()?;}
 if mode=="resize"{window.commands.send(WindowCommand::Resize)?;}
 if mode=="close"||mode=="resize"{let end=Instant::now()+Duration::from_secs(8);while !session.ended()?{if Instant::now()>=end{return Err("Natural window end not observed before Stop".into());}thread::sleep(Duration::from_millis(5));}}
 if ["transfer-shutdown","stop-shutdown","close-stop"].contains(&mode){
  let started_at=Instant::now();
  // No input thread: lifecycle fixture supplies empty, untrusted interaction buffers.
  let active=ActiveRecording{notice:None,capture:ActiveCapture::WindowsCapture{capture:session},video_clock:video_clock::Clock::start(video_clock::Capability::probe(ffmpeg),started_at),audio_recorder:None,mic_device_name:None,click_tracker:ClickTracker{stop_requested:Arc::new(AtomicBool::new(false)),clicks:Arc::new(Mutex::new(vec![])),drags:Arc::new(Mutex::new(vec![])),join_handle:None},output_path:dir.join("screen.mp4"),started_at,monitor:plan.monitor.clone(),capture_bounds:plan.bounds,source:plan.source.clone(),ffmpeg_path:ffmpeg.to_owned(),log:Arc::new(Mutex::new(ProcessLog::default()))};
  if mode=="transfer-shutdown"{
   state.request_shutdown()?;let (error,active)=transfer_active(state,active).err().ok_or("late transfer accepted")?;assert_eq!(error.code,"recorder_shutting_down");drop(active);incomplete(&dir)?;owned.verify(&dir).map_err(|e|format!("transfer cleanup: {e}"))?;
   fs::write(dir.join("result.json"),b"{\"lateTransferRejected\":true,\"retainedIncomplete\":true}")?;return Ok(());
  }
  transfer_active(state,active).map_err(|(e,_)|e)?;
  if mode=="close-stop"{window.close()?;}
  let start=Instant::now();
  let stops=thread::scope(|scope|{
   let a=scope.spawn(||tauri::async_runtime::block_on(stop_active_recording(state)));
   let b=scope.spawn(||tauri::async_runtime::block_on(stop_active_recording(state)));
   let closing=scope.spawn(||tauri::async_runtime::block_on(shutdown(state)));
   let results=[a.join().unwrap(),b.join().unwrap()];closing.join().unwrap();results
  });
  assert!(start.elapsed()<Duration::from_secs(15));
  assert!(stops.iter().filter(|r|r.is_ok()).count()<=1);
  for error in stops.iter().filter_map(|r|r.as_ref().err()){assert!(["recorder_finalizing","recorder_not_running"].contains(&error.code.as_str()));}
  let runtime=state.lock_runtime()?;assert!(runtime.active.is_none()&&!runtime.starting&&!runtime.finalizing&&runtime.shutting_down);drop(runtime);
  let manifest=decode_windows(&fs::read(dir.join("capture.json"))?,id)?;assert!(manifest.finalized_project().is_ok());owned.verify(&dir).map_err(|e|format!("race cleanup: {e}"))?;
  fs::write(dir.join("result.json"),serde_json::to_vec(&serde_json::json!({"lifecycleFinalized":true,"competingStopResults":stops.iter().map(|r|r.as_ref().map(|_|"completed".to_owned()).unwrap_or_else(|e|e.code.clone())).collect::<Vec<_>>(),"elapsedMs":start.elapsed().as_millis()}))?)?;
  return Ok(());
 }
 let start=Instant::now();let result=session.finish();let elapsed=start.elapsed();owned.verify(&dir).map_err(|e|format!("cleanup: {e}"))?;assert!(elapsed<=Duration::from_secs(12));
 if ["blocked","partial","exit7","lost-worker"].contains(&mode){let error=result.expect_err("fault must not finalize");incomplete(&dir)?;assert!(ProjectStore::new(root)?.open_or_import(id,ffmpeg,defaults).is_err());fs::write(dir.join("result.json"),serde_json::to_vec(&serde_json::json!({"expectedFailure":error.to_string(),"finishMs":elapsed.as_millis()}))?)?;return Ok(());}
 let completion=result?;if mode=="resize"{assert!(completion.notice.as_ref().is_some_and(|s|s.contains("changed size")));}
 let n=completion.count();let grid=probe_cfr_grid(ffmpeg,&dir.join("screen.mp4"),n,&AtomicBool::new(false),Instant::now()+Duration::from_secs(10))?;
 assert_eq!((grid.video.video.width,grid.video.video.height),(w+w%2,h+h%2));
 fs::write(dir.join("before-publish"),b"complete media before real Completion publication")?;
 let hash_deadline=Instant::now()+Duration::from_secs(2);while !dir.join("media-hashed").exists(){if Instant::now()>=hash_deadline{return Err("Independent before-publication media hash missing".into());}thread::sleep(Duration::from_millis(5));}
 completion.publish(&dir,&grid,false)?;
 assert!(!dir.join("project.json").exists());let manifest=decode_windows(&fs::read(dir.join("capture.json"))?,id)?;
 assert_eq!((manifest.source.raw_size.width,manifest.source.raw_size.height),(w,h));assert_eq!((manifest.source.content_rect.x,manifest.source.content_rect.y,manifest.source.content_rect.width,manifest.source.content_rect.height),(0,0,w,h));
 if let WindowsCaptureStateV2::Finalized{video_timing,..}=&manifest.state{assert_eq!(video_timing.clock.cutoff_reason,if mode=="close"{WindowsCutoffReason::SourceClosed}else{WindowsCutoffReason::Stop});}else{panic!("missing finalized authority")}
 let snapshot=manifest.finalized_project().unwrap();let mut other=defaults;other.pan_transition_us=987654;
 let project=ProjectStore::new(root)?.open_or_import(id,ffmpeg,other)?;assert_eq!(&project,snapshot);assert_eq!(project.camera.pan_transition_us,defaults.pan_transition_us);assert!(project.clicks.is_empty()&&project.zooms.is_empty());assert_eq!(project.video.cursor,focusflow_core::project::CursorMode::Embedded);
 fs::write(dir.join("result.json"),serde_json::to_vec(&serde_json::json!({"frames":n,"finishMs":elapsed.as_millis(),"independentMediaHashGate":true,"rawWidth":w,"rawHeight":h,"nonce":nonce,"instrumented":worker==std::env::current_exe()?}))?)?;
 window.close()?;Ok(())
}
fn platform(path:&Path)->TestResult<()> {
 use ::windows::{core::Interface,Win32::{Foundation::HMODULE,Graphics::{Direct3D::*,Direct3D11::*,Dxgi::IDXGIDevice},System::{Performance::*,Threading::*,JobObjects::IsProcessInJob,RemoteDesktop::ProcessIdToSessionId,StationsAndDesktops::*,WinRT::*}}};
 let mut desktop_name=[0u16;128];let mut station_name=[0u16;128];
 unsafe{let desktop=GetThreadDesktop(GetCurrentThreadId())?;let station=GetProcessWindowStation()?;GetUserObjectInformationW(::windows::Win32::Foundation::HANDLE(desktop.0),UOI_NAME,Some(desktop_name.as_mut_ptr().cast()),256,None)?;GetUserObjectInformationW(::windows::Win32::Foundation::HANDLE(station.0),UOI_NAME,Some(station_name.as_mut_ptr().cast()),256,None)?;}
 unsafe{RoInitialize(RO_INIT_MULTITHREADED)?;}let wgc=::windows_capture::graphics_capture_api::GraphicsCaptureApi::is_supported();unsafe{RoUninitialize();}
 let mut frequency=0;let mut qpc=0;let mut session=0;let mut nested=::windows::core::BOOL(0);
 // All borrowed current-process handles and stack outputs remain valid for each native query.
 unsafe{QueryPerformanceFrequency(&mut frequency)?;QueryPerformanceCounter(&mut qpc)?;ProcessIdToSessionId(std::process::id(),&mut session)?;IsProcessInJob(GetCurrentProcess(),None,&mut nested)?;}
 let mut device=None;let mut context=None;let mut level=D3D_FEATURE_LEVEL_11_0;
 let debug=unsafe{D3D11CreateDevice(None,D3D_DRIVER_TYPE_WARP,HMODULE::default(),D3D11_CREATE_DEVICE_BGRA_SUPPORT|D3D11_CREATE_DEVICE_DEBUG,Some(&[D3D_FEATURE_LEVEL_11_0]),D3D11_SDK_VERSION,Some(&mut device),Some(&mut level),Some(&mut context))};
 let debug_status=format!("{debug:?}");let debug_available=debug.is_ok();
 if debug.is_err(){unsafe{D3D11CreateDevice(None,D3D_DRIVER_TYPE_WARP,HMODULE::default(),D3D11_CREATE_DEVICE_BGRA_SUPPORT,Some(&[D3D_FEATURE_LEVEL_11_0]),D3D11_SDK_VERSION,Some(&mut device),Some(&mut level),Some(&mut context))?;}}
 let device=device.ok_or("WARP device missing")?;let dxgi:IDXGIDevice=device.cast()?;let adapter=unsafe{dxgi.GetAdapter()?};let desc=unsafe{adapter.GetDesc()?};
 let description=String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_owned();
 fs::write(path,serde_json::to_vec_pretty(&serde_json::json!({"qpcFrequency":frequency,"qpc":qpc,"sessionId":session,"desktop":String::from_utf16_lossy(&desktop_name).trim_end_matches('\0'),"windowStation":String::from_utf16_lossy(&station_name).trim_end_matches('\0'),"wgcCapability":format!("{wgc:?}"),"runnerAlreadyInJob":nested.as_bool(),"warpAdapter":description,"vendorId":desc.VendorId,"deviceId":desc.DeviceId,"featureLevel":level.0,"debugLayerAvailable":debug_available,"debugHRESULT":debug_status,"noDebugCapabilityIsNotMappingQualification":true}))?)?;Ok(())
}
pub(crate) fn run()->TestResult<()>{
 let args:Vec<_>=std::env::args_os().skip(1).collect();if args.len()==2&&args[0]=="--platform"{return platform(Path::new(&args[1]));}if args.len()!=7{return Err("usage: harness ROOT MODE WIDTH HEIGHT NONCE FFMPEG WORKER".into());}
 let root=PathBuf::from(&args[0]);fs::create_dir(&root)?;let root=root.canonicalize()?;
 let mode=args[1].to_str().ok_or("mode")?;let w=args[2].to_str().ok_or("width")?.parse()?;let h=args[3].to_str().ok_or("height")?.parse()?;let nonce:u16=args[4].to_str().ok_or("nonce")?.parse()?;
 if !["normal","static","repeat","close","resize","admission-close","admission-resize","marker-failure","blocked","partial","exit7","lost-worker","parent-death","transfer-shutdown","stop-shutdown","close-stop"].contains(&mode){return Err("Unknown generated case".into());}
 unsafe{SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;}
 let state=RecorderState::new();for i in 0..if mode=="repeat"{3}else{1}{case(&root,&format!("session-{i}"),if mode=="repeat"{"normal"}else{mode},w,h,nonce+i,Path::new(&args[5]),Path::new(&args[6]),&state)?;assert!(!state.cleanup_runtime().starting);assert!(!state.cleanup_runtime().finalizing);}
 println!("PASS {mode}");Ok(())
}
