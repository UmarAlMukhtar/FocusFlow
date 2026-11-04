//! Private, generated-native observation. No capture authority, public cursor stream or user input lane.
use super::*;
use ::windows::{core::BOOL,Win32::{Foundation::POINT,Graphics::Dwm::{DwmGetWindowAttribute,DWMWA_EXTENDED_FRAME_BOUNDS},System::Threading::{GetCurrentProcess,GetGuiResources,GR_GDIOBJECTS,GR_USEROBJECTS}}};
use crate::recorder::windows_capture::qualification::{cursor_math as math,proof::Proof};

pub(in crate::recorder) fn require_disposable()->Result<(),String>{
 let get=|key:&str|std::env::var(key).map_err(|_|format!("Private cursor gate missing {key}"));
 for (key,want) in [("GITHUB_ACTIONS","true"),("RUNNER_ENVIRONMENT","github-hosted"),("RUNNER_OS","Windows"),("GITHUB_REPOSITORY","UmarAlMukhtar/FocusFlow"),("GITHUB_REF","refs/heads/codex-windows-cursor-qualification"),("GITHUB_EVENT_NAME","push"),("GITHUB_WORKFLOW_REF","UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/codex-windows-cursor-qualification")]{if get(key)?!=want{return Err(format!("Private cursor gate rejected {key}"));}}
 let sha=get("GITHUB_SHA")?;if sha.len()!=40||!sha.bytes().all(|b|b.is_ascii_hexdigit())||get("FOCUSFLOW_CURSOR_REVIEWED_SHA")?!=sha{return Err("Private cursor SHA handshake rejected".into());}
 if get("FOCUSFLOW_CURSOR_REVIEWED_REPOSITORY")?!=get("GITHUB_REPOSITORY")?||get("FOCUSFLOW_CURSOR_REVIEWED_REF")?!=get("GITHUB_REF")?{return Err("Private cursor repository/ref handshake rejected".into());}
 let id=get("GITHUB_RUN_ID")?;let attempt=get("GITHUB_RUN_ATTEMPT")?;if id.is_empty()||attempt.is_empty()||!id.bytes().all(|b|b.is_ascii_digit())||!attempt.bytes().all(|b|b.is_ascii_digit())||get("FOCUSFLOW_CURSOR_REVIEWED_RUN")?!=format!("{id}:{attempt}"){return Err("Private cursor run handshake rejected".into());}Ok(())
}
struct DpiScope{previous:DPI_AWARENESS_CONTEXT,restored:bool}
impl DpiScope{
 fn enter()->TestResult<Self>{require_disposable()?;
  // Only this observer thread changes context; the previous opaque value is restored on every exit.
  unsafe{let previous=SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);if previous.0.is_null(){return Err("PMv2 context switch failed".into());}let scope=Self{previous,restored:false};if !AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(),DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).as_bool(){return Err("Observer is not PMv2".into());}Ok(scope)}
 }
 fn restore(&mut self)->TestResult<()>{unsafe{if SetThreadDpiAwarenessContext(self.previous).0.is_null()||!AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(),self.previous).as_bool(){return Err("DPI context restoration failed".into());}}self.restored=true;Ok(())}
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
struct Icon(HICON);impl Drop for Icon{fn drop(&mut self){unsafe{let _=DestroyIcon(self.0);}}}
struct Bitmaps(ICONINFO);impl Drop for Bitmaps{fn drop(&mut self){unsafe{if !self.0.hbmColor.is_invalid(){let _=DeleteObject(self.0.hbmColor.into());}if !self.0.hbmMask.is_invalid(){let _=DeleteObject(self.0.hbmMask.into());}}}}
struct ScreenDc(HDC);impl Drop for ScreenDc{fn drop(&mut self){unsafe{ReleaseDC(None,self.0);}}}
fn bitmap(bitmap:HBITMAP)->TestResult<(u32,u32,Vec<u8>)>{
 let mut object=BITMAP::default();
 // GetIconInfo bitmaps are owned, unselected, and remain alive throughout GetObject/GetDIBits.
 unsafe{if GetObjectW(bitmap.into(),std::mem::size_of::<BITMAP>() as i32,Some(&mut object as *mut _ as _))!=std::mem::size_of::<BITMAP>() as i32{return Err("GetObject bitmap read incomplete".into());}}
 let(w,h)=(u32::try_from(object.bmWidth)?,u32::try_from(object.bmHeight)?);if w==0||h==0||w>512||h>1024{return Err("Cursor bitmap outside allocation cap".into());}
 let mut info=BITMAPINFO{bmiHeader:BITMAPINFOHEADER{biSize:std::mem::size_of::<BITMAPINFOHEADER>() as u32,biWidth:w as i32,biHeight:-(h as i32),biPlanes:1,biBitCount:32,biCompression:BI_RGB.0,..Default::default()},..Default::default()};
 let dc=ScreenDc(unsafe{GetDC(None)});if dc.0.is_invalid(){return Err("GetDC failed".into());}let mut pixels=vec![0;w as usize*h as usize*4];
 // Top-down 32bpp rows have width*4 stride; a partial scan count is a failure, never a usable image.
 let lines=unsafe{GetDIBits(dc.0,bitmap,0,h,Some(pixels.as_mut_ptr().cast()),&mut info,DIB_RGB_COLORS)};if lines!=h as i32{return Err("GetDIBits scan count incomplete".into());}Ok((w,h,pixels))
}
fn copied_shape(cursor:HCURSOR,dir:&Path,index:usize,fail_stage:u8)->TestResult<math::Shape>{
 // CURSORINFO supplies a borrowed cursor. CopyIcon owns a separate reference; only that copy is destroyed.
 let icon=Icon(unsafe{CopyIcon(HICON(cursor.0))?});if fail_stage==1{return Err("Injected failure after owned copy".into());}let mut info=Bitmaps(ICONINFO::default());let result=unsafe{GetIconInfo(icon.0,&mut info.0)};result?;if fail_stage==2{return Err("Injected failure after owned GetIconInfo bitmaps".into());}
 if info.0.fIcon.as_bool()||info.0.hbmMask.is_invalid(){return Err("Not a cursor or missing mask".into());}let hotspot=[info.0.xHotspot,info.0.yHotspot];let(mw,mh,mask)=bitmap(info.0.hbmMask)?;if fail_stage==3{return Err("Injected failure after complete mask DIB read".into());}
 let shape=if info.0.hbmColor.is_invalid(){if mh%2!=0{return Err("Odd monochrome mask height".into());}let half=mh as usize/2*mw as usize;let bits:Vec<bool>=mask.chunks_exact(4).map(|p|p[0]!=0||p[1]!=0||p[2]!=0).collect();math::monochrome(mw,mh/2,hotspot,&bits[..half],&bits[half..])?}else{
  let(w,h,bgra)=bitmap(info.0.hbmColor)?;if(w,h)!=(mw,mh){return Err("Mask/color dimensions disagree".into());}math::color(w,h,hotspot,&bgra)?
 };native_raster(icon.0,&shape,dir,index)?;Ok(shape)
}
fn pam(path:&Path,w:u32,h:u32,pixels:&[u8])->TestResult<()>{if w==0||h==0||w>1920||h>1080||pixels.len()!=w as usize*h as usize*4{return Err("Invalid PAM budget".into());}let mut out=format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n").into_bytes();out.extend_from_slice(pixels);fs::write(path,out)?;Ok(())}
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
struct Observation {q0:i64,q1:i64,before:Geometry,after:Geometry,position:[i32;2],cursor_info_position:[i32;2],flags:u32,hit:u64,hit_root:u64,hit_pid:u32,hit_thread:u32,handle:u64,consistent:bool,shape:Option<serde_json::Value>,image_error:Option<String>,telemetry_accepted:bool,reason:&'static str}
fn observe(hwnd:HWND,dir:&Path,index:usize)->TestResult<Observation>{
 let q0=observation_qpc().ok_or("QPC unavailable")?;let before=geometry(hwnd)?;let mut info=CURSORINFO{cbSize:std::mem::size_of::<CURSORINFO>() as u32,..Default::default()};let mut p=POINT::default();
 unsafe{GetCursorInfo(&mut info)?;GetPhysicalCursorPos(&mut p)?;}let hit=unsafe{WindowFromPhysicalPoint(p)};let root=unsafe{GetAncestor(hit,GA_ROOT)};let mut hit_pid=0;let hit_thread=unsafe{GetWindowThreadProcessId(hit,Some(&mut hit_pid))};
 let image=copied_shape(info.hCursor,dir,index,0);let(shape,image_error)=match image{Ok(s)=>{let filename=format!("cursor-{index}.pam");pam(&dir.join(&filename),s.width,s.height,&s.rgba)?;(Some(serde_json::json!({"path":filename,"width":s.width,"height":s.height,"hotspot":s.hotspot,"animation":"unqualified; static generated fixtures only"})),None)},Err(e)=>(None,Some(e.to_string().chars().take(256).collect()))};
 let mut last=CURSORINFO{cbSize:std::mem::size_of::<CURSORINFO>() as u32,..Default::default()};let mut lastp=POINT::default();unsafe{GetCursorInfo(&mut last)?;GetPhysicalCursorPos(&mut lastp)?;}let after=geometry(hwnd)?;let q1=observation_qpc().ok_or("QPC unavailable")?;
 let consistent=before==after&&p.x==info.ptScreenPos.x&&p.y==info.ptScreenPos.y&&p.x==lastp.x&&p.y==lastp.y&&info.flags==last.flags&&info.hCursor==last.hCursor&&last.ptScreenPos.x==p.x&&last.ptScreenPos.y==p.y;
 Ok(Observation{q0,q1,before,after,position:[p.x,p.y],cursor_info_position:[info.ptScreenPos.x,info.ptScreenPos.y],flags:info.flags.0,hit:hit.0 as usize as u64,hit_root:root.0 as usize as u64,hit_pid,hit_thread,handle:info.hCursor.0 as usize as u64,consistent,shape,image_error,telemetry_accepted:false,reason:"No independently defensible historical stationary boundary; all telemetry rejected"})
}
// Fixture command history is checker truth only; observe() never receives it.
pub(super) fn case(root:&Path,w:u32,h:u32,nonce:u16,ffmpeg:&Path,worker:&Path)->TestResult<()> {
 let profile=if nonce%2==0{"borderless"}else{"decorated"};
 require_disposable()?;let mut dpi=DpiScope::enter()?;let mut position=Position::save()?;
 let fixture=root.join("session-0");fs::create_dir(&fixture)?;fixture_json(&fixture.join("source-request.json"),&serde_json::json!({"width":w,"height":h,"nonce":nonce,"animated":false,"privateProfile":profile}))?;let mut window=GeneratedWindow::observed(fixture.clone())?;let hwnd=HWND(window.hwnd as *mut _);let g=geometry(hwnd)?;
 let mut physical=Vec::with_capacity(3);for [x,y] in [[8,8],[w as i32-8,8],[8,h as i32-8]]{let mut point=POINT{x,y};unsafe{if !ClientToScreen(hwnd,&mut point).as_bool(){return Err("Physical fiducial query failed".into());}}physical.push([point.x,point.y]);}
 fixture_json(&fixture.join("native-topology.json"),&topology()?)?;fixture_json(&fixture.join("resource-paths.json"),&resource_paths(&fixture)?)?;let mut observations=Vec::with_capacity(8);let start_handles=unsafe{GetGuiResources(GetCurrentProcess(),GR_GDIOBJECTS)};
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
 for (i,with_cursor) in [false,true].into_iter().enumerate(){
  let dir=fixture.join(if with_cursor{"with"}else{"without"});fs::create_dir(&dir)?;
  let mut proof=Proof::prepare(&dir,window.hwnd as u64,g.pid,with_cursor,ffmpeg,worker)?;let raw=[proof.width,proof.height];proof.arm(&dir)?;
  for j in 0..3{observations.push(observe(hwnd,&dir,j)?);thread::sleep(Duration::from_millis(5));}
  thread::sleep(Duration::from_millis(400));proof.finish(&dir)?;
  fixture_json(&dir.join("geometry.json"),&serde_json::json!({"selectedHwnd":window.hwnd,"space":"NativeRaw fiducials versus PMv2 independently measured physical client points","physicalAnchors":physical,"clientAnchors":[[8,8],[w-8,8],[8,h-8]],"geometry":g,"rawSize":raw,"encodedSize":[raw[0]+raw[0]%2,raw[1]+raw[1]%2],"withCursor":with_cursor,"profile":profile,"observations":&observations[i*3..],"telemetry":[],"prefix":"rejectedAll","physicalLatencyClaim":false}))?;
 }
 let challenges=fixture.join("challenge");fs::create_dir(&challenges)?;let mut proof=Proof::prepare(&challenges,window.hwnd as u64,g.pid,true,ffmpeg,worker)?;proof.arm(&challenges)?;thread::sleep(Duration::from_millis(150));let mut challenge_rows=Vec::with_capacity(10);
 for (i,action) in ["alpha","hidden","restore-cursor","occlude","unocclude","move-away-back","move","move-back","minimize-restore"].into_iter().enumerate(){window.command(action)?;thread::sleep(Duration::from_millis(40));challenge_rows.push(observe(hwnd,&challenges,i)?);}
 window.resize()?;thread::sleep(Duration::from_millis(100));proof.finish(&challenges)?;fixture_json(&challenges.join("challenges.json"),&serde_json::json!({"observations":challenge_rows,"telemetry":[],"prefix":"rejectedAll","noResumeAfterEqualPolls":true,"observerInputs":"native queries only; independent source actions are checker truth"}))?;
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

struct Raster{dc:HDC,bitmap:HBITMAP,previous:HGDIOBJ,bits:*mut u8,len:usize}
impl Drop for Raster{fn drop(&mut self){unsafe{SelectObject(self.dc,self.previous);let _=DeleteObject(self.bitmap.into());let _=DeleteDC(self.dc);}}}
impl Raster{
 fn new(w:u32,h:u32)->TestResult<Self>{
  if w==0||h==0||w>512||h>512{return Err("Raster allocation cap".into());}
  // DC and DIB are owned; each failure releases every successfully acquired predecessor.
  unsafe{let dc=CreateCompatibleDC(None);if dc.is_invalid(){return Err("Raster DC failed".into());}let info=BITMAPINFO{bmiHeader:BITMAPINFOHEADER{biSize:std::mem::size_of::<BITMAPINFOHEADER>() as u32,biWidth:w as i32,biHeight:-(h as i32),biPlanes:1,biBitCount:32,biCompression:BI_RGB.0,..Default::default()},..Default::default()};let mut bits=std::ptr::null_mut();let bitmap=match CreateDIBSection(Some(dc),&info,DIB_RGB_COLORS,&mut bits,None,0){Ok(b)=>b,Err(e)=>{let _=DeleteDC(dc);return Err(e.into());}};
   let previous=SelectObject(dc,bitmap.into());if previous.is_invalid()||bits.is_null(){let _=DeleteObject(bitmap.into());let _=DeleteDC(dc);return Err("Raster selection failed".into());}Ok(Self{dc,bitmap,previous,bits:bits.cast(),len:w as usize*h as usize*4})}
 }
}
fn native_raster(icon:HICON,shape:&math::Shape,dir:&Path,index:usize)->TestResult<()> {
 let raster=Raster::new(shape.width,shape.height)?;
 for background in 0..4{
  let mut colors=Vec::with_capacity(shape.rgba.len());
  // The DIB memory belongs to raster until its Drop; no external bitmap is selected or accessed.
  let pixels=unsafe{std::slice::from_raw_parts_mut(raster.bits,raster.len)};
  for (i,p) in pixels.chunks_exact_mut(4).enumerate(){let x=i%shape.width as usize;let y=i/shape.width as usize;let rgb=match background{0=>[0,0,0],1=>[255,255,255],2=>if(x/4+y/4)%2==0{[35,35,35]}else{[210,210,210]},_=>[37,91,173]};colors.push(rgb);p.copy_from_slice(&[rgb[2],rgb[1],rgb[0],255]);}
  unsafe{DrawIconEx(raster.dc,0,0,icon,shape.width as i32,shape.height as i32,0,None,DI_NORMAL)?;GdiFlush().ok()?;}
  let mut rgba=Vec::with_capacity(raster.len);
  for ((got,source),bg) in pixels.chunks_exact(4).zip(shape.rgba.chunks_exact(4)).zip(colors){for c in 0..3{let expected=(u32::from(source[c])*u32::from(source[3])+u32::from(bg[c])*(255-u32::from(source[3]))+127)/255;let actual=u32::from(got[2-c]);if actual.abs_diff(expected)>1{return Err("Native DrawIconEx differs from straight RGBA by more than 1 channel value".into());}}rgba.extend_from_slice(&[got[2],got[1],got[0],255]);}
  pam(&dir.join(format!("raster-{index}-{background}.pam")),shape.width,shape.height,&rgba)?;
 }Ok(())
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
 let cursor=FixtureCursor::new()?;let inverted=FixtureCursor::xor()?;for stage in [1,2,3]{let _=copied_shape(cursor.0,dir,0,stage);}let count=||unsafe{[GetGuiResources(GetCurrentProcess(),GR_GDIOBJECTS),GetGuiResources(GetCurrentProcess(),GR_USEROBJECTS)]};let before=count();
 for _ in 0..16{for stage in [1,2,3]{if copied_shape(cursor.0,dir,0,stage).is_ok(){return Err("Injected resource path did not fail".into());}}if copied_shape(inverted.0,dir,0,0).err().is_none_or(|e|!e.to_string().contains("Destination-dependent XOR")){return Err("Native XOR cursor was not rejected explicitly".into());}if copied_shape(HCURSOR::default(),dir,0,0).is_ok(){return Err("Invalid borrowed cursor copy succeeded".into());}}
 let after=count();if before!=after{return Err("Copy/GetIconInfo failure path leaked native resources".into());}Ok(serde_json::json!({"iterations":16,"copyFailure":true,"afterCopyFailure":true,"afterIconInfoFailure":true,"afterDibitsFailure":true,"nativeXorRejected":true,"before":before,"after":after,"partialAcquisitionsOwned":true}))
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
