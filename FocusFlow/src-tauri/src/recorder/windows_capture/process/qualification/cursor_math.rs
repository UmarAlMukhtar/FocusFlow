//! Private proof arithmetic only; never grants capture authority or a stationary interval.
// This leaf is also compiled by portable core tests without the Tauri module tree.
#[path="../../cursor/image.rs"]
mod resource_image;
pub use resource_image::{Shape,color,monochrome};
pub fn origin(raw:&[[i32;2]],physical:&[[i32;2]])->Result<[i32;2],&'static str>{
 if raw.len()!=physical.len()||raw.len()<3||raw.len()>16{return Err("Need 3..16 independent anchors");}
 let delta=[i64::from(physical[0][0])-i64::from(raw[0][0]),i64::from(physical[0][1])-i64::from(raw[0][1])];
 let mut area=false;
 for i in 1..raw.len(){for j in i+1..raw.len(){
  let x=i64::from(raw[i][0])-i64::from(raw[0][0]);let y=i64::from(raw[i][1])-i64::from(raw[0][1]);
  let u=i64::from(raw[j][0])-i64::from(raw[0][0]);let v=i64::from(raw[j][1])-i64::from(raw[0][1]);area|=i128::from(x)*i128::from(v)!=i128::from(y)*i128::from(u);
 }}
 if !area||raw.iter().zip(physical).any(|(r,p)|[i64::from(p[0])-i64::from(r[0]),i64::from(p[1])-i64::from(r[1])]!=delta){return Err("Anchors do not prove unit translation");}
 Ok([i32::try_from(delta[0]).map_err(|_|"Origin overflow")?,i32::try_from(delta[1]).map_err(|_|"Origin overflow")?])
}
pub fn position(physical:[i32;2],origin:[i32;2],raw:[u32;2],encoded:[u32;2])->Result<[f64;2],&'static str>{
 if raw.contains(&0)||Some(encoded[0])!=raw[0].checked_add(raw[0]%2)||Some(encoded[1])!=raw[1].checked_add(raw[1]%2){return Err("Invalid padding");}
 let p=[i64::from(physical[0])-i64::from(origin[0]),i64::from(physical[1])-i64::from(origin[1])];
 if p[0]<0||p[1]<0||p[0]>=i64::from(raw[0])||p[1]>=i64::from(raw[1]){return Err("Outside raw content");}
 Ok([p[0] as f64/f64::from(encoded[0]),p[1] as f64/f64::from(encoded[1])])
}
pub fn timestamp(q:[i64;2],frequency:i64,epoch:i64,admission:i64,stop:i64,frames:u32)->Result<i64,&'static str>{
 if frequency<=0||epoch<0||admission<0||q[0]<admission||q[1]<q[0]||stop<q[1]||frames==0{return Err("Invalid bracket");}
 let f=i128::from(frequency);let e=i128::from(epoch);let lo=i128::from(q[0]);let hi=i128::from(q[1]);
 if (hi-lo)*30>f||lo*10_000_000<e*f{return Err("Pre-epoch or overwide bracket");}
 // All inputs are i64; the triple product is checked before multiplying microseconds.
 let relative=|v:i128|v.checked_mul(10_000_000).and_then(|v|v.checked_sub(e.checked_mul(f)?)).ok_or("Clock overflow");
 let end=i128::from(frames)*f*10_000_000;
 if relative(hi)?.checked_mul(30).ok_or("Clock overflow")?>=end||q[1]>=stop{return Err("Bracket crosses saved half-open end");}
 let n=relative(lo+(hi-lo)/2)?.checked_mul(1_000_000).ok_or("Clock overflow")?;
 let us=n/(f*10_000_000);if us<0||us>=i128::from(frames)*1_000_000/30{return Err("Outside saved microsecond end");}
 i64::try_from(us).map_err(|_|"Clock overflow")
}
/// No polling sequence alone establishes a historical stationary boundary.
#[derive(Default)] pub struct Prefix {ended:bool}
impl Prefix {pub fn ambiguous(&mut self){self.ended=true;}pub fn accept(&self,boundary:Option<i64>,at:i64)->bool{!self.ended&&boundary.is_some_and(|end|at<end)}}
#[derive(Debug,Clone,Copy,PartialEq,Eq)] pub struct Sample {pub at:i64,pub hidden:bool,pub discontinuity:bool}
pub fn coalesce(samples:&mut Vec<Sample>,sample:Sample)->Result<(),&'static str>{
 if samples.len()>=4096{return Err("Observation cap reached");}
 if let Some(last)=samples.last_mut(){if sample.at<last.at{return Err("Clock regression");}if sample.at==last.at{last.hidden|=sample.hidden;last.discontinuity|=sample.discontinuity;return Ok(());}}
 samples.push(sample);Ok(())
}
#[cfg(test)] mod tests {
 use super::*;
 #[test] fn signed_unit_translation_requires_three_noncollinear_independent_anchors(){
  let raw=[[3,4],[20,4],[3,30]];assert_eq!(origin(&raw,&[[-97,-46],[-80,-46],[-97,-20]]).unwrap(),[-100,-50]);
  for bad in [[[103,54],[121,54],[103,80]],[[106,58],[140,58],[106,110]],[[103,54],[120,54],[104,80]]]{assert!(origin(&raw,&bad).is_err());}
  assert!(origin(&[[1,1],[2,2],[3,3]],&[[11,11],[12,12],[13,13]]).is_err());assert!(origin(&raw[..2],&raw[..2]).is_err());
 }
 #[test] fn rational_qpc_uses_actual_frequency_midpoint_and_half_open_endpoint(){
  assert_eq!(timestamp([3_000_000,3_000_006],3_000_000,10_000_000,2_999_000,6_000_000,30).unwrap(),1);
  assert_eq!(timestamp([10_000_000,10_000_010],10_000_000,10_000_000,9_999_000,20_000_000,30).unwrap(),0);
  for q in [[-1,0],[3_000_001,3_000_000],[2_999_999,3_000_001],[5_999_999,6_000_001],[6_000_000,6_000_000],[3_000_000,3_100_001]]{assert!(timestamp(q,3_000_000,10_000_000,2_999_000,6_000_000,30).is_err(),"{q:?}");}
  assert!(timestamp([i64::MAX-1,i64::MAX],i64::MAX,i64::MAX,0,i64::MAX,30).is_err());
  assert!(timestamp([3_000_000,3_000_000],0,10_000_000,0,6_000_000,30).is_err());
 }
 #[test] fn fresh_pixels_define_shape_identity_and_zero_alpha_is_canonical(){
  let a=color(2,1,[1,0],&[0,0,64,128,99,77,66,0]).unwrap();assert_eq!(a.rgba,[128,0,0,128,0,0,0,0]);
  let b=color(2,1,[1,0],&[0,64,0,128,99,77,66,0]).unwrap();assert_ne!(a,b,"same native handle cannot be an image identity");
  assert!(color(1,1,[1,0],&[0;4]).is_err());assert!(color(1,1,[0,0],&[0,0,0,0]).is_err());assert!(color(1,1,[0,0],&[255,0,0,1]).is_err());
 }
 #[test] fn padding_prefix_and_duplicates_never_manufacture_visibility(){
  assert_eq!(position([540,308],[-100,-50],[641,359],[642,360]).unwrap(),[640.0/642.0,358.0/360.0]);assert!(position([541,308],[-100,-50],[641,359],[642,360]).is_err());
  let mut p=Prefix::default();assert!(!p.accept(None,1));assert!(p.accept(Some(3),1));p.ambiguous();assert!(!p.accept(Some(3),1));
  let mut v=vec![];for (hidden,discontinuity) in [(false,false),(true,false),(false,true)]{coalesce(&mut v,Sample{at:1,hidden,discontinuity}).unwrap();}assert_eq!(v,vec![Sample{at:1,hidden:true,discontinuity:true}]);assert!(coalesce(&mut v,Sample{at:0,hidden:false,discontinuity:false}).is_err());
 }
 #[test] fn mask_transparency_is_supported_but_destination_inversion_is_not(){
  let s=monochrome(3,1,[1,0],&[false,false,true],&[false,true,false]).unwrap();assert_eq!(s.rgba,[0,0,0,255,255,255,255,255,0,0,0,0]);
  assert!(monochrome(1,1,[0,0],&[true],&[true]).is_err());assert!(monochrome(2,1,[0,0],&[false],&[false]).is_err());
 }
}
