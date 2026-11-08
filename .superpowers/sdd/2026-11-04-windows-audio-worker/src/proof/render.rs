//! Bounded deterministic fixture. This generator is not a source of audio capture authority.
use crate::{format::{Encoding,Format},Result};
pub const SECONDS:u32=12;
pub const BLOCK_FRAMES:u32=1024;
const BURSTS_MS:[(u32,u32);6]=[(300,757),(1100,1831),(2345,3602),(4371,5124),(6237,8116),(9031,10893)];
pub struct Signal {format:Format,seed:u32,frame:u32}
impl Signal {
 pub fn new(format:Format,seed:u32)->Result<Self> {Ok(Self{format:format.validate()?,seed,frame:0})}
 pub fn remaining(&self)->u32 {self.format.rate*SECONDS-self.frame}
 pub fn block(&mut self,frames:u32)->Result<Vec<u8>> {
  if frames==0 || frames>BLOCK_FRAMES {return Err("Fixture block must contain 1..1024 frames".into());}
  let count=frames.min(self.remaining());let mut bytes=Vec::with_capacity(self.format.byte_len(count)?);
  for frame in self.frame..self.frame+count {
   let active=BURSTS_MS.iter().any(|&(start,end)|u64::from(frame)*1000>=u64::from(start)*u64::from(self.format.rate)&&u64::from(frame)*1000<u64::from(end)*u64::from(self.format.rate));
   for channel in 0..self.format.channels {
    let mut x=self.seed^frame.wrapping_mul(0x9e3779b9)^u32::from(channel).wrapping_mul(0x85ebca6b);
    x^=x<<13;x^=x>>17;x^=x<<5;
    let sample=if active {((x>>16) as i32-32768)/4} else {0};
    match (self.format.encoding,self.format.bits) {
     (Encoding::Pcm,16)=>bytes.extend_from_slice(&(sample as i16).to_le_bytes()),
     (Encoding::Pcm,24)=>bytes.extend_from_slice(&(sample<<8).to_le_bytes()[..3]),
     (Encoding::Pcm,32)=>bytes.extend_from_slice(&(sample<<16).to_le_bytes()),
     (Encoding::Float,32)=>bytes.extend_from_slice(&(sample as f32/32768.0).to_le_bytes()),
     _=>return Err("Unsupported fixture encoding".into()),
    }
   }
  }
  self.frame+=count;Ok(bytes)
 }
}

#[cfg(any(windows,test))]
use crate::{worker::Control,protocol::{ValidatedInit,ClockStamp,EventV1,Event,Owner,TerminalSummary}};
#[cfg(any(windows,test))]
use std::time::{Instant,Duration};
#[cfg(any(windows,test))]
trait RenderOps {
 fn control(&mut self)->Result<Control>;
 fn open(&mut self,id:&str)->Result<Format>;
 fn start(&mut self)->Result<()>;
 fn next(&mut self)->Result<bool>;
 fn wait(&mut self)->Result<()>;
 /// Returning guarantees every renderer COM reference/apartment has been dropped on this thread.
 fn stop_and_drop(&mut self)->Result<()>;
 fn stamp(&mut self)->Result<ClockStamp>;
 fn emit(&mut self,event:&EventV1)->Result<()>;
 fn now(&self)->Instant {Instant::now()}
}
#[cfg(any(windows,test))]
fn run_renderer(ops:&mut impl RenderOps,init:ValidatedInit)->i32 {
 let now=ops.now();let mut owner=match Owner::new(now+Duration::from_millis(crate::protocol::STARTUP_MS),init.nonce().into(),init.request().kind){Ok(o)=>o,Err(_)=>return 71};
 if owner.assigned(now).is_err(){return 71;}
 let mut stop=false;
 let result=(||->Result<()> {
  let mut live=|ops:&mut _|->Result<()> {match RenderOps::control(ops)? {Control::Live=>Ok(()),Control::Stop=>{stop=true;Err("parent stop before fixture completion".into())},Control::ParentEof=>Err("parent EOF".into())}};
  live(ops)?;let format=ops.open(&init.request().endpoint_id)?;live(ops)?;ops.start()?;live(ops)?;
  emit(ops,&mut owner,&init,Event::Ready{format})?;
  let deadline=ops.now()+Duration::from_secs(13);
  loop {live(ops)?;if ops.now()>=deadline{return Err("Renderer exceeded finite fixture deadline".into());}if ops.next()? {return Ok(());}live(ops)?;ops.wait()?;}
 })();
 let mut complete=result.is_ok();let mut cause=result.err().unwrap_or_else(||"fixture complete".into());
 while cause.len()>1024 {cause.pop();}
 let ending=(||->Result<()> {let observed=ops.stamp()?;if stop {owner.stop(ops.now()).map_err(|e|e.to_string())?;emit(ops,&mut owner,&init,Event::StopAck{observed})}else{emit(ops,&mut owner,&init,Event::Ending{cause:cause.clone(),observed})}})();
 // Never emit release while a Start/Stop/Release/drop is blocked or ownership remains live.
 if let Err(error)=ops.stop_and_drop(){complete=false;cause=error;while cause.len()>1024 {cause.pop();}}
 if ending.is_err(){return 71;}
 let final_events=(||->Result<()> {
  let at=ops.stamp()?;emit(ops,&mut owner,&init,Event::CaptureReleased{at})?;
  emit(ops,&mut owner,&init,Event::Terminal{outcome:TerminalSummary{accepted_frames:0,written_complete_frames:0,synchronized_frames:None,usable:None,retained_partial:false,cause,evidence_present:false}})
 })();
 if complete&&final_events.is_ok(){0}else{71}
}
#[cfg(any(windows,test))]
fn emit(ops:&mut impl RenderOps,owner:&mut Owner,init:&ValidatedInit,event:Event)->Result<()> {
 let frame=EventV1{version:1,nonce:init.nonce().into(),kind:init.request().kind,event};
 let stamp=ops.stamp()?;owner.message(ops.now(),stamp,&frame).map_err(|e|e.to_string())?;
 crate::protocol::encode_event(&frame).map_err(|e|e.to_string())?;ops.emit(&frame)
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::protocol::{InitV1,TrackRequest,TrackKind,encode_init,decode_init_frame};
 struct Ops{trace:Vec<&'static str>,fail:&'static str,stop:bool}
 impl RenderOps for Ops {
  fn control(&mut self)->Result<Control>{Ok(if self.stop {Control::Stop}else{Control::Live})}
  fn open(&mut self,_:&str)->Result<Format>{self.trace.push("open");Ok(Format{rate:48000,channels:2,bits:16,valid_bits:16,align:4,encoding:Encoding::Pcm})}
  fn start(&mut self)->Result<()>{self.trace.push("start");if self.fail=="start" {Err("start fault".into())}else{Ok(())}}
  fn next(&mut self)->Result<bool>{self.trace.push("render");if self.fail=="release"{Err("release fault".into())}else{Ok(true)}}
  fn wait(&mut self)->Result<()>{self.trace.push("wait");Ok(())}
  fn stop_and_drop(&mut self)->Result<()>{self.trace.push("stop/drop");if self.fail=="stop"{Err("stop fault after complete ownership drop".into())}else{Ok(())}}
  fn stamp(&mut self)->Result<ClockStamp>{Ok(ClockStamp{qpc:1,frequency:1000})}
  fn emit(&mut self,event:&EventV1)->Result<()>{self.trace.push(match event.event {Event::Ready{..}=>"Ready",Event::Ending{..}=>"Ending",Event::StopAck{..}=>"StopAck",Event::CaptureReleased{..}=>"CaptureReleased",Event::Terminal{..}=>"Terminal",_=>"unexpected"});Ok(())}
 }
 fn init()->ValidatedInit {let i=InitV1{version:1,nonce:"11111111-1111-1111-1111-111111111111".into(),request:TrackRequest{kind:TrackKind::System,endpoint_id:"owned-id".into(),explicitly_enabled:true,output_dir:std::env::temp_dir().join("render-test")}};decode_init_frame(&encode_init(&i).unwrap(),&i.nonce).unwrap()}
 // A misplaced release acknowledgment or a post-drop render operation breaks the real sequence.
 #[test] fn renderer_release_ack_follows_actual_stop_drop_for_clean_and_fault_paths() {
  for fail in ["","start","release","stop"] {let mut ops=Ops{trace:Vec::new(),fail,stop:false};let code=run_renderer(&mut ops,init());assert_eq!(code,if fail.is_empty(){0}else{71});let expected=if fail=="start"{vec!["open","start","Ending","stop/drop","CaptureReleased","Terminal"]}else{vec!["open","start","Ready","render","Ending","stop/drop","CaptureReleased","Terminal"]};assert_eq!(ops.trace,expected);}
 }
 #[test] fn renderer_prestart_stop_cannot_open_and_ack_precedes_cleanup() {let mut ops=Ops{trace:Vec::new(),fail:"",stop:true};assert_eq!(run_renderer(&mut ops,init()),71);assert_eq!(ops.trace,["StopAck","stop/drop","CaptureReleased","Terminal"]);}
}

// Only IMMDeviceEnumerator::GetDevice uses this HRESULT classification.
#[cfg(any(windows,test))]
pub(super) fn lookup_failure(hresult:i32,reason:String)->super::ProbeError {
 // HRESULT_FROM_WIN32(ERROR_NOT_FOUND), not a broad HRESULT category or error-message match.
 if hresult==0x80070490u32 as i32 {super::ProbeError::Unavailable(reason)}else{super::ProbeError::Failed(reason)}
}
#[cfg(windows)]
const _:()=assert!(windows::core::HRESULT::from_win32(windows::Win32::Foundation::ERROR_NOT_FOUND.0).0==0x80070490u32 as i32);

#[cfg(windows)]
pub(crate) mod native {
 use super::*;
 use crate::{native::{Apartment,Allocation,Event as AudioEvent,WorkerPermit,identity,mix_format,ensure_live},proof::{guard::ProofPermit,ProbeError}};
 use windows::{core::{Interface,PCWSTR},Win32::{Media::Audio::*,System::{Com::*,Threading::*},Foundation::{WAIT_OBJECT_0,WAIT_TIMEOUT}}};
 fn err(e:impl std::fmt::Display)->String{e.to_string()}
 struct Selected {client:IAudioClient,_device:IMMDevice,_enumerator:IMMDeviceEnumerator,format:Format,_com:Apartment}
 impl Selected {
  fn open(proof:&ProofPermit,id:&str,check:impl Fn()->Result<()>)->std::result::Result<Self,ProbeError> {
   if id!=proof.route(){return Err(ProbeError::Unavailable("Endpoint is not the approved owned render route".into()));}
   check()?;let com=Apartment::new()?;check()?;let wide:Vec<_>=id.encode_utf16().chain([0]).collect();
   // SAFETY: exact ID lookup only; interfaces/allocations stay on this owner apartment and drop before it.
   unsafe {
    let enumerator:IMMDeviceEnumerator=CoCreateInstance(&MMDeviceEnumerator,None,CLSCTX_ALL).map_err(err)?;check()?;
    let device=enumerator.GetDevice(PCWSTR(wide.as_ptr())).map_err(|e|lookup_failure(e.code().0,e.to_string()))?;check()?;
    if identity(&device)?!=id {return Err(ProbeError::Failed("Exact lookup returned a different endpoint identity".into()));}
    if device.GetState().map_err(err)?!=DEVICE_STATE_ACTIVE {return Err(ProbeError::Unavailable("Approved render endpoint is inactive".into()));}
    if device.cast::<IMMEndpoint>().map_err(err)?.GetDataFlow().map_err(err)?!=eRender {return Err(ProbeError::Unavailable("Approved endpoint is not a render route".into()));}
    check()?;let client:IAudioClient=device.Activate(CLSCTX_ALL,None).map_err(err)?;check()?;
    let mix=Allocation(client.GetMixFormat().map_err(err)?);
    if mix.0.is_null(){return Err(ProbeError::Failed("GetMixFormat returned no format storage".into()));}
    let format=mix_format(mix.0).map_err(ProbeError::Unavailable)?;
    if format.channels!=2 {return Err(ProbeError::Unavailable("Owned fixture requires an existing stereo render route; no conversion or fallback".into()));}
    check()?;Ok(Self{client,_device:device,_enumerator:enumerator,format,_com:com})
   }
  }
 }
 pub(crate) fn probe_exact(proof:&ProofPermit,id:&str)->std::result::Result<(),ProbeError> {Selected::open(proof,id,||Ok(())).map(drop)}
 struct Client<'a> {render:IAudioRenderClient,ready:AudioEvent,worker:&'a WorkerPermit,signal:Signal,capacity:u32,started:bool,selected:Selected}
 impl<'a> Client<'a> {
  fn open(proof:&ProofPermit,worker:&'a WorkerPermit,id:&str)->Result<Self> {
   let selected=Selected::open(proof,id,||ensure_live(worker)).map_err(ProbeError::reason)?;
   // SAFETY: shared mode uses the unchanged exact mix format; all COM and event handles stay owned.
   unsafe {
    let mix=Allocation(selected.client.GetMixFormat().map_err(err)?);
    if mix_format(mix.0)?!=selected.format {return Err("Render mix format changed before initialization".into());}
    ensure_live(worker)?;selected.client.Initialize(AUDCLNT_SHAREMODE_SHARED,AUDCLNT_STREAMFLAGS_EVENTCALLBACK,0,0,mix.0,None).map_err(err)?;ensure_live(worker)?;
    let ready=AudioEvent::create(false,None)?;selected.client.SetEventHandle(ready.0.raw()).map_err(err)?;ensure_live(worker)?;
    let capacity=selected.client.GetBufferSize().map_err(err)?;if capacity==0 {return Err("Renderer has no buffer capacity".into());}
    let render=selected.client.GetService::<IAudioRenderClient>().map_err(err)?;ensure_live(worker)?;
    let signal=Signal::new(selected.format,1)?;
    Ok(Self{render,ready,worker,signal,capacity,started:false,selected})
   }
  }
  fn start(&mut self)->Result<()> {
   ensure_live(self.worker)?;
   // SAFETY: same exact endpoint/client owner; never start after Stop/EOF or a changed mix/route.
   unsafe {
    if self.selected._device.GetState().map_err(err)?!=DEVICE_STATE_ACTIVE {return Err("Owned render route became inactive".into());}
    let mix=Allocation(self.selected.client.GetMixFormat().map_err(err)?);
    if mix_format(mix.0)?!=self.selected.format {return Err("Render mix format changed before Start".into());}
    ensure_live(self.worker)?;self.selected.client.Start().map_err(err)?;
   }
   self.started=true;Ok(())
  }
  fn next(&mut self)->Result<bool> {
   ensure_live(self.worker)?;
   // SAFETY: valid shared client; padding is checked before subtraction and acquisition.
   let padding=unsafe{self.selected.client.GetCurrentPadding().map_err(err)?};
   let available=self.capacity.checked_sub(padding).ok_or("Renderer padding exceeds buffer capacity")?;
   if self.signal.remaining()==0 {return Ok(padding==0);}
   let mut available=available;
   for _ in 0..8 {
   let frames=available.min(BLOCK_FRAMES).min(self.signal.remaining());if frames==0{break;}
   let bytes=self.signal.block(frames)?;ensure_live(self.worker)?;
   // SAFETY: exactly frames of writable WASAPI storage; copy bounded owned bytes, release once on this thread.
   unsafe {
    let buffer=self.render.GetBuffer(frames).map_err(err)?;
    if buffer.is_null(){let _=self.render.ReleaseBuffer(frames,AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);return Err("Renderer returned null writable buffer".into());}
    std::ptr::copy_nonoverlapping(bytes.as_ptr(),buffer,bytes.len());
    self.render.ReleaseBuffer(frames,0).map_err(err)?;
   }
   available-=frames;
   }
   Ok(false)
  }
  fn wait(&self)->Result<()> {
   ensure_live(self.worker)?;
   // A 10 ms event wait bounds EOF/Stop polling without reading or owning any extra control thread.
   let result=unsafe{WaitForSingleObject(self.ready.0.raw(),10)};
   if result==WAIT_OBJECT_0 || result==WAIT_TIMEOUT {Ok(())}else{Err("Renderer event wait failed".into())}
  }
  fn stop(&mut self)->Result<()> {
   // SAFETY: consume the single Stop attempt on this same-thread client before possible HRESULT failure.
   if self.started {self.started=false;unsafe{self.selected.client.Stop().map_err(err)?;}}
   Ok(())
  }
 }
 impl Drop for Client<'_>{fn drop(&mut self){let _=self.stop();}}
 struct NativeOps<'a> {proof:&'a ProofPermit,worker:&'a WorkerPermit,client:Option<Client<'a>>}
 impl RenderOps for NativeOps<'_> {
  fn control(&mut self)->Result<Control>{self.worker.control()}
  fn open(&mut self,id:&str)->Result<Format>{let client=Client::open(self.proof,self.worker,id)?;let format=client.selected.format;self.client=Some(client);Ok(format)}
  fn start(&mut self)->Result<()>{self.client.as_mut().ok_or("Renderer not opened")?.start()}
  fn next(&mut self)->Result<bool>{self.client.as_mut().ok_or("Renderer not opened")?.next()}
  fn wait(&mut self)->Result<()>{self.client.as_ref().ok_or("Renderer not opened")?.wait()}
  fn stop_and_drop(&mut self)->Result<()>{if let Some(mut client)=self.client.take(){let result=client.stop();drop(client);result}else{Ok(())}}
  fn stamp(&mut self)->Result<ClockStamp>{self.worker.stamp()}
  fn emit(&mut self,event:&EventV1)->Result<()>{use std::io::Write;let frame=crate::protocol::encode_event(event).map_err(err)?;let mut out=std::io::stdout().lock();out.write_all(&frame).map_err(err)?;out.flush().map_err(err)}
 }
 pub(crate) fn dispatch(proof:ProofPermit,job:&str,stop:&str,nonce:&str)->i32 {
  let (worker,init)=match WorkerPermit::receive(job,stop,nonce){Ok(v)=>v,Err(_)=>return 71};
  if init.request().kind!=crate::protocol::TrackKind::System || init.request().endpoint_id!=proof.route(){return 71;}
  let mut ops=NativeOps{proof:&proof,worker:&worker,client:None};
  run_renderer(&mut ops,init)
 }
}

#[cfg(test)]
mod probe_tests {
 use super::lookup_failure;
 use crate::proof::ProbeError;
 #[test] fn only_exact_device_not_found_hresult_is_unavailable() {
  for (hresult,absent) in [(0x80070490u32,true),(0x80004005,false),(0x80070005,false),(0x88890004,false),(0x80070002,false),(0,false)] {
   let reason="identical message text".to_string();
   assert_eq!(lookup_failure(hresult as i32,reason.clone()),if absent{ProbeError::Unavailable(reason)}else{ProbeError::Failed(reason)},"HRESULT {hresult:08x}");
  }
 }
}
