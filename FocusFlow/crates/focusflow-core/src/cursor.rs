//! Bounded cursor assets and a causal source-history evaluator shared by every renderer.
use std::{collections::HashMap, fs::{self,File,Metadata}, io::{BufRead,BufReader,Read}, path::{Path,PathBuf}, sync::{Arc,atomic::{AtomicBool,Ordering}}};
use serde::{Deserialize,Serialize};
use crate::{project::{self,invalid,require,CursorMode,CursorStyle,ProjectDocument}, store::ProjectCapabilities, CoreError,CoreResult,ErrorCode};
pub const MAX_INDEX_BYTES:u64=64*1024;
pub const MAX_SAMPLES_BYTES:u64=128*1024*1024;
pub const MAX_RECORD_BYTES:usize=1024;
pub const MAX_SAMPLES:usize=1_000_000;
pub const MAX_SHAPES:usize=256;
pub const MAX_NATIVE_BYTES:usize=32*1024*1024;
pub const MAX_HEADER_BYTES:usize=256;
pub const STALE_US:u64=250_000;
pub const ARROW_GEOMETRY:(u32,u32,u32,u32)=(32,48,2,2);
#[derive(Debug,Clone,Deserialize,Serialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct CursorIndex { pub schema_version:u32,pub width:u32,pub height:u32,pub duration_us:u64,pub samples_path:String,pub shapes:Vec<CursorShape> }
#[derive(Debug,Clone,Deserialize,Serialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct CursorShape { pub id:String,pub path:String,pub width:u32,pub height:u32,pub hotspot_x:u32,pub hotspot_y:u32 }
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct WireSample { at_us:u64,x:f64,y:f64,visible:bool,#[serde(deserialize_with="required_shape")] shape_id:Option<String>,discontinuity:bool }
fn required_shape<'de,D:serde::Deserializer<'de>>(de:D)->Result<Option<String>,D::Error> { Option::deserialize(de) }
#[derive(Debug,Clone,Copy)]
struct Sample {at_us:u64,x:f64,y:f64,shape:u16,visible:bool,discontinuity:bool}
const _:()=assert!(std::mem::size_of::<Sample>()<=48);
const _:()=assert!(std::mem::size_of::<[f64;2]>()*MAX_SAMPLES<=16*1024*1024);
const NO_SHAPE:u16=u16::MAX;
#[derive(Debug,Clone)]
pub struct PreparedShape { pub width:u32,pub height:u32,pub hotspot_x:u32,pub hotspot_y:u32,pub pixels:Arc<Vec<u8>> }
#[derive(Debug,Clone)]
pub struct PreparedCursor {
    samples:Arc<Vec<Sample>>,endpoints:Arc<Vec<[f64;2]>>,visible_indices:Arc<Vec<u32>>,duration_us:u64,tau_us:u64,
    pub shapes:Arc<Vec<PreparedShape>>,pub scale:f64,pub style:CursorStyle,
    /// One aggregate warning; individual frames do not repeat it.
    pub has_hidden_or_stale_gaps:bool,
}
#[derive(Debug,Clone,Copy,PartialEq)]
pub struct EvaluatedCursor {pub x:f64,pub y:f64,pub shape_index:usize}
#[derive(Debug,Clone)]
pub enum CursorAssetReport {
    Unchecked,
    Unavailable(CoreError),
    Available {usable_positions:bool,native_available:bool,native_unavailable_reason:Option<String>},
}

pub fn capabilities(doc:&ProjectDocument,report:&CursorAssetReport)->ProjectCapabilities {
    let mut result=ProjectCapabilities::default();
    let reason=match doc.video.cursor {
        CursorMode::Embedded=>"Recordings contain an embedded cursor and no separate continuous cursor telemetry".into(),
        CursorMode::Absent=>"This source has no separate cursor telemetry".into(),
        CursorMode::Separate=>match report {
            CursorAssetReport::Unchecked=>"Open the project to validate cursor assets".into(),
            CursorAssetReport::Unavailable(error)=>format!("Cursor assets unavailable: {}",error.message),
            CursorAssetReport::Available {usable_positions,native_available,native_unavailable_reason}=>{
                result.cursor_follow=*usable_positions;
                result.cursor_replacement=true;result.cursor_smoothing=true;result.cursor_native_style=*native_available;
                native_unavailable_reason.clone().unwrap_or_default()
            }
        }
    };
    result.cursor_unavailable_reason=bounded(reason,1024);result
}
pub fn inspect_cursor(root:&Path,doc:&ProjectDocument)->CursorAssetReport {
    inspect(root,doc,false)
}
/// Initial capture authority must account for every declared image, including unused ones.
pub(crate) fn inspect_capture_cursor(root:&Path,doc:&ProjectDocument)->CursorAssetReport {
    inspect(root,doc,true)
}
fn inspect(root:&Path,doc:&ProjectDocument,all_declared_images:bool)->CursorAssetReport {
    if doc.video.cursor!=CursorMode::Separate {return CursorAssetReport::Unchecked;}
    match load(root,doc,&||Ok(false),true,all_declared_images) {
        Ok((prepared,reason))=>CursorAssetReport::Available {usable_positions:!prepared.visible_indices.is_empty(),native_available:reason.is_none(),native_unavailable_reason:reason},
        Err(error)=>CursorAssetReport::Unavailable(error),
    }
}
/// Call after job reservation. Preparation owns immutable bytes; evaluation never reopens paths.
pub fn prepare_cursor(root:&Path,doc:&ProjectDocument,cancel:&AtomicBool)->CoreResult<Option<Arc<PreparedCursor>>> {
    prepare_cursor_with_cancel(root,doc,&||Ok(cancel.load(Ordering::Relaxed)))
}
/// The reserved worker can observe local flags and cross-process cancellation files together.
pub fn prepare_cursor_with_cancel(root:&Path,doc:&ProjectDocument,cancel:&impl Fn()->CoreResult<bool>)->CoreResult<Option<Arc<PreparedCursor>>> {
    project::validate(doc)?;
    if !doc.cursor_settings.enabled && !doc.zooms.iter().any(|z|z.mode==project::ZoomMode::Follow) {return Ok(None);}
    cancelled(cancel)?;
    let (prepared,reason)=load(root,doc,cancel,false,false)?;
    if doc.cursor_settings.enabled && doc.cursor_settings.style==CursorStyle::Native {
        if let Some(reason)=reason {return Err(invalid("cursor",reason));}
    }
    Ok(Some(Arc::new(prepared)))
}

impl PreparedCursor {
    /// Exact rational source presentation time. Invalid rates safely produce no cursor.
    pub fn sample_at(&self,source_numerator:u128,rate:u32)->Option<EvaluatedCursor> {
        let (i,[x,y])=self.fresh_position(source_numerator,rate)?;
        let shape_index=if self.style==CursorStyle::Arrow {0} else {self.samples[i].shape as usize};
        self.shapes.get(shape_index).filter(|s|!s.pixels.is_empty())?;
        Some(EvaluatedCursor{x,y,shape_index})
    }
    pub fn position_at(&self,source_numerator:u128,rate:u32)->Option<[f64;2]> {
        self.fresh_position(source_numerator,rate).map(|(_,p)|p)
    }
    fn fresh_position(&self,source_numerator:u128,rate:u32)->Option<(usize,[f64;2])> {
        if rate==0 || source_numerator>=u128::from(self.duration_us)*u128::from(rate) {return None;}
        let rate=u128::from(rate);
        let i=self.samples.partition_point(|s|u128::from(s.at_us)*rate<=source_numerator).checked_sub(1)?;
        let s=self.samples[i];
        if !s.visible || source_numerator>=u128::from(s.at_us+STALE_US)*rate {return None;}
        Some((i,position(s,self.endpoints[i],(source_numerator-u128::from(s.at_us)*rate) as f64/rate as f64,self.tau_us)))
    }
    /// Hold the eligible visible run's analytic left limit, never a hidden sentinel.
    pub fn follow_focus_at(&self,source_numerator:u128,rate:u32,entry_us:u64,fallback:[f64;2])->[f64;2] {
        let clamp=|p:[f64;2]|[p[0].clamp(0.0,1.0),p[1].clamp(0.0,1.0)];
        if let Some(p)=self.position_at(source_numerator,rate) {return clamp(p);}
        if rate==0 {return fallback;}
        let rate=u128::from(rate);
        let end=self.visible_indices.partition_point(|i|u128::from(self.samples[*i as usize].at_us)*rate<=source_numerator);
        if let Some(i)=end.checked_sub(1).map(|j|self.visible_indices[j] as usize) {
            let s=self.samples[i];
            let until=(s.at_us+STALE_US).min(self.duration_us).min(self.samples.get(i+1).map_or(self.duration_us,|s|s.at_us));
            if until>entry_us && u128::from(until)*rate<=source_numerator {
                return clamp(position(s,self.endpoints[i],(until-s.at_us) as f64,self.tau_us));
            }
        }
        fallback
    }
    pub fn sample_count(&self)->usize {self.samples.len()}
    pub fn buffer_bytes(&self)->usize {
        self.samples.capacity()*std::mem::size_of::<Sample>()+self.endpoints.capacity()*16+self.visible_indices.capacity()*4+self.shapes.iter().map(|s|s.pixels.capacity()).sum::<usize>()
    }
}
/// Global output frame index on the cumulative clip grid, never window-local time.
pub fn source_frame_time(clip:&crate::edits::MappedClip,global_frame:u64,rate:u32)->CoreResult<u128> {
    require(rate>0,"fps","Frame rate must be positive")?;
    let source=u128::from(clip.source_start_us)*u128::from(rate)+u128::from(global_frame)*1_000_000;
    source.checked_sub(u128::from(clip.output_start_us)*u128::from(rate)).ok_or_else(||invalid("frame","Frame precedes this clip"))
}
fn position(s:Sample,start:[f64;2],dt:f64,tau:u64)->[f64;2] {
    if tau==0 {return [s.x,s.y];}
    let decay=(-dt/tau as f64).exp();[s.x+(start[0]-s.x)*decay,s.y+(start[1]-s.y)*decay]
}
fn load(root:&Path,doc:&ProjectDocument,cancel:&impl Fn()->CoreResult<bool>,inspect:bool,all_declared_images:bool)->CoreResult<(PreparedCursor,Option<String>)> {
    project::validate(doc)?;
    require(doc.video.cursor==CursorMode::Separate,"cursor","Source does not have separate cursor provenance")?;
    let reference=doc.cursor.as_ref().ok_or_else(||invalid("cursor","Missing cursor index reference"))?;
    let index_path=resolve(root,&reference.index_path)?.ok_or_else(||invalid("cursor","Cursor index file is missing"))?;
    let index=read_stable(&index_path,MAX_INDEX_BYTES,|file| {
        let mut bytes=Vec::new();file.take(MAX_INDEX_BYTES+1).read_to_end(&mut bytes).map_err(io_error)?;
        require(bytes.len() as u64<=MAX_INDEX_BYTES,"cursor.index","Cursor index exceeds 64 KiB")?;
        serde_json::from_slice::<CursorIndex>(&bytes).map_err(|_|invalid("cursor.index","Malformed cursor index: required fields, types or unknown fields"))
    })?;
    require(index.schema_version==1,"cursor.index.schemaVersion","Only cursor index schema 1 is supported")?;
    require((index.width,index.height,index.duration_us)==(doc.video.width,doc.video.height,doc.video.duration_us),"cursor.index","Cursor geometry and duration must equal the video")?;
    require(index.shapes.len()<=MAX_SHAPES,"cursor.shapes","At most 256 cursor shapes are supported")?;
    let mut ids=HashMap::new();let mut shape_paths=Vec::new();let mut decoded=0usize;
    for (i,shape) in index.shapes.iter().enumerate() {
        require(project::valid_id(&shape.id) && ids.insert(shape.id.as_str(),i as u16).is_none(),"cursor.shapes.id","Shape IDs must be valid and unique")?;
        require((1..=512).contains(&shape.width)&&(1..=512).contains(&shape.height),"cursor.shapes","Native image dimensions must be between 1 and 512")?;
        require(shape.hotspot_x<shape.width&&shape.hotspot_y<shape.height,"cursor.shapes.hotspot","Cursor hotspot must lie inside its image")?;
        decoded=decoded.checked_add(shape.width as usize*shape.height as usize*4).ok_or_else(||invalid("cursor.shapes","Native payload size overflow"))?;
        require(decoded<=MAX_NATIVE_BYTES,"cursor.shapes","Decoded cursor images exceed 32 MiB")?;
        shape_paths.push(resolve(root,&shape.path)?);
    }
    let samples_path=resolve(root,&index.samples_path)?.ok_or_else(||invalid("cursor.samples","Cursor samples file is missing"))?;
    let samples=read_stable(&samples_path,MAX_SAMPLES_BYTES,|file| {
        let mut reader=BufReader::new(file);let mut line=Vec::with_capacity(MAX_RECORD_BYTES+1);let mut samples=Vec::<Sample>::new();let mut total=0u64;
        loop {
            if samples.len()%4096==0 {cancelled(cancel)?;}
            line.clear();let n=(&mut reader).take((MAX_RECORD_BYTES+1) as u64).read_until(b'\n',&mut line).map_err(io_error)?;
            if n==0 {break;}
            total+=n as u64;
            require(total<=MAX_SAMPLES_BYTES,"cursor.samples","Cursor samples exceed 128 MiB")?;
            require(n<=MAX_RECORD_BYTES,"cursor.samples","Cursor record exceeds 1024 bytes including newline")?;
            require(samples.len()<MAX_SAMPLES,"cursor.samples","At most 1000000 cursor samples are supported")?;
            let wire:WireSample=serde_json::from_slice(&line).map_err(|_|invalid("cursor.samples",format!("Malformed cursor record {}: required fields, types or unknown fields",samples.len()+1)))?;
            require(wire.at_us<index.duration_us && samples.last().is_none_or(|s|s.at_us<wire.at_us),"cursor.samples.atUs","Sample times must strictly increase inside the source duration")?;
            require(wire.x.is_finite()&&wire.y.is_finite()&&(-2048.0..=index.width as f64+2048.0).contains(&(wire.x*index.width as f64))&&(-2048.0..=index.height as f64+2048.0).contains(&(wire.y*index.height as f64)),"cursor.samples.position","Cursor position exceeds the signed 2048-pixel source margin")?;
            let shape=match wire.shape_id {Some(id)=>*ids.get(id.as_str()).ok_or_else(||invalid("cursor.samples.shapeId","Sample references an unknown shape ID"))?,None=>NO_SHAPE};
            if samples.len()==samples.capacity() {samples.reserve_exact((MAX_SAMPLES-samples.len()).min(4096));}
            samples.push(Sample{at_us:wire.at_us,x:wire.x,y:wire.y,shape,visible:wire.visible,discontinuity:wire.discontinuity});
        }
        require(!samples.is_empty(),"cursor.samples","Cursor requires at least one sample")?;Ok(samples)
    })?;
    let mut reason=if samples.iter().any(|s|s.visible&&s.shape==NO_SHAPE) {Some("Native cursor images are unavailable for position-only observations; choose arrow explicitly".into())} else {None};
    let mut shapes=Vec::with_capacity(index.shapes.len());
    let mut visible_shapes=vec![false;index.shapes.len()];
    for sample in &samples {if sample.visible && sample.shape!=NO_SHAPE {visible_shapes[sample.shape as usize]=true;}}
    let need_native=inspect || (doc.cursor_settings.enabled && doc.cursor_settings.style==CursorStyle::Native);
    if need_native {
        for (i,(shape,path)) in index.shapes.iter().zip(shape_paths).enumerate() {
            cancelled(cancel)?;
            let pixels=match path {
                Some(path)=>read_stable(&path,MAX_HEADER_BYTES as u64+shape.width as u64*shape.height as u64*4,|file|read_pam(file,shape)),
                None=>Err(invalid("cursor.shapes","Native cursor image is missing")),
            };
            match pixels {
                Ok(pixels)=>shapes.push(PreparedShape{width:shape.width,height:shape.height,hotspot_x:shape.hotspot_x,hotspot_y:shape.hotspot_y,pixels:Arc::new(pixels)}),
                Err(error)=>{
                    if (all_declared_images || visible_shapes[i]) && reason.is_none() {reason=Some(bounded(format!("Native cursor unavailable: {}; choose arrow explicitly",error.message),1024));}
                    shapes.push(PreparedShape{width:shape.width,height:shape.height,hotspot_x:shape.hotspot_x,hotspot_y:shape.hotspot_y,pixels:Arc::new(Vec::new())});
                }
            }
        }
    }
    if doc.cursor_settings.enabled && doc.cursor_settings.style==CursorStyle::Arrow {shapes=vec![arrow()];}
    let mut endpoints=Vec::with_capacity(samples.len());
    for (i,s) in samples.iter().copied().enumerate() {
        if i%4096==0 {cancelled(cancel)?;}
        let reset=i==0 || !s.visible || s.discontinuity || !samples[i-1].visible || s.at_us-samples[i-1].at_us>STALE_US;
        endpoints.push(if reset {[s.x,s.y]} else {position(samples[i-1],endpoints[i-1],(s.at_us-samples[i-1].at_us) as f64,doc.cursor_settings.smoothing_us)});
    }
    let mut visible_indices=Vec::with_capacity(samples.iter().filter(|s|s.visible).count());
    for (i,s) in samples.iter().enumerate() {if s.visible {visible_indices.push(i as u32);}}
    require(visible_indices.capacity()*std::mem::size_of::<u32>()<=4*1024*1024,"cursor.samples","Visible index exceeds 4 MiB")?;
    let gaps=samples[0].at_us>0||samples.iter().any(|s|!s.visible)||samples.windows(2).any(|s|s[1].at_us-s[0].at_us>STALE_US)||samples.last().unwrap().at_us+STALE_US<index.duration_us;
    Ok((PreparedCursor{samples:Arc::new(samples),endpoints:Arc::new(endpoints),visible_indices:Arc::new(visible_indices),duration_us:index.duration_us,tau_us:doc.cursor_settings.smoothing_us,shapes:Arc::new(shapes),scale:doc.cursor_settings.scale,style:doc.cursor_settings.style,has_hidden_or_stale_gaps:gaps},reason))
}
fn read_pam(file:&mut File,shape:&CursorShape)->CoreResult<Vec<u8>> {
    let mut reader=BufReader::new(file);let mut header=Vec::new();let mut keys=HashMap::new();
    loop {
        let mut line=Vec::new();let remaining=MAX_HEADER_BYTES+1-header.len();
        let n=(&mut reader).take(remaining as u64).read_until(b'\n',&mut line).map_err(io_error)?;
        require(n>0,"cursor.image","Truncated PAM header")?;header.extend_from_slice(&line);
        require(header.len()<=MAX_HEADER_BYTES,"cursor.image","PAM header exceeds 256 bytes")?;
        if header.len()==line.len() {require(line==b"P7\n","cursor.image","Expected canonical P7 PAM")?;continue;}
        if line==b"ENDHDR\n" {break;}
        require(line.is_ascii(),"cursor.image","PAM header must be ASCII")?;
        let text=std::str::from_utf8(&line).map_err(|_|invalid("cursor.image","PAM header must be ASCII"))?;
        let mut parts=text.split_whitespace();let key=parts.next().unwrap_or("");let value=parts.next().unwrap_or("");
        require(matches!(key,"WIDTH"|"HEIGHT"|"DEPTH"|"MAXVAL"|"TUPLTYPE")&&!value.is_empty()&&parts.next().is_none()&&keys.insert(key.to_owned(),value.to_owned()).is_none(),"cursor.image","Unknown, duplicate or malformed PAM header field")?;
    }
    for (key,value) in [("WIDTH",shape.width.to_string()),("HEIGHT",shape.height.to_string()),("DEPTH","4".into()),("MAXVAL","255".into()),("TUPLTYPE","RGB_ALPHA".into())] {
        require(keys.get(key)==Some(&value),"cursor.image","PAM must match descriptor geometry and straight RGBA8 tuple")?;
    }
    let mut pixels=vec![0;shape.width as usize*shape.height as usize*4];reader.read_exact(&mut pixels).map_err(|_|invalid("cursor.image","Truncated PAM payload"))?;
    let mut extra=[0];require(reader.read(&mut extra).map_err(io_error)?==0,"cursor.image","PAM has trailing bytes")?;Ok(pixels)
}
fn resolve(root:&Path,relative:&str)->CoreResult<Option<PathBuf>> {
    if relative.len()>512 || !project::valid_asset_name(relative) {return Err(CoreError{code:ErrorCode::InvalidPath,message:"Cursor paths must be safe relative asset paths of at most 512 UTF-8 bytes".into(),field:Some("cursor.path".into())});}
    match crate::paths::asset_path(root,relative) {
        Ok(path)=>Ok(Some(path)),
        Err(error) if error.code==ErrorCode::NotFound=>{crate::store::validate_missing_asset(root,relative)?;Ok(None)},
        Err(error)=>Err(error),
    }
}
fn same_metadata(a:&Metadata,b:&Metadata)->bool {
    let same=a.len()==b.len()&&a.modified().ok()==b.modified().ok()&&a.created().ok()==b.created().ok();
    #[cfg(unix)] {use std::os::unix::fs::MetadataExt;same&&a.dev()==b.dev()&&a.ino()==b.ino()}
    #[cfg(not(unix))] {same}
}
fn read_stable<T>(path:&Path,max:u64,read:impl FnOnce(&mut File)->CoreResult<T>)->CoreResult<T> {
    let mut file=File::open(path).map_err(io_error)?;let before=file.metadata().map_err(io_error)?;
    require(before.len()<=max,"cursor","Cursor asset exceeds its byte limit")?;
    let result=read(&mut file)?;
    require(same_metadata(&before,&file.metadata().map_err(io_error)?)&&same_metadata(&before,&fs::metadata(path).map_err(io_error)?),"cursor","Cursor asset changed during preparation; retry after capture finishes")?;
    Ok(result)
}
fn cancelled(cancel:&impl Fn()->CoreResult<bool>)->CoreResult<()> {
    if cancel()? {Err(CoreError{code:ErrorCode::Cancelled,message:"Cursor preparation cancelled".into(),field:None})} else {Ok(())}
}
fn io_error(error:std::io::Error)->CoreError {invalid("cursor",format!("Could not read cursor asset: {error}"))}
fn bounded(mut text:String,max:usize)->String {if text.len()>max {let mut end=max-3;while !text.is_char_boundary(end) {end-=1;}text.truncate(end);text.push_str("...");}text}
/// One fixed 32x48 arrow mask, with source-pixel hotspot (2,2).
fn arrow()->PreparedShape {
    let (width,height,hotspot_x,hotspot_y)=ARROW_GEOMETRY;
    let mut pixels=vec![0;width as usize*height as usize*4];
    for y in 2..44usize {for x in 2..30usize {
        let triangle=y<=32&&x<=2+(y-2)*2/3;
        let stem=(24..42).contains(&y)&&x>=11+(y-24)/3&&x<=17+(y-24)/3;
        if triangle||stem {let edge=x==2||x==2+(y-2)*2/3||y==2;let value=if edge {20} else {255};let i=(y*width as usize+x)*4;pixels[i..i+4].copy_from_slice(&[value,value,value,255]);}
    }}
    PreparedShape{width,height,hotspot_x,hotspot_y,pixels:Arc::new(pixels)}
}
/// Scale both tile geometry and hotspot using nearest integer, positive ties upward.
pub fn scaled_shape(shape:&PreparedShape,scale:f64)->CoreResult<(u32,u32,u32,u32)> {
    require(scale.is_finite()&&(0.5..=4.0).contains(&scale),"cursorSettings.scale","Cursor scale must be between 0.5 and 4")?;
    Ok(((shape.width as f64*scale).round() as u32,(shape.height as f64*scale).round() as u32,(shape.hotspot_x as f64*scale).round() as u32,(shape.hotspot_y as f64*scale).round() as u32))
}
