use focusflow_windows_audio_worker::{format::{Encoding,Format},proof::render::{Signal,BLOCK_FRAMES}};
fn format()->Format {Format{rate:8000,channels:2,bits:16,valid_bits:16,align:4,encoding:Encoding::Pcm}}
// Literal reference is independently calculated integer xorshift at seed 1, frame 2400..2402.
#[test] fn literal_channel_order_zeros_and_block_independence() {
 let mut s=Signal::new(format(),1).unwrap();
 assert_eq!(s.block(1024).unwrap(),vec![0;4096]);assert_eq!(s.block(1024).unwrap(),vec![0;4096]);assert_eq!(s.block(352).unwrap(),vec![0;1408]);
 let b=s.block(3).unwrap();let samples:Vec<_>=b.chunks_exact(2).map(|b|i16::from_le_bytes([b[0],b[1]])).collect();assert_eq!(samples,[-514,-6173,-4369,-2832,7196,1539]);
 let mut other=Signal::new(format(),1).unwrap();let mut all=Vec::new();for _ in 0..3{all.extend(other.block(801).unwrap());}assert_eq!(&all[2400*4..],b);
}
#[test] fn finite_twelve_seconds_unequal_bursts_and_bounded_allocations() {
 let mut s=Signal::new(format(),1).unwrap();let mut frames=0;let mut active=Vec::new();let mut run=0;
 for _ in 0..94 {let block=s.block(BLOCK_FRAMES).unwrap();assert!(block.len()<=4096);for bytes in block.chunks_exact(4){let left=i16::from_le_bytes([bytes[0],bytes[1]]);let right=i16::from_le_bytes([bytes[2],bytes[3]]);assert!(left.abs()<=8192&&right.abs()<=8192);if left!=0||right!=0 {run+=1;}else if run>0{active.push(run);run=0;}frames+=1;}}
 assert_eq!(frames,96000);assert_eq!(s.remaining(),0);assert_eq!(active,vec![3656,5848,10056,6024,15032,14896]);assert!(s.block(1).unwrap().is_empty());
 let mut s=Signal::new(format(),1).unwrap();assert!(s.block(1025).is_err());assert!(s.block(0).is_err());assert_eq!(s.remaining(),96000);
}
#[test] fn nonperiodic_seeded_sequence_and_all_supported_encodings() {
 for (encoding,bits) in [(Encoding::Pcm,16),(Encoding::Pcm,24),(Encoding::Pcm,32),(Encoding::Float,32)] {
  let mut f=format();f.encoding=encoding;f.bits=bits;f.valid_bits=bits;f.align=2*(bits/8);
  let mut a=Signal::new(f,1).unwrap();let mut b=Signal::new(f,2).unwrap();for n in [1024,1024,352]{a.block(n).unwrap();b.block(n).unwrap();}
  let bytes=a.block(1024).unwrap();assert_ne!(bytes,b.block(1024).unwrap());let samples=f.decode(&bytes).unwrap();assert!(samples.iter().all(|x|x.is_finite()&&x.abs()<=0.25));for lag in [1,2,16,64,127,256] {assert_ne!(&samples[..512],&samples[lag*2..lag*2+512]);}assert!(samples.chunks_exact(2).any(|v|v[0]!=v[1]));
 }
 let mut bad=format();bad.rate=u32::MAX;assert!(Signal::new(bad,1).is_err());
}
