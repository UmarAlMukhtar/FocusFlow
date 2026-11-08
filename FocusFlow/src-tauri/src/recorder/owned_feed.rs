//! Portable arithmetic, owned pixels and causal selector for the Windows WGC worker.
use std::{collections::VecDeque, io::{self, Write}, sync::{Arc, atomic::{AtomicUsize, Ordering}}};
pub const MAX_OWNED: usize = 22;
pub const MAX_FRAMES: u32 = 2_592_000;
#[derive(Clone, Copy, Debug)]
pub struct Geometry { pub width: usize, pub height: usize, pub encoded_width: usize, pub encoded_height: usize, pub bytes: usize }
impl Geometry {
 pub fn matches(&self,content:(i32,i32),texture:(u32,u32))->bool {content==(self.width as i32,self.height as i32)&&texture==(self.width as u32,self.height as u32)}
 pub fn new(width:u32,height:u32)->Result<Self,&'static str>{
  if !(2..=16384).contains(&width)||!(2..=16384).contains(&height){return Err("Window dimensions must be 2..16384 pixels");}
  let (ew,eh)=((width+width%2) as usize,(height+height%2) as usize);
  let bytes=ew.checked_mul(eh).and_then(|n|n.checked_mul(4)).ok_or("Window pixel budget overflow")?;
  if bytes.checked_mul(MAX_OWNED).is_none_or(|n|n>256*1024*1024){return Err("Window exceeds the 256 MiB owned-pixel budget; use a smaller window");}
  Ok(Self{width:width as usize,height:height as usize,encoded_width:ew,encoded_height:eh,bytes})
 }
}
pub enum CaptureGate {
 Warming {deadline:std::time::Instant},
 Frozen {geometry:Geometry},
 Armed {geometry:Geometry,admission:i64},
}
#[derive(Clone,Copy,Debug)]
pub enum FrameAction {Discard,GeometryChanged,Copy{geometry:Geometry,admission:i64}}
impl CaptureGate {
 pub fn new(deadline:std::time::Instant)->Self {Self::Warming{deadline}}
 pub fn observe(&mut self,content:(i32,i32),texture:(u32,u32),now:std::time::Instant)->Result<FrameAction,&'static str> {
  match self {
   Self::Warming{deadline}=>{
    if now>=*deadline{return Err("Timed out waiting for matching window dimensions");}
    let geometry=Geometry::new(u32::try_from(content.0).map_err(|_|"Invalid window content width")?,u32::try_from(content.1).map_err(|_|"Invalid window content height")?)?;
    Geometry::new(texture.0,texture.1)?;
    if geometry.matches(content,texture){*self=Self::Frozen{geometry};}
    Ok(FrameAction::Discard)
   },
   Self::Frozen{geometry}=>Ok(if geometry.matches(content,texture){FrameAction::Discard}else{FrameAction::GeometryChanged}),
   Self::Armed{geometry,admission}=>Ok(if geometry.matches(content,texture){FrameAction::Copy{geometry:*geometry,admission:*admission}}else{FrameAction::GeometryChanged}),
  }
 }
 pub fn geometry(&self)->Option<Geometry> {
  match self {Self::Warming{..}=>None,Self::Frozen{geometry}|Self::Armed{geometry,..}=>Some(*geometry)}
 }
 pub fn arm(&mut self,admission:i64)->Result<(),&'static str> {
  if admission<0{return Err("Invalid native clock admission");}
  let Self::Frozen{geometry}=self else {return Err("Window capture must be frozen before admission");};
  *self=Self::Armed{geometry:*geometry,admission};Ok(())
 }
}
#[derive(Default)]
pub struct OwnedCount {pub live:AtomicUsize}
pub struct OwnedPixels {pub bytes:Vec<u8>,live:Arc<OwnedCount>}
impl OwnedPixels {
 pub fn copy(raw:&[u8],pitch:usize,g:Geometry,live:Arc<OwnedCount>)->Result<Self,&'static str>{
  if pitch<g.width*4||pitch.checked_mul(g.height).is_none_or(|n|raw.len()<n){return Err("Invalid BGRA row pitch or buffer length");}
  let previous=live.live.fetch_add(1,Ordering::SeqCst);
  if previous>=MAX_OWNED{live.live.fetch_sub(1,Ordering::SeqCst);return Err("Owned pixel buffer limit exceeded");}
  let mut bytes=Vec::new();if bytes.try_reserve_exact(g.bytes).is_err(){live.live.fetch_sub(1,Ordering::SeqCst);return Err("Could not allocate window pixels");}bytes.resize(g.bytes,0);
  for y in 0..g.height {bytes[y*g.encoded_width*4..y*g.encoded_width*4+g.width*4].copy_from_slice(&raw[y*pitch..y*pitch+g.width*4]);}
  Ok(Self{bytes,live})
 }
}
impl Drop for OwnedPixels {fn drop(&mut self){self.live.live.fetch_sub(1,Ordering::SeqCst);}}
pub fn admitted(native:i64,admission:i64,frequency:i64)->Result<bool,&'static str>{
 if native<0||admission<0||frequency<=0{return Err("Invalid native clock input");}
 Ok(i128::from(native)*i128::from(frequency)>=i128::from(admission)*10_000_000)
}
pub fn complete_cells(epoch:i64,cutoff:i64,frequency:i64)->Result<u32,&'static str>{
 if epoch<0||cutoff<0||frequency<=0{return Err("Invalid native clock input");}
 let delta=(i128::from(cutoff)*10_000_000).checked_sub(i128::from(epoch)*i128::from(frequency)).ok_or("Clock overflow")?;
 let cells=delta.checked_mul(30).ok_or("Clock overflow")?/(i128::from(frequency)*10_000_000);
 u32::try_from(cells.max(0)).map_err(|_|"Complete cell count overflow")
}
#[derive(Clone)]
pub struct Source {pub seq:u64,pub native:i64,pub published:i64,pub pixels:Arc<OwnedPixels>}
pub struct Selector {pub epoch:Option<i64>,admission:i64,frequency:i64,previous:Option<(i64,u64,i64)>,future:VecDeque<Source>,last:Option<Source>,next:u32}
impl Selector {
 pub fn new(admission:i64,frequency:i64)->Result<Self,&'static str>{admitted(0,admission,frequency)?;Ok(Self{epoch:None,admission,frequency,previous:None,future:VecDeque::new(),last:None,next:0})}
 pub fn push(&mut self,s:Source)->Result<bool,&'static str>{
  if s.published<0||self.previous.is_some_and(|(t,n,p)|s.native<t||s.seq<=n||s.published<p){return Err("Native timestamp or delivery sequence regression");}
  let accept=admitted(s.native,self.admission,self.frequency)?;self.previous=Some((s.native,s.seq,s.published));if !accept{return Ok(false);}
  if self.future.len()>=8{return Err("Future snapshot queue overflow");}self.epoch.get_or_insert(s.native);self.future.push_back(s);Ok(true)
 }
 pub fn select(&mut self,n:u32,decision:i64)->Result<Source,&'static str>{
  if n!=self.next||n>=MAX_FRAMES{return Err("Output ordinal invalid or duration limit reached");}
  let epoch=self.epoch.ok_or("No admitted snapshot")?;if complete_cells(epoch,decision,self.frequency)?<=n{return Err("Cell has not ended");}
  // Publication and decision share the native mutex: a same-QPC later delivery cannot rewrite this choice.
  while self.future.front().is_some_and(|s|s.published<=decision&&30*(i128::from(s.native)-i128::from(epoch))<=i128::from(n)*10_000_000){self.last=self.future.pop_front();}
  let selected=self.last.clone().ok_or("No causal snapshot")?;self.next+=1;Ok(selected)
 }
}
pub fn write_frame(writer:&mut impl Write,bytes:&[u8],written:&mut usize)->io::Result<()> {
 *written=0;while *written<bytes.len(){match writer.write(&bytes[*written..(*written+65536).min(bytes.len())]){Ok(0)=>return Err(io::ErrorKind::WriteZero.into()),Ok(n)=>*written+=n,Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)=>return Err(e)}}Ok(())
}
#[cfg(test)]
mod tests {
 use super::*;
 fn source(seq:u64,t:i64,p:i64)->Source {Source{seq,native:t,published:p,pixels:Arc::new(OwnedPixels::copy(&[seq as u8;16],8,Geometry::new(2,2).unwrap(),Arc::new(OwnedCount::default())).unwrap())}}
 #[test] fn startup_geometry_freezes_first_equal_callback_without_pixels() {
  use std::time::{Duration,Instant};
  let t0=Instant::now();let mut gate=CaptureGate::new(t0+Duration::from_secs(2));
  assert!(matches!(gate.observe((322,272),(336,279),t0+Duration::from_millis(1)),Ok(FrameAction::Discard)));
  assert!(gate.geometry().is_none());
  assert!(matches!(gate.observe((322,272),(322,272),t0+Duration::from_millis(2)),Ok(FrameAction::Discard)));
  let frozen=gate.geometry().unwrap();assert_eq!((frozen.width,frozen.height),(322,272));
  assert!(matches!(gate.observe((322,272),(322,272),t0+Duration::from_millis(3)),Ok(FrameAction::Discard)));
  gate.arm(123).unwrap();
  assert!(matches!(gate.observe((322,272),(322,272),t0+Duration::from_millis(4)),Ok(FrameAction::Copy{geometry,admission:123}) if (geometry.width,geometry.height)==(322,272)));
 }
 #[test] fn startup_geometry_deadline_invalid_and_postfreeze_changes_fail() {
  use std::time::{Duration,Instant};
  let t0=Instant::now();let deadline=t0+Duration::from_secs(2);
  for bad in [(0,272),(-1,272),(1,272),(16385,272),(3840,2160),(322,0),(322,-1),(322,1),(322,16385)] {
   let mut gate=CaptureGate::new(deadline);
   assert!(gate.observe(bad,(322,272),t0).is_err(),"invalid content: {bad:?}");
   assert!(gate.geometry().is_none());
  }
  for bad in [(0,272),(1,272),(16385,272),(3840,2160),(322,0),(322,1),(322,16385),(u32::MAX,272)] {
   let mut gate=CaptureGate::new(deadline);
   assert!(gate.observe((322,272),bad,t0).is_err(),"invalid texture: {bad:?}");
   assert!(gate.geometry().is_none());
  }
  for now in [deadline,deadline+Duration::from_nanos(1)] {
   for texture in [(322,272),(336,279)] {
    let mut gate=CaptureGate::new(deadline);
    assert!(gate.observe((322,272),texture,now).is_err());assert!(gate.geometry().is_none());
   }
  }
  for arm in [false,true] {
   for (content,texture) in [((323,272),(322,272)),((322,272),(323,272)),((323,272),(323,272)),((-1,272),(322,272))] {
    let mut gate=CaptureGate::new(deadline);
    assert!(matches!(gate.observe((322,272),(322,272),deadline-Duration::from_nanos(1)),Ok(FrameAction::Discard)));
    if arm {gate.arm(123).unwrap();}
    assert!(matches!(gate.observe(content,texture,deadline),Ok(FrameAction::GeometryChanged)));
    let frozen=gate.geometry().unwrap();assert_eq!((frozen.width,frozen.height),(322,272));
   }
  }
  let mut gate=CaptureGate::new(deadline);
  assert!(gate.arm(123).is_err());assert!(gate.geometry().is_none());
  gate.observe((322,272),(322,272),t0).unwrap();
  assert!(gate.arm(-1).is_err());
  assert!(matches!(gate.observe((322,272),(322,272),deadline),Ok(FrameAction::Discard)));
  gate.arm(0).unwrap();assert!(gate.arm(123).is_err());
  assert!(matches!(gate.observe((322,272),(322,272),deadline),Ok(FrameAction::Copy{admission:0,..})));
 }
 #[test] fn startup_gate_action_is_not_upgraded_by_later_arm() {
  use std::time::{Duration,Instant};
  let t0=Instant::now();let mut gate=CaptureGate::new(t0+Duration::from_secs(2));
  gate.observe((322,272),(322,272),t0).unwrap();
  let entered_frozen=gate.observe((322,272),(322,272),t0+Duration::from_millis(1)).unwrap();
  gate.arm(3_000_000).unwrap();assert!(matches!(entered_frozen,FrameAction::Discard));
  let FrameAction::Copy{admission,..}=gate.observe((322,272),(322,272),t0).unwrap() else {panic!("armed callback must authorize copy")};
  let mut selector=Selector::new(admission,3_000_000).unwrap();
  assert!(!admitted(9_999_999,admission,3_000_000).unwrap());
  assert!(!selector.push(source(0,9_999_999,3_000_000)).unwrap());assert_eq!(selector.epoch,None);
  assert!(selector.push(source(1,10_000_001,3_000_001)).unwrap());assert_eq!(selector.epoch,Some(10_000_001));
  assert_eq!(selector.select(0,3_100_001).unwrap().seq,1);
 }
 #[test] fn admission_and_non_10mhz_cutoff_are_exact() {
  for (t,a,f,want) in [(9_999_999,3_000_000,3_000_000,false),(10_000_000,3_000_000,3_000_000,true),(10_000_001,3_000_000,3_000_000,true)] {assert_eq!(admitted(t,a,f).unwrap(),want);}
  for (s,want) in [(3_099_999,0),(3_100_000,1),(3_100_001,1),(3_200_000,2),(2_000_000,0)] {assert_eq!(complete_cells(10_000_000,s,3_000_000).unwrap(),want);}
  assert_eq!(complete_cells(10_000_000,31_000_000,30_000_000).unwrap(),1);
  assert!(complete_cells(i64::MAX,i64::MAX,i64::MAX).is_err());assert!(admitted(-1,0,1).is_err());
 }
 #[test] fn same_qpc_frontier_and_equal_native_ties_never_rewrite_selected_frame() {
  let mut s=Selector::new(3_000_000,3_000_000).unwrap();
  assert!(!s.push(source(0,9_999_999,3_000_000)).unwrap());
  s.push(source(1,10_000_000,3_000_001)).unwrap();s.push(source(2,10_000_000,3_100_000)).unwrap();
  assert!(s.select(0,3_099_999).is_err());let previous=s.select(0,3_100_000).unwrap();assert_eq!(previous.seq,2);assert_eq!(previous.pixels.bytes,vec![2;16]);
  s.push(source(3,10_000_000,3_100_000)).unwrap();assert_eq!(s.select(1,3_200_000).unwrap().seq,3);assert_eq!(previous.seq,2);assert_eq!(previous.pixels.bytes,vec![2;16]);
  s.push(source(4,11_000_000,3_200_001)).unwrap();assert_eq!(s.select(2,3_300_000).unwrap().seq,3);
  assert_eq!(s.select(3,3_400_000).unwrap().seq,4);
 }
 #[test] fn variable_pitch_padding_preserves_pixels_and_owns_callback_bytes() {
  let g=Geometry::new(3,3).unwrap();assert_eq!((g.encoded_width,g.encoded_height,g.bytes),(4,4,64));
  let mut raw:Vec<u8>=(1..=48).collect();let live=Arc::new(OwnedCount::default());
  let p=OwnedPixels::copy(&raw,16,g,live.clone()).unwrap();raw.fill(99);
  assert_eq!(&p.bytes[..16],&[1,2,3,4,5,6,7,8,9,10,11,12,0,0,0,0]);
  assert_eq!(&p.bytes[32..48],&[33,34,35,36,37,38,39,40,41,42,43,44,0,0,0,0]);assert_eq!(&p.bytes[48..],&[0;16]);
  assert_eq!(live.live.load(Ordering::SeqCst),1);drop(p);assert_eq!(live.live.load(Ordering::SeqCst),0);
  assert!(OwnedPixels::copy(&raw,11,g,live.clone()).is_err());assert!(OwnedPixels::copy(&raw[..30],16,g,live).is_err());
 }
 #[test] fn first_resize_frame_old_texture_new_content_is_rejected_before_copy() {
  let g=Geometry::new(641,359).unwrap();assert!(g.matches((641,359),(641,359)));
  assert!(!g.matches((640,359),(641,359)));assert!(!g.matches((641,360),(641,359)));
  assert!(!g.matches((641,359),(642,360)));assert!(!g.matches((-1,359),(641,359)));
 }
 #[test] fn dimensions_memory_ownership_and_future_queue_are_bounded() {
  for (w,h) in [(0,2),(1,2),(16385,2),(16384,16384),(3840,2160)] {assert!(Geometry::new(w,h).is_err());}
  let g=Geometry::new(2,2).unwrap();let live=Arc::new(OwnedCount::default());let mut held=vec![];
  for _ in 0..22 {held.push(OwnedPixels::copy(&[0;16],8,g,live.clone()).unwrap());}assert!(OwnedPixels::copy(&[0;16],8,g,live.clone()).is_err());drop(held);assert_eq!(live.live.load(Ordering::SeqCst),0);
  let mut s=Selector::new(0,10_000_000).unwrap();for n in 0..8 {s.push(source(n,n as i64,0)).unwrap();}assert!(s.push(source(8,8,0)).is_err());
  let mut s=Selector::new(0,10_000_000).unwrap();s.push(source(4,4,5)).unwrap();assert!(s.push(source(3,4,5)).is_err());assert!(s.push(source(5,3,5)).is_err());
 }
 struct Short {data:Vec<u8>,calls:usize,fail:Option<io::ErrorKind>}
 impl Write for Short {fn write(&mut self,b:&[u8])->io::Result<usize>{assert!(b.len()<=65536);self.calls+=1;if self.calls==1{return Err(io::ErrorKind::Interrupted.into())}if self.calls==3 {if let Some(e)=self.fail{return if e==io::ErrorKind::WriteZero{Ok(0)}else{Err(e.into())}}}let n=b.len().min(3);self.data.extend_from_slice(&b[..n]);Ok(n)}fn flush(&mut self)->io::Result<()>{Ok(())}}
 #[test] fn interrupted_short_zero_and_broken_writes_keep_exact_progress() {
  for failure in [None,Some(io::ErrorKind::WriteZero),Some(io::ErrorKind::BrokenPipe)] {let mut w=Short{data:vec![],calls:0,fail:failure};let mut count=0;let result=write_frame(&mut w,b"abcdefghij",&mut count);if failure.is_none(){result.unwrap();assert_eq!(w.data,b"abcdefghij");assert_eq!(count,10)}else{assert!(result.is_err());assert_eq!(count,3);assert_eq!(w.data,b"abc")}}
 }
}
