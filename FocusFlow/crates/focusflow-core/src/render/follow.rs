//! Symbolic focus on the shared camera schedule; source telemetry is prepared once.
use serde::{Deserialize,Serialize};
use std::sync::Arc;
use crate::{cursor::PreparedCursor,project::{invalid,require},CoreResult};
use super::{CameraPose,RenderPlan,CENTER,MAX_CAMERA_INTERVALS};
const MAX_BYTES:usize=16*1024*1024;
#[derive(Debug,Clone,Copy,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub enum Endpoint {PreviousBoundary,Fixed{x:f64,y:f64},Follow{zoom_index:usize}}
impl Endpoint {
 pub(super) fn fixed(p:CameraPose)->Self {Self::Fixed{x:p.x,y:p.y}}
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Target {pub entry_us:u64,pub fallback:[f64;2]}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Phase {
 pub clip_index:usize,pub start_us:u64,pub end_us:u64,pub source_start_us:u64,pub output_start_us:u64,
 pub from:Endpoint,pub to:Endpoint,pub weight:f64,pub scale_from:f64,pub scale_to:f64,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FollowPlan {pub phases:Vec<Phase>,pub targets:Vec<Target>}
impl FollowPlan {
 fn bytes(&self)->usize {self.phases.capacity()*std::mem::size_of::<Phase>()+self.targets.capacity()*std::mem::size_of::<Target>()}
 pub(super) fn check(&self)->CoreResult<()> {
  require(self.phases.len()<=MAX_CAMERA_INTERVALS,"render.camera","Follow exceeds 65536 camera phases")?;
  require(self.bytes()+self.phases.len()*std::mem::size_of::<[f64;2]>()<=MAX_BYTES,"render.follow","Follow metadata exceeds 16 MiB")
 }
}
#[derive(Debug,Clone)]
pub struct PreparedFocus {bound:Arc<FollowPlan>,anchors:Arc<Vec<[f64;2]>>}
impl PreparedFocus {
 pub fn prepare(plan:&RenderPlan,cursor:&PreparedCursor,cancel:&impl Fn()->CoreResult<bool>)->CoreResult<Option<Arc<Self>>> {
  let Some(symbolic)=&plan.follow else {return Ok(None)};
  symbolic.check()?;
  let mut prepared=Self{bound:Arc::new(symbolic.clone()),anchors:Arc::new(Vec::new())};
  let mut anchors=Vec::with_capacity(symbolic.phases.len());
  let mut previous=[CENTER.x,CENTER.y];
  for phase in &symbolic.phases {
   if cancel()? {return Err(crate::ffmpeg::cancel_error());}
   anchors.push(previous);
   previous=prepared.evaluate_phase(phase,previous,u128::from(phase.end_us),1,cursor)?.0;
  }
  prepared.anchors=Arc::new(anchors);
  require(prepared.buffer_bytes()<=MAX_BYTES,"render.follow","Prepared focus exceeds 16 MiB")?;
  Ok(Some(Arc::new(prepared)))
 }
 pub fn validate(&self,plan:&RenderPlan)->CoreResult<()> {
  require(plan.follow.as_ref()==Some(self.bound.as_ref()),"render.follow","Prepared focus does not match the immutable camera plan")
 }
 pub fn buffer_bytes(&self)->usize {self.bound.bytes()+self.anchors.capacity()*std::mem::size_of::<[f64;2]>()+std::mem::size_of::<Self>()+std::mem::size_of::<FollowPlan>()+4*std::mem::size_of::<usize>()}
 /// Exact global output microsecond numerator/rate, independent of window or preview starts.
 pub fn pose_at(&self,numerator:u128,rate:u32,cursor:&PreparedCursor)->CoreResult<CameraPose> {
  require(rate>0,"fps","Frame rate must be positive")?;
  let index=self.bound.phases.partition_point(|p|u128::from(p.end_us)*u128::from(rate)<=numerator);
  let Some(phase)=self.bound.phases.get(index) else {return Ok(CENTER)};
  let (p,scale)=self.evaluate_phase(phase,self.anchors[index],numerator,rate,cursor)?;
  Ok(CameraPose{x:p[0],y:p[1],scale})
 }
 fn evaluate_phase(&self,p:&Phase,previous:[f64;2],numerator:u128,rate:u32,cursor:&PreparedCursor)->CoreResult<([f64;2],f64)> {
  let endpoint=|e:Endpoint|->CoreResult<[f64;2]> {Ok(match e {
   Endpoint::PreviousBoundary=>previous,Endpoint::Fixed{x,y}=>[x,y],
   Endpoint::Follow{zoom_index}=>{
    let target=self.bound.targets.get(zoom_index).ok_or_else(||invalid("render.follow","Unknown follow target"))?;
    let source=(numerator+u128::from(p.source_start_us)*u128::from(rate)).checked_sub(u128::from(p.output_start_us)*u128::from(rate)).ok_or_else(||invalid("render.follow","Focus time precedes source"))?;
    cursor.follow_focus_at(source,rate,target.entry_us,target.fallback)
   }
  })};
  let from=endpoint(p.from)?;let to=endpoint(p.to)?;
  let start=u128::from(p.start_us)*u128::from(rate);let end=u128::from(p.end_us)*u128::from(rate);
  let t=if end==start || numerator>=end {1.0} else if numerator<=start {0.0} else {ease((numerator-start) as f64/(end-start) as f64)};
  let w=t*p.weight;
  Ok(([from[0]+(to[0]-from[0])*w,from[1]+(to[1]-from[1])*w],p.scale_from+(p.scale_to-p.scale_from)*t))
 }
}
pub(super) fn ease(t:f64)->f64 {0.5-0.5*(std::f64::consts::PI*t).cos()}
