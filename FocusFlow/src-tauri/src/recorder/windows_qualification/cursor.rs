//! Private, generated-native observation. No capture authority, public cursor stream or user input lane.
use super::*;
use crate::recorder::windows_capture::cursor::image_native::{Icon,Bitmaps,Raster,decode_with};
use ::windows::{core::BOOL,Win32::{Foundation::POINT,Graphics::Dwm::{DwmGetWindowAttribute,DWMWA_EXTENDED_FRAME_BOUNDS},System::Threading::{GetCurrentProcess,GetGuiResources,GR_GDIOBJECTS,GR_USEROBJECTS}}};
use crate::recorder::windows_capture::qualification::{cursor_math as math,proof::Proof};

pub(in crate::recorder) fn require_disposable()->Result<(),String>{
 let get=|key:&str|std::env::var(key).map_err(|_|format!("Private cursor gate missing {key}"));
 for (key,want) in [("GITHUB_ACTIONS","true"),("RUNNER_ENVIRONMENT","github-hosted"),("RUNNER_OS","Windows"),("GITHUB_REPOSITORY","UmarAlMukhtar/FocusFlow"),("GITHUB_REF","refs/heads/codex-windows-cursor-qualification"),("GITHUB_EVENT_NAME","push"),("GITHUB_WORKFLOW_REF","UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/codex-windows-cursor-qualification")]{if get(key)?!=want{return Err(format!("Private cursor gate rejected {key}"));}}
 let sha=get("GITHUB_SHA")?;if sha.len()!=40||!sha.bytes().all(|b|b.is_ascii_hexdigit())||get("FOCUSFLOW_CURSOR_REVIEWED_SHA")?!=sha{return Err("Private cursor SHA handshake rejected".into());}
 if get("FOCUSFLOW_CURSOR_REVIEWED_REPOSITORY")?!=get("GITHUB_REPOSITORY")?||get("FOCUSFLOW_CURSOR_REVIEWED_REF")?!=get("GITHUB_REF")?{return Err("Private cursor repository/ref handshake rejected".into());}
 let id=get("GITHUB_RUN_ID")?;let attempt=get("GITHUB_RUN_ATTEMPT")?;if id.is_empty()||attempt.is_empty()||!id.bytes().all(|b|b.is_ascii_digit())||!attempt.bytes().all(|b|b.is_ascii_digit())||get("FOCUSFLOW_CURSOR_REVIEWED_RUN")?!=format!("{id}:{attempt}"){return Err("Private cursor run handshake rejected".into());}Ok(())
}
pub(super) struct DpiScope{previous:DPI_AWARENESS_CONTEXT,restored:bool}
impl DpiScope{
 pub(super) fn enter()->TestResult<Self>{require_disposable()?;Self::enter_checked()}
 fn enter_checked()->TestResult<Self>{
  // Only this observer thread changes context; the previous opaque value is restored on every exit.
  unsafe{let previous=SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);if previous.0.is_null(){return Err("PMv2 context switch failed".into());}let scope=Self{previous,restored:false};if !AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(),DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool(){return Err("Observer is not PMv2".into());}Ok(scope)}
 }
 pub(super) fn restore(&mut self)->TestResult<()>{unsafe{if SetThreadDpiAwarenessContext(self.previous).0.is_null()||!AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(),self.previous).as_bool(){return Err("DPI context restoration failed".into());}}self.restored=true;Ok(())}
}
impl Drop for DpiScope{fn drop(&mut self){if !self.restored{unsafe{SetThreadDpiAwarenessContext(self.previous);}}}}
struct Position{original:POINT,restored:bool}
impl Position{
 fn save()->TestResult<Self>{require_disposable()?;let mut p=POINT::default();unsafe{GetPhysicalCursorPos(&mut p)?;}Ok(Self{original:p,restored:false})}
 fn place(&self,p:[i32;2])->TestResult<()>{require_disposable()?;unsafe{SetPhysicalCursorPos(p[0],p[1])?;}Ok(())}
 fn restore(&mut self)->TestResult<()>{unsafe{SetPhysicalCursorPos(self.original.x,self.original.y)?;}self.restored=true;Ok(())}
}
impl Drop for Position{fn drop(&mut self){if !self.restored{unsafe{let _=SetPhysicalCursorPos(self.original.x,self.original.y);}}}}
#[derive(serde::Serialize,Clone,PartialEq,Eq)]
#[serde(rename_all="camelCase")]
struct Geometry {client_origin:[i32;2],client_size:[i32;2],window:[i32;4],dwm:[i32;4],dpi:u32,awareness:i32,pid:u32,thread:u32}
fn rect(r:RECT)->[i32;4]{[r.left,r.top,r.right,r.bottom]}
fn geometry(hwnd:HWND)->TestResult<Geometry>{
 let mut c=RECT::default();let mut w=RECT::default();let mut d=RECT::default();let mut p=POINT::default();let mut pid=0;
 // HWND is borrowed from the verified source; every output is initialized stack storage under scoped PMv2.
 unsafe{GetClientRect(hwnd,&mut c)?;GetWindowRect(hwnd,&mut w)?;DwmGetWindowAttribute(hwnd,DWMWA_EXTENDED_FRAME_BOUNDS,&mut d as *mut _ as _,std::mem::size_of::<RECT>() as u32)?;
  if !ClientToScreen(hwnd,&mut p).as_bool(){return Err("Physical client origin query failed".into());}let thread=GetWindowThreadProcessId(hwnd,Some(&mut pid));if pid==0||thread==0{return Err("Source identity unavailable".into());}
  let dpi=GetDpiForWindow(hwnd);if dpi==0{return Err("Source DPI unavailable".into());}Ok(Geometry{client_origin:[p.x,p.y],client_size:[c.right-c.left,c.bottom-c.top],window:rect(w),dwm:rect(d),dpi,awareness:GetAwarenessFromDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd)).0,pid,thread})}
}
pub(super) fn copied_shape(cursor:HCURSOR,dir:&Path,index:usize,fail_stage:u8)->TestResult<math::Shape>{
 if index>=12{return Err("Cursor diagnostic index cap".into());}
 // CURSORINFO supplies a borrowed cursor. CopyIcon owns a separate reference; only that copy is destroyed.
 let icon=Icon(unsafe{CopyIcon(HICON(cursor.0))?});if fail_stage==1{return Err("Injected failure after owned copy".into());}
 owned_shape(&icon,cursor,dir,index,fail_stage)
}
fn owned_shape(icon:&Icon,original:HCURSOR,dir:&Path,index:usize,fail_stage:u8)->TestResult<math::Shape>{
 if index>=12{return Err("Cursor diagnostic index cap".into());}
 let decoded=decode_with(icon,|stage,raw| {
  if let Some((w,h,pixels))=raw {pam(&dir.join(format!("native-color-{index}.pam")),w,h,pixels)?;}
  if fail_stage==stage {return Err(format!("Injected failure after resource stage {stage}").into());}Ok(())
 })?;
 let shape=math::Shape{width:decoded.width,height:decoded.height,hotspot:decoded.hotspot,rgba:decoded.rgba};
 // Keep failed candidates for diagnosis; only the successful caller publishes shape metadata.
 pam(&dir.join(format!("cursor-{index}.pam")),shape.width,shape.height,&shape.rgba)?;
 native_raster(HICON(original.0),&shape,dir,index)?;Ok(shape)
}
fn pam(path:&Path,w:u32,h:u32,pixels:&[u8])->TestResult<()>{if w==0||h==0||w>1920||h>1080||pixels.len()!=w as usize*h as usize*4{return Err("Invalid PAM budget".into());}let mut out=format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();out.extend_from_slice(pixels);fs::write(path,out)?;Ok(())}
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
struct Observation {q0:i64,q1:i64,before:Geometry,after:Geometry,position:[i32;2],cursor_info_position:[i32;2],flags:u32,hit:u64,hit_root:u64,hit_pid:u32,hit_thread:u32,handle:u64,consistent:bool,shape:Option<serde_json::Value>,image_error:Option<String>,telemetry_accepted:bool,reason:&'static str}
fn observe(hwnd:HWND,dir:&Path,index:usize)->TestResult<Observation>{
 let q0=observation_qpc().ok_or("QPC unavailable")?;let before=geometry(hwnd)?;let mut info=CURSORINFO{cbSize:std::mem::size_of::<CURSORINFO>() as u32,..Default::default()};let mut p=POINT::default();
 unsafe{GetCursorInfo(&mut info)?;GetPhysicalCursorPos(&mut p)?;}let hit=unsafe{WindowFromPhysicalPoint(p)};let root=unsafe{GetAncestor(hit,GA_ROOT)};let mut hit_pid=0;let hit_thread=unsafe{GetWindowThreadProcessId(hit,Some(&mut hit_pid))};
 // Only native queries and owned snapshot acquisition belong to this QPC bracket.
 let icon=unsafe{CopyIcon(HICON(info.hCursor.0))}.map(Icon);
 let mut last=CURSORINFO{cbSize:std::mem::size_of::<CURSORINFO>() as u32,..Default::default()};let mut lastp=POINT::default();unsafe{GetCursorInfo(&mut last)?;GetPhysicalCursorPos(&mut lastp)?;}let after=geometry(hwnd)?;let q1=observation_qpc().ok_or("QPC unavailable")?;
 let consistent=before==after&&p.x==info.ptScreenPos.x&&p.y==info.ptScreenPos.y&&p.x==lastp.x&&p.y==lastp.y&&info.flags==last.flags&&info.hCursor==last.hCursor&&last.ptScreenPos.x==p.x&&last.ptScreenPos.y==p.y;
 // The controlled source retains the original cursor; validation/I/O consumes the immutable owned copy after q1.
 let image=match icon{Ok(icon)=>owned_shape(&icon,info.hCursor,dir,index,0),Err(e)=>Err(e.into())};
 let(shape,image_error)=match image{Ok(s)=>{let filename=format!("cursor-{index}.pam");(Some(serde_json::json!({"path":filename,"width":s.width,"height":s.height,"hotspot":s.hotspot,"animation":"unqualified; static generated fixtures only"})),None)},Err(e)=>(None,Some(e.to_string().chars().take(256).collect()))};
 Ok(Observation{q0,q1,before,after,position:[p.x,p.y],cursor_info_position:[info.ptScreenPos.x,info.ptScreenPos.y],flags:info.flags.0,hit:hit.0 as usize as u64,hit_root:root.0 as usize as u64,hit_pid,hit_thread,handle:info.hCursor.0 as usize as u64,consistent,shape,image_error,telemetry_accepted:false,reason:"No independently defensible historical stationary boundary; all telemetry rejected"})
}
// Fixture command history is checker truth only; observe() never receives it.
pub(super) fn case(root:&Path,w:u32,h:u32,nonce:u16,ffmpeg:&Path,worker:&Path)->TestResult<()> {
 let profile=if nonce%2==0{"borderless"}else{"decorated"};
 require_disposable()?;let mut dpi=DpiScope::enter()?;let mut position=Position::save()?;
 let fixture=root.join("session-0");fs::create_dir(&fixture)?;fixture_json(&fixture.join("source-request.json"),&serde_json::json!({"width":w,"height":h,"nonce":nonce,"animated":false,"privateProfile":profile}))?;let mut window=GeneratedWindow::observed(fixture.clone())?;let hwnd=HWND(window.hwnd as *mut _);let g=geometry(hwnd)?;
 let mut physical=Vec::with_capacity(3);for [x,y] in [[8,8],[w as i32-8,8],[8,h as i32-8]]{let mut point=POINT{x,y};unsafe{if !ClientToScreen(hwnd,&mut point).as_bool(){return Err("Physical fiducial query failed".into());}}physical.push([point.x,point.y]);}
 fixture_json(&fixture.join("native-topology.json"),&topology()?)?;fixture_json(&fixture.join("resource-paths.json"),&resource_paths(&fixture)?)?;
 let alpha_probe=fixture.join("alpha-probe");fs::create_dir(&alpha_probe)?;
 // Diagnose the required alpha oracle before replaying successful capture controls.
 {let alpha=FixtureCursor::color()?;copied_shape(alpha.0,&alpha_probe,10,0)?;}
 let start_handles=unsafe{GetGuiResources(GetCurrentProcess(),GR_GDIOBJECTS)};
 let p=[g.client_origin[0]+w as i32/2,g.client_origin[1]+h as i32/2];position.place(p)?;
 require_disposable()?;
 // A hosted VM may suppress the pointer until actual mouse input reaches its desktop.
 unsafe {
  use ::windows::Win32::UI::Input::KeyboardAndMouse::*;
  let inputs=[1,-1].map(|dx|INPUT{r#type:INPUT_MOUSE,Anonymous:INPUT_0{mi:MOUSEINPUT{dx,dy:0,dwFlags:MOUSEEVENTF_MOVE,..Default::default()}}});
  if SendInput(&inputs,std::mem::size_of::<INPUT>() as i32)!=inputs.len() as u32{
   return Err("Hosted cursor wake input was rejected".into());
  }
 }
 thread::sleep(Duration::from_millis(100));
 position.place(p)?;thread::sleep(Duration::from_millis(150));
 let visibility=fixture.join("visibility");fs::create_dir(&visibility)?;let mut visibility_rows=Vec::with_capacity(3);for(i,point)in [[g.client_origin[0],g.client_origin[1]],[g.client_origin[0]+w as i32-1,g.client_origin[1]+h as i32-1],[g.client_origin[0]+w as i32+10,g.client_origin[1]+h as i32+10]].into_iter().enumerate(){position.place(point)?;thread::sleep(Duration::from_millis(40));visibility_rows.push(observe(hwnd,&visibility,i)?);}fixture_json(&visibility.join("visibility.json"),&serde_json::json!({"observations":visibility_rows,"telemetry":[],"prefix":"rejectedAll","selectedHwnd":window.hwnd}))?;position.place(p)?;thread::sleep(Duration::from_millis(100));
 for (name,with_cursor) in [("without",false),("with",true),("alpha-without",false),("alpha-with",true)]{
  if name=="alpha-without"{window.command("alpha")?;thread::sleep(Duration::from_millis(150));}
  let dir=fixture.join(name);fs::create_dir(&dir)?;let mut observations=Vec::with_capacity(3);
  let mut proof=Proof::prepare(&dir,window.hwnd as u64,g.pid,with_cursor,ffmpeg,worker)?;let raw=[proof.width,proof.height];proof.arm(&dir)?;
  for j in 0..3{observations.push(observe(hwnd,&dir,j)?);thread::sleep(Duration::from_millis(5));}
  thread::sleep(Duration::from_millis(400));proof.finish(&dir)?;
  fixture_json(&dir.join("geometry.json"),&serde_json::json!({"selectedHwnd":window.hwnd,"space":"NativeRaw fiducials versus PMv2 independently measured physical client points","physicalAnchors":physical,"clientAnchors":[[8,8],[w-8,8],[8,h-8]],"geometry":g,"rawSize":raw,"encodedSize":[raw[0]+raw[0]%2,raw[1]+raw[1]%2],"withCursor":with_cursor,"profile":profile,"observations":observations,"telemetry":[],"prefix":"rejectedAll","physicalLatencyClaim":false}))?;
 }
 window.command("restore-cursor")?;thread::sleep(Duration::from_millis(100));
 let challenges=fixture.join("challenge");fs::create_dir(&challenges)?;
 let mut proof=Proof::prepare(&challenges,window.hwnd as u64,g.pid,true,ffmpeg,worker)?;proof.arm(&challenges)?;thread::sleep(Duration::from_millis(150));let mut challenge_rows=Vec::with_capacity(10);
 for (i,action) in ["alpha","hidden","restore-cursor","occlude","unocclude","move-away-back","move","move-back","minimize-restore"].into_iter().enumerate(){window.command(action)?;thread::sleep(Duration::from_millis(40));challenge_rows.push(observe(hwnd,&challenges,i)?);}
 proof.finish(&challenges)?;fixture_json(&challenges.join("challenges.json"),&serde_json::json!({"observations":challenge_rows,"telemetry":[],"prefix":"rejectedAll","noResumeAfterEqualPolls":true,"observerInputs":"native queries only; independent source actions are checker truth"}))?;
 let resize=fixture.join("resize");fs::create_dir(&resize)?;
 let mut proof=Proof::prepare(&resize,window.hwnd as u64,g.pid,true,ffmpeg,worker)?;proof.arm(&resize)?;thread::sleep(Duration::from_millis(300));
 let live_before=observation_qpc().ok_or("Private resize live-check QPC unavailable")?;let already_ended=proof.ended()?;let live_after=observation_qpc().ok_or("Private resize live-check QPC unavailable")?;
 if already_ended{return Err("Private resize worker ended before the explicit resize".into());}
 window.resize()?;let action_sequence=window.sequence;let start=Instant::now();while !proof.ended()?{if start.elapsed()>Duration::from_secs(2){return Err("Private resize did not automatically end worker before manual Stop".into());}thread::sleep(Duration::from_millis(5));}
 let ended=observation_qpc().ok_or("Private resize ended-observation QPC unavailable")?;
 fixture_json(&resize.join("resize-observation.json"),&serde_json::json!({"actionSequence":action_sequence,"liveCheckBeforeQpc":live_before,"liveCheckAfterQpc":live_after,"workerAliveBeforeResize":true,"workerEndedBeforeFinish":true,"endedObservedQpc":ended,"manualStopRequested":false}))?;proof.finish(&resize)?;
 let closed=fixture.join("closed");fs::create_dir(&closed)?;let mut proof=Proof::prepare(&closed,window.hwnd as u64,g.pid,true,ffmpeg,worker)?;proof.arm(&closed)?;thread::sleep(Duration::from_millis(300));window.close()?;let start=Instant::now();while !proof.ended()?{if start.elapsed()>Duration::from_secs(2){return Err("Private close did not automatically end worker before manual Stop".into());}thread::sleep(Duration::from_millis(5));}proof.finish(&closed)?;fixture_json(&closed.join("close-observation.json"),&serde_json::json!({"workerEndedBeforeFinish":true,"manualStopRequested":false,"qpc":observation_qpc()}))?;
 position.restore()?;dpi.restore()?;let end_handles=unsafe{GetGuiResources(GetCurrentProcess(),GR_GDIOBJECTS)};
 fixture_json(&fixture.join("proof-result.json"),&serde_json::json!({"privateProof":true,"sourceClosed":true,"positionRestored":true,"dpiRestored":true,"showCountMutated":false,"borrowedCursorDestroyed":false,"gdiStart":start_handles,"gdiEnd":end_handles,"telemetryAccepted":0,"nativeQualification":"pending independent raw/clock/image check"}))?;if start_handles!=end_handles{return Err("Private GDI resource count changed".into());}Ok(())
}

/// Owned generated cursor, never a system-theme cursor or a borrowed CURSORINFO handle.
pub(super) struct FixtureCursor(pub HCURSOR,bool);
impl FixtureCursor {pub(super) fn new()->TestResult<Self>{require_disposable()?;let mut and=[255u8;32];let mut xor=[0u8;32];for y in 0..16{for x in 0..16{if(x<4&&y<12)||(y>8&&x<12){let i=y*2+x/8;let b=0x80>>(x%8);and[i]&=!b;if x<2{xor[i]|=b;}}}}
 // Both fixed word-aligned masks outlive the call; CreateCursor returns ownership of a new static cursor.
 unsafe{Ok(Self(CreateCursor(None,3,5,16,16,and.as_ptr().cast(),xor.as_ptr().cast())?,false))}
}}
impl Drop for FixtureCursor{fn drop(&mut self){unsafe{if self.1{let _=DestroyIcon(HICON(self.0.0));}else{let _=DestroyCursor(self.0);}}}}

fn native_raster(original:HICON,shape:&math::Shape,dir:&Path,index:usize)->TestResult<()> {
 // original is borrowed synchronously; the source/fixture retains its lifetime and ownership.
 let raster=Raster::new(shape.width,shape.height)?;let mut first_mismatch=None;
 for background in 0..4{
  let mut colors=Vec::with_capacity(shape.rgba.len());
  // The DIB memory belongs to raster until its Drop; no external bitmap is selected or accessed.
  {let pixels=unsafe{std::slice::from_raw_parts_mut(raster.bits,raster.len)};
  for (i,p) in pixels.chunks_exact_mut(4).enumerate(){let x=i%shape.width as usize;let y=i/shape.width as usize;let rgb=match background{0=>[0,0,0],1=>[255,255,255],2=>if(x/4+y/4)%2==0{[35,35,35]}else{[210,210,210]},_=>[37,91,173]};colors.push(rgb);p.copy_from_slice(&[rgb[2],rgb[1],rgb[0],255]);}}
  unsafe{DrawIconEx(raster.dc,0,0,original,shape.width as i32,shape.height as i32,0,None,DI_NORMAL)?;GdiFlush().ok()?;}
  let pixels=unsafe{std::slice::from_raw_parts(raster.bits,raster.len)};
  let mut rgba=Vec::with_capacity(raster.len);let mut mismatch=None;
  for (pixel,((got,source),bg)) in pixels.chunks_exact(4).zip(shape.rgba.chunks_exact(4)).zip(colors).enumerate(){
   for c in 0..3{
    let expected=(u32::from(source[c])*u32::from(source[3])+u32::from(bg[c])*(255-u32::from(source[3]))+127)/255;
    let actual=u32::from(got[2-c]);
    if actual.abs_diff(expected)>1&&mismatch.is_none(){mismatch=Some(format!("Native DrawIconEx mismatch: background={background}, pixel={pixel}, channel={c}, actual={actual}, expected={expected}"));}
   }
   rgba.extend_from_slice(&[got[2],got[1],got[0],255]);
  }
  pam(&dir.join(format!("raster-{index}-{background}.pam")),shape.width,shape.height,&rgba)?;
  if first_mismatch.is_none(){first_mismatch=mismatch;}
 }if let Some(error)=first_mismatch{return Err(error.into());}Ok(())
}
pub(super) fn metadata_source(dir:&Path,w:u32,h:u32,nonce:u16)->TestResult<()> {
 crate::recorder::windows_capture::qualification::startup::require_disposable()?;
 if(w,h,nonce)!=(320,240,601){return Err("Unexpected metadata source fixture".into());}let mut dpi=DpiScope::enter_checked()?;super::source(dir,w,h,nonce,false,Some("startup-metadata"))?;dpi.restore()
}
pub(super) fn source(dir:&Path,w:u32,h:u32,nonce:u16,profile:&str)->TestResult<()> {
 require_disposable()?;if !["borderless","decorated"].contains(&profile){return Err("Invalid private fixture profile".into());}let mut dpi=DpiScope::enter()?;super::source(dir,w,h,nonce,false,Some(profile))?;dpi.restore()
}

impl FixtureCursor{
 pub(super) fn color()->TestResult<Self>{require_disposable()?;let raster=Raster::new(16,16)?;
  // Owned DIB bytes are premultiplied BGRA; the asymmetric alpha shape has a nonzero hotspot.
  let bytes=unsafe{std::slice::from_raw_parts_mut(raster.bits,raster.len)};for(i,p)in bytes.chunks_exact_mut(4).enumerate(){let x=i%16;let y=i/16;let a=if x<4&&y<12{255}else if y>8&&x<12{128}else{0};p.copy_from_slice(&[0,a/2,a,a]);}
  let mask_bits=[0u8;32];let mask=unsafe{CreateBitmap(16,16,1,1,Some(mask_bits.as_ptr().cast()))};if mask.is_invalid(){return Err("Color fixture mask failed".into());}
  let info=Bitmaps(ICONINFO{fIcon:BOOL(0),xHotspot:3,yHotspot:5,hbmMask:mask,hbmColor:HBITMAP::default()});
  // CreateIconIndirect copies both bitmaps, leaving their ownership with the local guards.
  let native=ICONINFO{hbmColor:raster.bitmap,..info.0};
  // CreateIconIndirect may select its inputs temporarily; restore this owned DC before the copy.
  unsafe{if SelectObject(raster.dc,raster.previous).is_invalid(){return Err("Color fixture bitmap deselection failed".into());}}let icon=unsafe{CreateIconIndirect(&native)?};Ok(Self(HCURSOR(icon.0),true))
 }
}
fn resource_paths(dir:&Path)->TestResult<serde_json::Value>{
 let alpha=FixtureCursor::color()?;let inverted=FixtureCursor::xor()?;for stage in 1..=5{let _=copied_shape(alpha.0,dir,0,stage);}let count=||unsafe{[GetGuiResources(GetCurrentProcess(),GR_GDIOBJECTS),GetGuiResources(GetCurrentProcess(),GR_USEROBJECTS)]};let before=count();
 for _ in 0..16{for stage in 1..=5{let error=copied_shape(alpha.0,dir,0,stage).err().ok_or("Injected resource path did not fail")?;if !error.to_string().starts_with("Injected failure after"){return Err(error);}}if copied_shape(inverted.0,dir,0,0).err().is_none_or(|e|!e.to_string().contains("Destination-dependent XOR")){return Err("Native XOR cursor was not rejected explicitly".into());}if copied_shape(HCURSOR::default(),dir,0,0).is_ok(){return Err("Invalid borrowed cursor copy succeeded".into());}}
 let after=count();if before!=after{return Err("Copy/GetIconInfo failure path leaked native resources".into());}Ok(serde_json::json!({"iterations":16,"copyFailure":true,"afterCopyFailure":true,"afterIconInfoFailure":true,"afterDibitsFailure":true,"afterColorRasterFailure":true,"afterColorDrawFailure":true,"nativeXorRejected":true,"before":before,"after":after,"partialAcquisitionsOwned":true}))
}
fn topology()->TestResult<serde_json::Value>{
 unsafe extern "system" fn monitor(h:HMONITOR,_:HDC,_:*mut RECT,data:LPARAM)->BOOL{
  // Enumeration is synchronous; data points to this call's bounded Vec for the callback lifetime.
  let out=unsafe{&mut*(data.0 as *mut Vec<serde_json::Value>)};if out.len()>=16{return BOOL(0);}let mut info=MONITORINFO{cbSize:std::mem::size_of::<MONITORINFO>() as u32,..Default::default()};let(mut x,mut y)=(0,0);
  unsafe{if !GetMonitorInfoW(h,&mut info).as_bool(){return BOOL(0);}if GetDpiForMonitor(h,MDT_EFFECTIVE_DPI,&mut x,&mut y).is_err(){return BOOL(0);}}
  out.push(serde_json::json!({"bounds":rect(info.rcMonitor),"dpi":[x,y],"primary":info.dwFlags&MONITORINFOF_PRIMARY!=0}));BOOL(1)
 }
 let mut monitors=Vec::<serde_json::Value>::with_capacity(16);unsafe{if !EnumDisplayMonitors(None,None,Some(monitor),LPARAM(&mut monitors as *mut _ as isize)).as_bool(){return Err("Actual monitor inventory failed or exceeded 16 monitor cap".into());}}
 if monitors.is_empty(){return Err("No actual display available".into());}Ok(serde_json::json!({"actualMonitors":monitors,"observer":"PMv2 physical coordinates","profileAwareness":"PMv2 only","dpi120":"UNAVAILABLE unless actual inventory contains it; not exercised","dpi144":"UNAVAILABLE unless actual inventory contains it; not exercised","dpi192":"UNAVAILABLE unless actual inventory contains it; not exercised","mixedDpi":"UNAVAILABLE; no cross-monitor profile executed","negativeOrigin":"UNAVAILABLE; signed arithmetic is portable evidence only","nativeHandleReuse":"UNAVAILABLE; allocation reuse not observed","animation":"UNAVAILABLE; static generated cursor fixtures only","suppressedCursor":"UNAVAILABLE; not synthesized"}))
}

impl FixtureCursor {fn xor()->TestResult<Self>{require_disposable()?;let and=[255u8;32];let mut xor=[0u8;32];xor[0]=0x80;
 // A single destination-inversion pixel is intentional; the reader must reject its whole native image.
 unsafe{Ok(Self(CreateCursor(None,3,5,16,16,and.as_ptr().cast(),xor.as_ptr().cast())?,false))}
}}


fn static_snapshot(
    hwnd: u64,
    pid: u32,
    size: [u32; 2],
    require_pointer: bool,
) -> TestResult<serde_json::Value> {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    require_disposable()?;
    let inventory = topology()?;
    let monitors = inventory["actualMonitors"]
        .as_array()
        .ok_or("Static monitor inventory missing")?;
    if monitors.len() != 1
        || monitors[0]["bounds"][0] != 0
        || monitors[0]["bounds"][1] != 0
        || monitors[0]["dpi"] != serde_json::json!([96, 96])
    {
        return Err("Static placement requires one origin-zero 96DPI monitor".into());
    }
    let target = HWND(hwnd as usize as *mut _);
    let g = geometry(target)?;
    // All handles are borrowed from the exact generated source; calls only query its current state.
    unsafe {
        if hwnd == 0
            || pid == 0
            || pid == std::process::id()
            || g.pid != pid
            || g.thread == 0
            || g.client_size != size.map(|v| v as i32)
            || g.dpi != 96
            || !IsWindow(Some(target)).as_bool()
            || !IsWindowVisible(target).as_bool()
            || IsIconic(target).as_bool()
            || GetAncestor(target, GA_ROOT) != target
            || !AreDpiAwarenessContextsEqual(
                GetWindowDpiAwarenessContext(target),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
            .as_bool()
        {
            return Err("Static source identity/geometry/visibility rejected".into());
        }
        let point = static_policy::center(
            g.client_origin,
            size,
            GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0,
            GetAsyncKeyState(i32::from(VK_RBUTTON.0)) < 0,
        )?;
        let hit = WindowFromPhysicalPoint(POINT {
            x: point[0],
            y: point[1],
        });
        let mut hit_pid = 0;
        let tid = GetWindowThreadProcessId(hit, Some(&mut hit_pid));
        if GetAncestor(hit, GA_ROOT) != target || hit_pid != pid || tid != g.thread {
            return Err("Static neutral point is not owned by source".into());
        }
        let mut actual = POINT::default();
        GetPhysicalCursorPos(&mut actual)?;
        let mut info = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        GetCursorInfo(&mut info)?;
        if require_pointer
            && ([actual.x, actual.y] != point
                || [info.ptScreenPos.x, info.ptScreenPos.y] != point
                || info.flags != CURSOR_SHOWING
                || info.hCursor.0.is_null())
        {
            return Err("Static pointer is not visible at owned center".into());
        }
        Ok(
            serde_json::json!({"hwnd":hwnd,"sourcePid":pid,"clientSize":size,"geometry":g,"monitors":monitors,"position":point,"cursorHandle":info.hCursor.0 as usize as u64,"cursorFlags":info.flags.0,"buttonsUp":true,"footprint":[point[0]-3,point[1]-5,16,16],"margin":2}),
        )
    }
}

pub(super) fn place_owned_static(
    hwnd: u64,
    pid: u32,
    client_size: [u32; 2],
) -> TestResult<serde_json::Value> {
    use ::windows::Win32::UI::Input::KeyboardAndMouse::*;
    require_disposable()?;
    let mut dpi = DpiScope::enter()?;
    let before = static_snapshot(hwnd, pid, client_size, false)?;
    let p = before["position"]
        .as_array()
        .ok_or("Static point missing")?;
    let point = [
        i32::try_from(p[0].as_i64().ok_or("Static X missing")?)?,
        i32::try_from(p[1].as_i64().ok_or("Static Y missing")?)?,
    ];
    let mut operations = Vec::with_capacity(4);
    let q0 = observation_qpc().ok_or("Static placement QPC unavailable")?;
    // Only the validated generated neutral canvas is touched; no unrelated pointer restoration.
    unsafe {
        SetPhysicalCursorPos(point[0], point[1])?;
    }
    operations.push(serde_json::json!({"operation":"place-center","q0":q0,"q1":observation_qpc().ok_or("Static placement QPC unavailable")?}));
    for dx in [1, -1] {
        let check = static_snapshot(hwnd, pid, client_size, false)?;
        if check["geometry"] != before["geometry"] || check["monitors"] != before["monitors"] {
            return Err("Static geometry changed before wake".into());
        }
        let mut actual = POINT::default();
        unsafe {
            GetPhysicalCursorPos(&mut actual)?;
        }
        let hit = unsafe { WindowFromPhysicalPoint(actual) };
        let mut hit_pid = 0;
        let tid = unsafe { GetWindowThreadProcessId(hit, Some(&mut hit_pid)) };
        if actual.y != point[1]
            || ![point[0], point[0] + 1].contains(&actual.x)
            || unsafe { GetAncestor(hit, GA_ROOT) } != HWND(hwnd as usize as *mut _)
            || hit_pid != pid
            || u64::from(tid)
                != before["geometry"]["thread"]
                    .as_u64()
                    .ok_or("Static thread missing")?
        {
            return Err("Pointer left owned wake footprint".into());
        }
        let q0 = observation_qpc().ok_or("Static wake QPC unavailable")?;
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    ..Default::default()
                },
            },
        };
        // Fixed relative wake only; each call is preceded by the current owned point/buttons guard.
        if unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) } != 1 {
            return Err("Static hosted wake input rejected".into());
        }
        operations.push(serde_json::json!({"operation":"wake-horizontal","dx":dx,"q0":q0,"q1":observation_qpc().ok_or("Static wake QPC unavailable")?}));
    }
    thread::sleep(Duration::from_millis(100));
    let check = static_snapshot(hwnd, pid, client_size, false)?;
    if check["geometry"] != before["geometry"] || check["monitors"] != before["monitors"] {
        return Err("Static geometry changed before settle".into());
    }
    let q0 = observation_qpc().ok_or("Static settle QPC unavailable")?;
    unsafe {
        SetPhysicalCursorPos(point[0], point[1])?;
    }
    thread::sleep(Duration::from_millis(150));
    let mut receipt = static_snapshot(hwnd, pid, client_size, true)?;
    if receipt["geometry"] != before["geometry"] || receipt["monitors"] != before["monitors"] {
        return Err("Static source moved during placement".into());
    }
    operations.push(serde_json::json!({"operation":"settle-center","q0":q0,"q1":observation_qpc().ok_or("Static settle QPC unavailable")?}));
    receipt["operations"] = serde_json::json!(operations);
    receipt["wakeMs"] = 100.into();
    receipt["settleMs"] = 150.into();
    if serde_json::to_vec(&receipt)?.len() > 4096 {
        return Err("Static placement receipt exceeds cap".into());
    }
    dpi.restore()?;
    Ok(receipt)
}

pub(super) fn verify_owned_static(
    hwnd: u64,
    pid: u32,
    size: [u32; 2],
    placement: &serde_json::Value,
) -> TestResult<()> {
    let current = static_snapshot(hwnd, pid, size, true)?;
    for key in [
        "hwnd",
        "sourcePid",
        "clientSize",
        "geometry",
        "monitors",
        "position",
        "cursorHandle",
        "cursorFlags",
        "buttonsUp",
        "footprint",
        "margin",
    ] {
        if current[key] != placement[key] {
            return Err(format!("Static source/pointer changed: {key}").into());
        }
    }
    Ok(())
}

mod static_policy {
    pub(super) fn center(
        origin: [i32; 2],
        size: [u32; 2],
        left: bool,
        right: bool,
    ) -> Result<[i32; 2], String> {
        if left || right || size[0] == 0 || size[1] == 0 || size[0] > 1920 || size[1] > 1080 {
            return Err("Static placement needs buttons up and bounded client".into());
        }
        let [w, h] = size.map(i64::from);
        let [x, y] = [w / 2, h / 2];
        // Cursor hotspot [3,5], 16x16 footprint, plus two pixels on every side.
        let area = [x - 5, y - 7, x + 15, y + 13];
        let reserved = [
            [0, 0, 16, 16],
            [w - 16, 0, w, 16],
            [0, h - 16, 16, h],
            [w - 16, h - 16, w, h],
            [16, 40, 144, 96],
        ];
        if area[0] < 0
            || area[1] < 0
            || area[2] > w
            || area[3] > h
            || reserved
                .iter()
                .any(|r| area[0] < r[2] && r[0] < area[2] && area[1] < r[3] && r[1] < area[3])
        {
            return Err("Static center footprint is outside neutral canvas".into());
        }
        if i64::from(origin[0]) + area[0] < i64::from(i32::MIN)
            || i64::from(origin[1]) + area[1] < i64::from(i32::MIN)
            || i64::from(origin[0]) + area[2] > i64::from(i32::MAX)
            || i64::from(origin[1]) + area[3] > i64::from(i32::MAX)
        {
            return Err("Static footprint physical overflow".into());
        }
        Ok([
            i32::try_from(i64::from(origin[0]) + x).map_err(|_| "Static center overflow")?,
            i32::try_from(i64::from(origin[1]) + y).map_err(|_| "Static center overflow")?,
        ])
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn center_uses_physical_client_origin_and_neutral_footprint() {
            assert_eq!(
                center([100, 100], [320, 240], false, false).unwrap(),
                [260, 220]
            );
            assert_eq!(
                center([108, 131], [641, 359], false, false).unwrap(),
                [428, 310]
            );
            assert!(center([0, 0], [160, 120], false, false).is_err());
            assert!(center([i32::MAX, 0], [320, 240], false, false).is_err());
            assert!(center([i32::MAX - 160, 0], [320, 240], false, false).is_err());
            assert!(center([0, 0], [0, 240], false, false).is_err());
        }
        #[test]
        fn placement_rejects_either_down_button() {
            for (left, right) in [(true, false), (false, true), (true, true)] {
                assert!(center([100, 100], [320, 240], left, right).is_err());
            }
        }
    }
}
