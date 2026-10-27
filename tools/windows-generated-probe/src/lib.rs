pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 360;
pub const MAX_FRAMES: u32 = 600;

pub fn marker(pixels: &[u8], channels: usize) -> Option<u16> { marker_at(pixels,channels,160) }
pub fn marker_at(pixels: &[u8], channels: usize, y:usize) -> Option<u16> {
    if y>=HEIGHT {return None;}
    if !(channels == 3 || channels == 4) || pixels.len() != WIDTH.checked_mul(HEIGHT)?.checked_mul(channels)? { return None; }
    let mut id = 0u16;
    for bit in 0..16 {
        let i = (y * WIDTH + 40 + bit * 32) * channels;
        let rgb = &pixels[i..i+3];
        if rgb.iter().all(|c| *c > 210) { id |= 1 << bit; }
        else if !rgb.iter().all(|c| *c < 40) { return None; }
    }
    Some(id)
}
pub fn qpc_100ns(ticks: i64, frequency: i64) -> Option<i64> {
    if frequency <= 0 { return None; }
    i64::try_from(i128::from(ticks).checked_mul(10_000_000)? / i128::from(frequency)).ok()
}


use std::{collections::VecDeque, io::{self,Write}, sync::{Arc,atomic::{AtomicUsize,Ordering}}};
pub const FRAME_BYTES:usize=WIDTH*HEIGHT*4;
// Callback + source queue + future queue + retained + output queue + writer + controller.
pub const MAX_OWNED:usize=1+8+8+1+2+1+1;
#[derive(Default)]
pub struct OwnedCount {pub live:AtomicUsize,pub high:AtomicUsize}
pub struct OwnedPixels { pub bytes:Vec<u8>, live:Arc<OwnedCount> }
impl OwnedPixels {
 pub fn copy(raw:&[u8],pitch:usize,live:Arc<OwnedCount>)->Result<Self,&'static str> {
  if pitch<WIDTH*4 || pitch.checked_mul(HEIGHT).is_none_or(|n|raw.len()<n) {return Err("invalid row pitch or buffer length");}
  let previous=live.live.fetch_add(1,Ordering::SeqCst);
  if previous>=MAX_OWNED {live.live.fetch_sub(1,Ordering::SeqCst);return Err("owned buffer cap exceeded");}
  live.high.fetch_max(previous+1,Ordering::SeqCst);
  let mut bytes=vec![0;FRAME_BYTES];
  for y in 0..HEIGHT {bytes[y*WIDTH*4..(y+1)*WIDTH*4].copy_from_slice(&raw[y*pitch..y*pitch+WIDTH*4]);}
  Ok(Self{bytes,live})
 }
}
impl Drop for OwnedPixels {fn drop(&mut self){self.live.live.fetch_sub(1,Ordering::SeqCst);}}
pub fn stamp_tag(pixels:&mut [u8],tag:u16)->Result<(),&'static str>{
 if pixels.len()!=FRAME_BYTES{return Err("invalid tag buffer");}
 for bit in 0..16 {let c=if tag&(1<<bit)!=0{240}else{12};for y in 224..256 {for x in 32+bit*32..56+bit*32 {pixels[(y*WIDTH+x)*4..(y*WIDTH+x)*4+4].copy_from_slice(&[c,c,c,255]);}}}
 Ok(())
}
pub fn valid_corners(pixels:&[u8])->bool {
 if pixels.len()!=FRAME_BYTES{return false;}
 [(16,16,[0u8,0,255]),(624,16,[0,255,0]),(16,344,[255,0,0]),(624,344,[0,255,255])].iter().all(|(x,y,color)|{
  (0..3).all(|c|{let mut sum=0u32;for dy in -3isize..=3 {for dx in -3isize..=3 {sum+=pixels[(((*y as isize+dy) as usize)*WIDTH+(*x as isize+dx) as usize)*4+c] as u32;}}(sum as f64/49.0-color[c] as f64).abs()<=60.0})
 })
}
pub fn admitted(native:i64,admission:i64,f:i64)->Result<bool,&'static str>{
 if native<0 || admission<0 || f<=0{return Err("negative timestamp or invalid frequency");}
 Ok(i128::from(native).checked_mul(i128::from(f)).ok_or("timestamp overflow")?>=i128::from(admission).checked_mul(10_000_000).ok_or("timestamp overflow")?)
}
pub fn complete_cells(epoch:i64,cutoff:i64,f:i64)->Result<u32,&'static str>{
 if epoch<0||cutoff<0||f<=0{return Err("invalid clock input");}
 let delta=i128::from(cutoff).checked_mul(10_000_000).and_then(|a|i128::from(epoch).checked_mul(i128::from(f)).and_then(|b|a.checked_sub(b))).ok_or("clock overflow")?;
 // Checked before clamping so extreme invalid clock domains cannot hide overflow.
 let cells=delta.checked_mul(30).ok_or("clock overflow")? / (i128::from(f)*10_000_000);
 u32::try_from(cells.max(0)).map_err(|_|"cell count overflow")
}
pub fn eligible(native:i64,epoch:i64,ordinal:u32)->bool {
 30*(i128::from(native)-i128::from(epoch))<=i128::from(ordinal)*10_000_000
}
#[derive(Clone)]
pub struct Source {pub seq:u32,pub native:i64,pub copy_complete:i64,pub id:u16,pub pixels:Arc<OwnedPixels>}
pub struct Selector {pub epoch:Option<i64>,admission:i64,frequency:i64,previous:Option<i64>,future:VecDeque<Source>,last:Option<Source>,next:u32,pub high:usize}
impl Selector {
 pub fn new(admission:i64,frequency:i64)->Result<Self,&'static str>{
  admitted(0,admission,frequency)?;Ok(Self{epoch:None,admission,frequency,previous:None,future:VecDeque::new(),last:None,next:0,high:0})
 }
 pub fn push(&mut self,source:Source)->Result<bool,&'static str>{
  if self.previous.is_some_and(|t|source.native<=t){return Err("native timestamp regression or duplicate");}
  let accept=admitted(source.native,self.admission,self.frequency)?;self.previous=Some(source.native);
  if !accept{return Ok(false);}
  if self.future.len()>=8{return Err("future snapshot queue overflow");}
  self.epoch.get_or_insert(source.native);self.future.push_back(source);self.high=self.high.max(self.future.len());Ok(true)
 }
 pub fn select(&mut self,n:u32,decision:i64)->Result<Source,&'static str>{
  if n!=self.next||n>=MAX_FRAMES{return Err("output ordinal invalid or cap reached");}
  let epoch=self.epoch.ok_or("no admitted snapshot")?;
  if complete_cells(epoch,decision,self.frequency)?<=n{return Err("cell has not ended");}
  while self.future.front().is_some_and(|s|s.copy_complete<=decision&&eligible(s.native,epoch,n)) {self.last=self.future.pop_front();}
  let selected=self.last.clone().ok_or("no causal snapshot")?;self.next+=1;Ok(selected)
 }
}
pub fn write_frame(writer:&mut impl Write,bytes:&[u8],written:&mut usize)->io::Result<()> {
 *written=0;
 while *written<bytes.len(){match writer.write(&bytes[*written..(*written+65536).min(bytes.len())]){Ok(0)=>return Err(io::ErrorKind::WriteZero.into()),Ok(n)=>*written+=n,Err(e) if e.kind()==io::ErrorKind::Interrupted=>{},Err(e)=>return Err(e)}}
 Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_decodes_independent_literal_and_rejects_damage() {
        let mut pixels = vec![96; WIDTH * HEIGHT * 4];
        // Literal ID 0xa5 = bits 0,2,5,7, unrelated to decoder implementation.
        for bit in 0..16 {
            let c = if [0,2,5,7].contains(&bit) { 240 } else { 12 };
            let i = (160 * WIDTH + 40 + bit * 32) * 4;
            pixels[i..i+3].fill(c);
        }
        assert_eq!(marker(&pixels,4), Some(165));
        pixels[(160*WIDTH+40)*4] = 120;
        assert_eq!(marker(&pixels,4),None);
        assert_eq!(marker(&pixels[..20],4),None);
    }
    #[test]
    fn qpc_conversion_uses_wide_checked_signed_math() {
        assert_eq!(qpc_100ns(12_345_678,3_000_000),Some(41_152_260));
        assert_eq!(qpc_100ns(-3,10_000_000),Some(-3));
        assert_eq!(qpc_100ns(i64::MAX,1),None);
        assert_eq!(qpc_100ns(1,0),None);
    }
}

#[cfg(test)]
mod owned_tests {
 use super::*;
 use std::{io::{self,Write},sync::{Arc,atomic::Ordering}};
 fn source(seq:u32,t:i64,c:i64)->Source { Source{seq,native:t,copy_complete:c,id:seq as u16,pixels:Arc::new(OwnedPixels::copy(&vec![0;FRAME_BYTES], WIDTH*4, Arc::new(OwnedCount::default())).unwrap())} }
 #[test] fn exact_grid_and_stop_boundaries() {
  assert_eq!(complete_cells(10_000_000,3_099_999,3_000_000).unwrap(),0);
  assert_eq!(complete_cells(10_000_000,3_100_000,3_000_000).unwrap(),1);
  assert_eq!(complete_cells(10_000_000,3_200_000,3_000_000).unwrap(),2);
  assert_eq!(complete_cells(10_000_000,3_300_001,3_000_000).unwrap(),3);
  assert_eq!(complete_cells(10_000_000,2_000_000,3_000_000).unwrap(),0);
  assert!(complete_cells(i64::MAX,i64::MAX,i64::MAX).is_err());
  assert!(complete_cells(0,1,0).is_err());
 }
 #[test] fn admission_future_exclusion_idle_late_and_stall() {
  let mut s=Selector::new(3_000_000,3_000_000).unwrap();
  assert!(!s.push(source(0,9_999_999,3_000_001)).unwrap());
  assert!(s.push(source(1,10_000_000,3_000_002)).unwrap());
  s.push(source(2,10_400_000,3_120_000)).unwrap();
  assert!(s.select(0,3_099_999).is_err());
  assert_eq!(s.select(0,3_100_000).unwrap().seq,1);
  assert_eq!(s.select(1,3_200_000).unwrap().seq,1);
  assert_eq!(s.select(2,3_300_000).unwrap().seq,2);
  // A late callback changes only subsequent decisions; idle/stall holds owned bytes.
  s.push(source(3,10_500_000,4_000_000)).unwrap();
  assert_eq!(s.select(3,4_000_000).unwrap().seq,3);
  assert_eq!(s.select(4,4_100_000).unwrap().seq,3);
 }
 #[test] fn duplicates_regressions_future_queue_and_output_cap_fail() {
  let mut s=Selector::new(0,10_000_000).unwrap();s.push(source(0,0,0)).unwrap();
  assert!(s.push(source(1,0,1)).is_err());
  assert!(s.push(source(1,-1,1)).is_err());
  for n in 1..8 {s.push(source(n,n as i64*1_000_000, n as i64*1_000_000)).unwrap();}
  assert!(s.push(source(8,8_000_000,8_000_000)).is_err());
  assert!(s.select(MAX_FRAMES,9_000_000).is_err());
  let (tx,_rx)=std::sync::mpsc::sync_channel(2);tx.try_send(1).unwrap();tx.try_send(2).unwrap();assert!(tx.try_send(3).is_err());
 }
 #[test] fn owned_padded_copy_survives_mutation_and_tracks_bound() {
  let live=Arc::new(OwnedCount::default());let pitch=WIDTH*4+16;let mut raw=vec![7;pitch*HEIGHT];
  let mut p=OwnedPixels::copy(&raw,pitch,live.clone()).unwrap();raw.fill(99);
  assert!(p.bytes.iter().all(|b|*b==7));stamp_tag(&mut p.bytes,165).unwrap();
  assert_eq!(marker_at(&p.bytes,4,240),Some(165));assert_eq!(live.live.load(Ordering::SeqCst),1);
  drop(p);assert_eq!(live.live.load(Ordering::SeqCst),0);
  assert!(OwnedPixels::copy(&raw[..10],pitch,live.clone()).is_err());
  assert!(OwnedPixels::copy(&raw,WIDTH*4-1,live.clone()).is_err());
  live.live.store(MAX_OWNED,Ordering::SeqCst);assert!(OwnedPixels::copy(&raw,pitch,live).is_err());
 }
 struct ShortWriter { data:Vec<u8>, calls:usize, fail:Option<io::ErrorKind> }
 impl Write for ShortWriter {
  fn write(&mut self,b:&[u8])->io::Result<usize>{ self.calls+=1;if self.calls==1{return Err(io::ErrorKind::Interrupted.into())}if self.calls==3 {if let Some(e)=self.fail{return if e==io::ErrorKind::WriteZero{Ok(0)}else{Err(e.into())}}}let n=b.len().min(3);self.data.extend_from_slice(&b[..n]);Ok(n) }
  fn flush(&mut self)->io::Result<()>{Ok(())}
 }
 #[test] fn partial_write_never_restarts_and_preserves_failure_count() {
  for failure in [None,Some(io::ErrorKind::WriteZero),Some(io::ErrorKind::BrokenPipe)] {
   let mut w=ShortWriter{data:vec![],calls:0,fail:failure};let mut count=0;let r=write_frame(&mut w,b"abcdefghij",&mut count);
   if failure.is_none(){r.unwrap();assert_eq!(w.data,b"abcdefghij");assert_eq!(count,10);}else{assert!(r.is_err());assert_eq!(w.data,b"abc");assert_eq!(count,3);}
  }
 }
}

#[derive(Debug)]
pub struct SupervisorResult {pub reason:Option<&'static str>,pub request_succeeded:bool,pub observed_qpc:i64}
pub fn finish_supervisor(stop:&std::sync::atomic::AtomicBool,rx:&std::sync::mpsc::Receiver<SupervisorResult>)->Result<SupervisorResult,&'static str>{
 // A shutdown request must not erase an action already in progress.
 stop.store(true,Ordering::SeqCst);
 rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_|"supervisor completion unobserved; fallback cleanup required")
}

#[cfg(test)]
mod supervisor_tests {
 use super::*;
 use std::sync::{Arc,atomic::{AtomicBool,Ordering},mpsc};
 #[test]
 fn finalization_observes_action_result_in_either_scheduling_order() {
  for completed_first in [true,false] {
   let stop=Arc::new(AtomicBool::new(false));let acted=Arc::new(AtomicBool::new(false));let(tx,rx)=mpsc::sync_channel(1);
   let result=SupervisorResult{reason:Some("writer stalled over 1 second"),request_succeeded:true,observed_qpc:123};
   if completed_first {acted.store(true,Ordering::SeqCst);tx.send(result).unwrap();}
   else {let stopped=stop.clone();let action=acted.clone();std::thread::spawn(move||{
    // Model an already-started native termination returning after shutdown is requested.
    while !stopped.load(Ordering::SeqCst){std::thread::yield_now();}
    action.store(true,Ordering::SeqCst);tx.send(result).unwrap();
   });}
   let observed=finish_supervisor(&stop,&rx).unwrap();
   assert!(stop.load(Ordering::SeqCst));assert!(acted.load(Ordering::SeqCst));assert!(observed.request_succeeded);
   assert_eq!(observed.reason,Some("writer stalled over 1 second"));assert_eq!(observed.observed_qpc,123);
  }
 }
 #[test]
 fn missing_supervisor_result_is_failure_not_termination_proof() {
  let stop=AtomicBool::new(false);let(tx,rx)=mpsc::sync_channel(1);drop(tx);
  assert_eq!(finish_supervisor(&stop,&rx).unwrap_err(),"supervisor completion unobserved; fallback cleanup required");
 }
}
