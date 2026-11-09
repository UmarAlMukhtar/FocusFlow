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
