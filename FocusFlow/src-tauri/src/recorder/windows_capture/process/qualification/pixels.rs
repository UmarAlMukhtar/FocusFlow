//! Fixed-size raw BGRA observations; never an acceptance policy or retained image.
use serde::Serialize;
#[derive(Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub(super) struct Pixels {
 pub marker_rgb:[[[u8;3];16];4],pub marker_words:[Option<u16>;4],pub corner_rgb:[[u8;3];4],
 pub content_pixels:usize,pub nonzero_rgb_pixels:usize,pub all_zero_rgb:bool,pub rgb_min:[u8;3],pub rgb_max:[u8;3],
}
pub(super) fn observe(bytes:&[u8],width:usize,height:usize,stride:usize)->Result<Pixels,&'static str>{
 if width<160||height<120||stride<width||bytes.len()>16*1024*1024||stride.checked_mul(height).and_then(|n|n.checked_mul(4)).is_none_or(|n|n>bytes.len()){return Err("Rejected pixel observation bounds invalid");}
 let rgb=|x:usize,y:usize|{let n=(y*stride+x)*4;[bytes[n+2],bytes[n+1],bytes[n]]};
 let mut result=Pixels{marker_rgb:[[[0;3];16];4],marker_words:[Some(0);4],corner_rgb:[[0;3];4],content_pixels:width*height,nonzero_rgb_pixels:0,all_zero_rgb:false,rgb_min:[255;3],rgb_max:[0;3]};
 for (row,y) in [44,60,76,92].into_iter().enumerate(){for bit in 0..16{
  let c=rgb(20+bit*8,y);result.marker_rgb[row][bit]=c;
  if c[0]!=c[1]||c[1]!=c[2]||(70..=180).contains(&c[0]){result.marker_words[row]=None;}else if c[0]>180{if let Some(word)=&mut result.marker_words[row]{*word|=1<<bit;}}
 }}
 for (i,(x,y)) in [(8,8),(width-8,8),(8,height-8),(width-8,height-8)].into_iter().enumerate(){result.corner_rgb[i]=rgb(x,y);}
 for y in 0..height{for x in 0..width{let c=rgb(x,y);if c!=[0;3]{result.nonzero_rgb_pixels+=1;}for k in 0..3{result.rgb_min[k]=result.rgb_min[k].min(c[k]);result.rgb_max[k]=result.rgb_max[k].max(c[k]);}}}
 result.all_zero_rgb=result.nonzero_rgb_pixels==0;Ok(result)
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test] fn zero_rgb_ignores_alpha_and_padding_but_finds_one_nonzero_pixel(){
  let (w,h,stride)=(161,121,162);let mut b=vec![255;stride*122*4];
  for y in 0..h{for x in 0..w{b[(y*stride+x)*4..(y*stride+x)*4+3].fill(0);}}
  let zero=observe(&b,w,h,stride).unwrap();assert_eq!(zero.nonzero_rgb_pixels,0);assert!(zero.all_zero_rgb);assert_eq!(zero.content_pixels,19481);assert_eq!(zero.rgb_min,[0;3]);assert_eq!(zero.rgb_max,[0;3]);
  b[(100*stride+150)*4+2]=7;let one=observe(&b,w,h,stride).unwrap();assert_eq!(one.nonzero_rgb_pixels,1);assert!(!one.all_zero_rgb);assert_eq!(one.rgb_max,[7,0,0]);assert_eq!(one.marker_words,[Some(0);4]);
 }
 #[test] fn raw_replicas_and_rgb_corners_survive_invalid_identity(){
  let mut b=vec![0;160*120*4];
  for (row,word) in [(44,0x1234u16),(60,0xabcd),(76,0x4321),(92,0xdcba)]{for bit in 0..16{let n=(row*160+20+bit*8)*4;b[n..n+3].fill(if word&(1<<bit)!=0{240}else{12});}}
  b[(8*160+8)*4..(8*160+8)*4+3].copy_from_slice(&[1,2,3]);
  let p=observe(&b,160,120,160).unwrap();assert_eq!(p.marker_words,[Some(0x1234),Some(0xabcd),Some(0x4321),Some(0xdcba)]);assert_eq!(p.corner_rgb[0],[3,2,1]);
  b[(44*160+20)*4]=80;let p=observe(&b,160,120,160).unwrap();assert_eq!(p.marker_words[0],None);assert_eq!(p.marker_rgb[0][0],[12,12,80]);
  assert!(serde_json::to_vec(&p).unwrap().len()<2048);assert!(std::mem::size_of::<Pixels>()<512);
 }
 #[test] fn invalid_bounds_are_errors_without_pixel_reads(){
  for (b,w,h,s) in [(&[][..],160,120,160),(&[0;32][..],159,120,160),(&[][..],160,119,160),(&[][..],160,120,159),(&[][..],usize::MAX,120,usize::MAX)]{assert!(observe(b,w,h,s).is_err());}
 }
}
