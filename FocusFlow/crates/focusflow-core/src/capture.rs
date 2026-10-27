//! Strict native authority. Recording remains separate from structural publication and import.
use std::{fs, io::Read, path::{Path, PathBuf}, sync::atomic::AtomicBool};
use serde::{Deserialize, Serialize};
use crate::{CoreError, CoreResult, ErrorCode, project::{self, invalid, require, ProjectDocument, ProjectWarning, CursorMode, TimingProvenance, MAX_JSON_BYTES}, paths, store, cursor, ffmpeg};

macro_rules! literals {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
        pub enum $name { $(#[serde(rename=$value)] $variant),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D:serde::Deserializer<'de>>(de:D)->Result<Self,D::Error> {
                let value=String::deserialize(de)?;
                match value.as_str() { $($value=>Ok(Self::$variant),)+ _=>Err(serde::de::Error::unknown_variant(&value,&[$($value),+])) }
            }
        }
    };
}
pub mod windows;
pub use windows::{decode_windows, publish_windows_incomplete, validate_windows_finalized_assets, publish_windows_finalized_assets};

literals!(Backend { LinuxX11 => "linuxX11" });
literals!(SourceKind { Screen => "screen", Region => "region", Window => "window" });
literals!(DiagnosticStage { Starting => "starting", Encoding => "encoding", Cursor => "cursor", Clock => "clock", Publishing => "publishing" });
literals!(VideoPath { Screen => "screen.mp4" });
literals!(SeparateCursor { Separate => "separate" });
literals!(RawSamplesPath { Raw => "cursor/raw.ndjson" });
literals!(EndpointProvenance { SavedPacketFirstAndTail => "savedPacketFirstAndTail" });
literals!(Interval { HalfOpen => "halfOpen" });
literals!(SourceClock { UnixMicrosecondsPreImageCopyRequest => "unixMicrosecondsPreImageCopyRequest" });
literals!(Epoch { CaptureLaunchMonotonicNs => "captureLaunchMonotonicNs" });
literals!(ClockProvenance { CompletedEncoderGridAndSavedPacketCadence => "completedEncoderGridAndSavedPacketCadence" });
literals!(Interactions { DeliveryTimeRebasedOnce => "deliveryTimeRebasedOnce" });
literals!(AudioTiming { Unmeasured => "unmeasured" });
literals!(CursorClock { VideoFrameZeroRelativeUs => "videoFrameZeroRelativeUs" });
literals!(Observation { X11RequestBracketMidpoint => "x11RequestBracketMidpoint" });
literals!(InitialState { BoundedPreRollHold => "boundedPreRollHold" });
literals!(Visibility { SameScreenNativeImageOrArrow1xIntersectsCrop => "sameScreenNativeImageOrArrow1xIntersectsCrop" });
literals!(ServerVisibility { Unknown => "unknown" });

fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<Option<T>, D::Error> { Option::deserialize(de) }
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all="camelCase")]
pub struct CaptureManifestV1 {
    pub schema_version: u32,
    pub capture_id: String,
    pub source: NativeSourceV1,
    pub diagnostic: Option<CaptureDiagnosticV1>,
    pub state: CaptureStateV1,
}
impl<'de> Deserialize<'de> for CaptureManifestV1 {
    fn deserialize<D:serde::Deserializer<'de>>(de:D)->Result<Self,D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Wire {
            schema_version:u32, capture_id:String, source:NativeSourceV1,
            #[serde(deserialize_with="required")]
            diagnostic:Option<CaptureDiagnosticV1>, state:CaptureStateV1,
        }
        let UniqueJson(value)=UniqueJson::deserialize(de)?;
        // Serde named structs otherwise accept positional sequences without any field names.
        named_object_shapes(&value,&mut Vec::new()).map_err(serde::de::Error::custom)?;
        let wire:Wire=serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self{schema_version:wire.schema_version,capture_id:wire.capture_id,source:wire.source,diagnostic:wire.diagnostic,state:wire.state})
    }
}
// These four snapshot collections are the only arrays in the native manifest contract.
fn named_object_shapes<'a>(value:&'a serde_json::Value,path:&mut Vec<&'a str>)->Result<(),&'static str> {
    match value {
        serde_json::Value::Array(values)=>{
            if !matches!(path.as_slice(),["state","initialProject","clips"|"zooms"|"clicks"|"warnings"]) {
                return Err("Native named objects require JSON objects, not positional arrays");
            }
            path.push("*");
            for value in values {named_object_shapes(value,path)?;}
            path.pop();
        }
        serde_json::Value::Object(fields)=>for (key,value) in fields {
            path.push(key);named_object_shapes(value,path)?;path.pop();
        },
        _=>{},
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct NativeSourceV1 {
    pub backend: Backend,
    pub kind: SourceKind,
    pub initial_bounds: Bounds,
    #[serde(deserialize_with="required")]
    pub window_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Bounds { pub x:i32, pub y:i32, pub width:u32, pub height:u32 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct CaptureDiagnosticV1 { pub stage:DiagnosticStage, pub message:String }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag="status", rename_all="camelCase", deny_unknown_fields)]
pub enum CaptureStateV1 {
    #[serde(rename_all="camelCase")]
    Incomplete { video_path:VideoPath, video_cursor:SeparateCursor, raw_samples_path:RawSamplesPath },
    #[serde(rename_all="camelCase")]
    Finalized {
        video_timing:VideoTimingV1,
        #[serde(deserialize_with="required")]
        cursor_timing:Option<CursorTimingV1>,
        #[serde(deserialize_with="strict_project")]
        initial_project:ProjectDocument,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct VideoTimingV1 {
    pub endpoint:SavedVideoEndpointV1,
    #[serde(deserialize_with="required")]
    pub clock:Option<AcceptedX11ClockV1>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct SavedVideoEndpointV1 { pub provenance:EndpointProvenance, pub end_pts:u64, pub time_base:TimeBase, pub interval:Interval }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct TimeBase { pub numerator:u32, pub denominator:u32 }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct AcceptedX11ClockV1 {
    pub source_clock:SourceClock, pub epoch:Epoch, pub anchor:Anchor, pub completed_frames:u64,
    pub cadence:TimeBase, pub provenance:ClockProvenance, pub interactions:Interactions, pub audio:AudioTiming,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct Anchor { pub low_ns:String, pub high_exclusive_ns:String }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct CursorTimingV1 {
    pub clock:CursorClock, pub observation:Observation, pub initial_state:InitialState,
    pub visibility:Visibility, pub server_visibility:ServerVisibility,
}

// Preserve duplicate detection before native shape checks and the existing project decoder consume a Value.
struct UniqueJson(serde_json::Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D:serde::Deserializer<'de>>(de:D)->Result<Self,D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value=UniqueJson;
            fn expecting(&self,f:&mut std::fmt::Formatter)->std::fmt::Result {f.write_str("JSON with unique object keys")}
            fn visit_map<A:serde::de::MapAccess<'de>>(self,mut map:A)->Result<Self::Value,A::Error> {
                let mut object=serde_json::Map::new();
                while let Some((key,UniqueJson(value)))=map.next_entry::<String,UniqueJson>()? {
                    if object.insert(key,value).is_some() {return Err(serde::de::Error::custom("Duplicate native snapshot field"));}
                }
                Ok(UniqueJson(object.into()))
            }
            fn visit_seq<A:serde::de::SeqAccess<'de>>(self,mut seq:A)->Result<Self::Value,A::Error> {
                let mut values=Vec::new();while let Some(UniqueJson(value))=seq.next_element()? {values.push(value);}Ok(UniqueJson(values.into()))
            }
            fn visit_bool<E:serde::de::Error>(self,v:bool)->Result<Self::Value,E>{Ok(UniqueJson(v.into()))}
            fn visit_i64<E:serde::de::Error>(self,v:i64)->Result<Self::Value,E>{Ok(UniqueJson(v.into()))}
            fn visit_u64<E:serde::de::Error>(self,v:u64)->Result<Self::Value,E>{Ok(UniqueJson(v.into()))}
            fn visit_f64<E:serde::de::Error>(self,v:f64)->Result<Self::Value,E>{serde_json::Number::from_f64(v).map(|n|UniqueJson(n.into())).ok_or_else(||E::custom("Nonfinite number"))}
            fn visit_str<E:serde::de::Error>(self,v:&str)->Result<Self::Value,E>{Ok(UniqueJson(v.into()))}
            fn visit_string<E:serde::de::Error>(self,v:String)->Result<Self::Value,E>{Ok(UniqueJson(v.into()))}
            fn visit_unit<E:serde::de::Error>(self)->Result<Self::Value,E>{Ok(UniqueJson(serde_json::Value::Null))}
            fn visit_none<E:serde::de::Error>(self)->Result<Self::Value,E>{self.visit_unit()}
            fn visit_some<D:serde::Deserializer<'de>>(self,de:D)->Result<Self::Value,D::Error>{UniqueJson::deserialize(de)}
        }
        de.deserialize_any(Visitor)
    }
}
fn strict_project<'de,D:serde::Deserializer<'de>>(de:D)->Result<ProjectDocument,D::Error> {
    let UniqueJson(value)=UniqueJson::deserialize(de)?;
    if value.get("schemaVersion").and_then(|v|v.as_u64())!=Some(3)
        || value.get("cursor").is_none() || value.get("microphone").is_none()
        || value.get("timing").and_then(|v|v.get("audioOffsetUs")).is_none() {
        return Err(serde::de::Error::custom("Native snapshot requires schema 3 and explicit cursor, microphone and audioOffsetUs fields"));
    }
    // Tighten unit-enum wire types here without changing ordinary saved-project decoding.
    for path in ["/video/cursor","/cursorSettings/style","/timing/provenance","/frameStyle/canvas","/frameStyle/background/type"] {
        if !value.pointer(path).is_some_and(serde_json::Value::is_string) {
            return Err(serde::de::Error::custom("Native snapshot literal fields require JSON strings"));
        }
    }
    if !value.get("warnings").and_then(serde_json::Value::as_array).is_some_and(|warnings|warnings.iter().all(serde_json::Value::is_string)) {
        return Err(serde::de::Error::custom("Native snapshot warnings require an array of JSON strings"));
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

fn product(values:&[u128])->CoreResult<u128> {
    values.iter().try_fold(1u128,|n,value|n.checked_mul(*value).ok_or_else(||invalid("videoTiming","Native timing arithmetic overflow")))
}
fn equal_time(a:ffmpeg::PacketTime,b:ffmpeg::PacketTime)->CoreResult<bool> {
    Ok(product(&[a.ticks.into(),a.numerator.into(),b.denominator.into()])?==product(&[b.ticks.into(),b.numerator.into(),a.denominator.into()])?)
}
impl SavedVideoEndpointV1 {
    fn packet_time(&self)->ffmpeg::PacketTime {ffmpeg::PacketTime{ticks:self.end_pts,numerator:self.time_base.numerator,denominator:self.time_base.denominator}}
    pub fn duration_us(&self)->CoreResult<u64> {
        endpoint_duration_us(self.end_pts,&self.time_base)
    }
}
fn endpoint_duration_us(end_pts:u64,time_base:&TimeBase)->CoreResult<u64> {
    require(end_pts>0&&end_pts<i64::MAX as u64,"videoTiming.endpoint.endPts","Expected a positive bounded saved-video endpoint")?;
    require((1..=i32::MAX as u32).contains(&time_base.numerator)&&(1..=i32::MAX as u32).contains(&time_base.denominator),"videoTiming.endpoint.timeBase","Expected positive signed-32-bit timebase components")?;
    let micros=product(&[end_pts.into(),time_base.numerator.into(),1_000_000])?;
    require(micros<=product(&[project::MAX_DURATION_US.into(),time_base.denominator.into()])?,"videoTiming.endpoint","Exact video endpoint exceeds 24 hours")?;
    let duration=(micros/u128::from(time_base.denominator)) as u64;
    require(duration>0,"videoTiming.endpoint","Video endpoint must contain at least one microsecond")?;Ok(duration)
}

fn decimal(text:&str)->CoreResult<i128> {
    require(!text.is_empty()&&text.len()<=39&&text.bytes().all(|c|c.is_ascii_digit())&&(text=="0"||!text.starts_with('0')),"videoTiming.clock.anchor","Expected a canonical nonnegative i128 decimal string")?;
    text.parse().map_err(|_|invalid("videoTiming.clock.anchor","Anchor exceeds i128"))
}
impl CaptureManifestV1 {
    pub fn validate(&self,id:&str)->CoreResult<()> {
        if self.schema_version!=1 {return Err(CoreError{code:ErrorCode::UnsupportedSchema,message:"Only native capture schema 1 is supported".into(),field:Some("schemaVersion".into())});}
        require(project::valid_id(id)&&self.capture_id==id,"captureId","Capture ID must equal its real containing directory ID")?;
        let b=&self.source.initial_bounds;
        require((2..=16384).contains(&b.width)&&(2..=16384).contains(&b.height)&&b.width%2==0&&b.height%2==0,"source.initialBounds","Native bounds require even dimensions between 2 and 16384")?;
        require(b.x.checked_add(b.width as i32).is_some()&&b.y.checked_add(b.height as i32).is_some(),"source.initialBounds","Native bounds exceed signed coordinates")?;
        match self.source.kind {
            SourceKind::Window=>{let xid=self.source.window_id.as_deref().ok_or_else(||invalid("source.windowId","Window capture requires a canonical nonzero XID"))?;
                require(!xid.is_empty()&&!xid.starts_with('0')&&xid.bytes().all(|c|c.is_ascii_digit())&&xid.parse::<u32>().is_ok_and(|id|id>0),"source.windowId","Window XID must be canonical decimal in 1..u32::MAX")?;}
            _=>require(self.source.window_id.is_none()&&b.x>=0&&b.y>=0,"source","Screen and region require null windowId and nonnegative root origins")?,
        }
        if let Some(diagnostic)=&self.diagnostic {require(!diagnostic.message.is_empty()&&diagnostic.message.len()<=1024,"diagnostic.message","Capture diagnostic must contain 1..1024 UTF-8 bytes")?;}
        if let CaptureStateV1::Finalized{video_timing,cursor_timing,initial_project:doc}=&self.state {
            project::validate(doc)?;
            require(doc.schema_version==3&&doc.revision==0&&doc.project_id==id,"initialProject","Native snapshot must be schema 3, revision 0 and use the capture ID")?;
            let duration=video_timing.endpoint.duration_us()?;
            require(doc.video.path=="screen.mp4"&&(doc.video.width,doc.video.height)==(b.width,b.height)&&doc.video.duration_us==duration,"initialProject.video","Initial video must match native bounds, endpoint and screen.mp4")?;
            require(doc.clips.len()==1&&doc.clips[0].id=="clip-0"&&doc.clips[0].in_us==0&&doc.clips[0].out_us==duration,"initialProject.clips","Native snapshot requires one clip-0 spanning the full video")?;
            require(doc.microphone.as_ref().is_none_or(|mic|mic.path=="mic.wav"),"initialProject.microphone","Native microphone must reference mic.wav")?;
            require(doc.timing.provenance==TimingProvenance::LegacyUnknown&&doc.timing.audio_offset_us.is_none()&&doc.warnings.contains(&ProjectWarning::LegacyTimingUnknown)&&!doc.warnings.contains(&ProjectWarning::EmbeddedCursor),"initialProject.timing","Native audio timing must remain legacyUnknown/null with legacyTimingUnknown and no embeddedCursor warning")?;
            let degraded=doc.warnings.contains(&ProjectWarning::CursorCaptureIncomplete);
            require(self.diagnostic.is_some()==degraded&&(!degraded||!doc.cursor_settings.enabled),"diagnostic","Declared cursor degradation requires a diagnostic, cursorCaptureIncomplete and disabled initial cursor settings")?;
            if let Some(clock)=&video_timing.clock {
                require((1..=2_592_000).contains(&clock.completed_frames)&&clock.cadence.numerator==1&&clock.cadence.denominator==30,"videoTiming.clock","Accepted clock requires 1..2592000 completed frames at exactly 1/30")?;
                require(equal_time(ffmpeg::PacketTime{ticks:clock.completed_frames,numerator:1,denominator:30},video_timing.endpoint.packet_time())?,"videoTiming.clock","Completed encoder grid must equal the exact saved-video endpoint")?;
                let low=decimal(&clock.anchor.low_ns)?;let high=decimal(&clock.anchor.high_exclusive_ns)?;
                require(low<high&&high-low<=1_001_000&&low.checked_add((high-low)/2).is_some(),"videoTiming.clock.anchor","Anchor must be nonempty and within the 1001000ns bridge envelope")?;
            }
            match doc.video.cursor {
                CursorMode::Separate=>require(video_timing.clock.is_some()&&cursor_timing.is_some()&&doc.cursor.as_ref().is_some_and(|c|c.index_path=="cursor/index.json"),"initialProject.cursor","Separate native cursor requires accepted clock, cursor timing and cursor/index.json")?,
                CursorMode::Absent=>require(doc.cursor.is_none()&&cursor_timing.is_none()&&!doc.cursor_settings.enabled&&degraded,"initialProject.cursor","Absent native cursor requires null cursor/timing and declared disabled degradation")?,
                CursorMode::Embedded=>return Err(invalid("initialProject.video.cursor","Native capture schema 1 does not support embedded cursor finalization")),
            }
        }
        Ok(())
    }
    pub fn finalized_project(&self)->CoreResult<&ProjectDocument> {
        match &self.state { CaptureStateV1::Finalized{initial_project,..}=>Ok(initial_project), _=>Err(invalid("capture.json","Native capture is incomplete; retain its files and recover or finalize it before opening")) }
    }
}

pub fn decode(bytes:&[u8],id:&str)->CoreResult<CaptureManifestV1> {
    require(bytes.len() as u64<=MAX_JSON_BYTES,"capture.json","Native capture JSON exceeds 8 MiB")?;
    let manifest:CaptureManifestV1=serde_json::from_slice(bytes).map_err(|error| {
        let mut message=format!("Invalid native capture manifest; retain its files and correct capture.json: {error}");
        store::truncate_text(&mut message,1024);
        invalid("capture.json",message)
    })?;
    manifest.validate(id)?;Ok(manifest)
}
fn path_error(message:&str)->CoreError {CoreError{code:ErrorCode::InvalidPath,message:message.into(),field:Some("capture.path".into())}}
fn io_error(error:std::io::Error)->CoreError {CoreError{code:ErrorCode::Io,message:format!("Could not access native capture: {error}"),field:Some("capture.json".into())}}
// Native authority disallows aliases even when their targets happen to be contained today.
fn asset(dir:&Path,relative:&str)->CoreResult<Option<PathBuf>> {
    require(relative.len()<=512&&project::valid_asset_name(relative),"capture.path","Expected a safe native relative asset path")?;
    let mut path=dir.to_path_buf();
    for component in relative.split('/') {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink()=>return Err(path_error("Native capture assets cannot use symlink aliases")),
            Ok(_)=>{},
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{store::validate_missing_asset(dir,relative)?;return Ok(None)},
            Err(error)=>return Err(io_error(error)),
        }
    }
    let resolved=paths::asset_path(dir,relative)?;
    require(resolved==dir.join(relative),"capture.path","Native asset must resolve to its declared path")?;Ok(Some(resolved))
}
pub(crate) fn read(dir:&Path,id:&str)->CoreResult<Option<CaptureManifestV1>> {
    let Some(path)=asset(dir,"capture.json")? else {return Ok(None)};
    let mut bytes=Vec::new();fs::File::open(path).and_then(|f|f.take(MAX_JSON_BYTES+1).read_to_end(&mut bytes)).map_err(io_error)?;
    decode(&bytes,id).map(Some)
}
pub(crate) enum CaptureAuthority {LinuxV1(CaptureManifestV1),WindowsV2(windows::WindowsCaptureManifestV2)}
impl CaptureAuthority {
    pub(crate) fn finalized_project(&self)->CoreResult<&ProjectDocument> {
        match self {Self::LinuxV1(m)=>m.finalized_project(),Self::WindowsV2(m)=>m.finalized_project()}
    }
}
pub(crate) fn read_authority(dir:&Path,id:&str)->CoreResult<Option<CaptureAuthority>> {
    let marker=windows::read_marker(dir)?;
    let Some(path)=asset(dir,"capture.json")? else {
        require(!marker,"capture.json","Windows capture.json is missing; retain files for recovery")?;return Ok(None)
    };
    let mut bytes=Vec::new();fs::File::open(path).and_then(|f|f.take(MAX_JSON_BYTES+1).read_to_end(&mut bytes)).map_err(io_error)?;
    require(bytes.len() as u64<=MAX_JSON_BYTES,"capture.json","Native capture JSON exceeds 8 MiB")?;
    let UniqueJson(value)=serde_json::from_slice(&bytes).map_err(|error| {let mut message=format!("Invalid native capture manifest; retain its files and correct capture.json: {error}");store::truncate_text(&mut message,1024);invalid("capture.json",message)})?;
    named_object_shapes(&value,&mut Vec::new()).map_err(|e|invalid("capture.json",e))?;
    let authority=match value.get("schemaVersion").and_then(serde_json::Value::as_u64) {
        Some(1)=>{require(!marker,"capture.json","Windows intent marker conflicts with Linux authority; retain files for recovery")?;CaptureAuthority::LinuxV1(decode(&bytes,id)?)},
        Some(2)=>{require(marker,"capture.json","Windows capture intent marker is missing; retain files for recovery")?;CaptureAuthority::WindowsV2(decode_windows(&bytes,id)?)},
        _=>return Err(invalid("schemaVersion","Unsupported or missing capture schema; retain files for recovery")),
    };
    Ok(Some(authority))
}
pub(crate) fn import_authority(dir:&Path,authority:CaptureAuthority,ffmpeg:&Path)->CoreResult<ProjectDocument> {
    match authority {CaptureAuthority::LinuxV1(m)=>import(dir,m,ffmpeg),CaptureAuthority::WindowsV2(m)=>windows::import_windows(dir,m,ffmpeg)}
}

pub(crate) fn import(dir:&Path,manifest:CaptureManifestV1,ffmpeg_path:&Path)->CoreResult<ProjectDocument> {
    if !matches!(manifest.state,CaptureStateV1::Finalized{..}) {return Err(invalid("capture.json","Native capture is incomplete; recover or finalize it before opening"))}
    let path=asset(dir,"screen.mp4")?.ok_or_else(||invalid("video","Native screen.mp4 is missing"))?;
    let evidence=ffmpeg::probe_video_evidence(ffmpeg_path,&path)?;
    validate_finalized_assets(dir,&manifest,&evidence)
}

/// Validate finalized authority against already-probed media without writes or subprocesses.
pub fn validate_finalized_assets(dir:&Path,manifest:&CaptureManifestV1,evidence:&ffmpeg::ProbedVideoEvidence)->CoreResult<ProjectDocument> {
    validate_asset_inventory(dir,manifest,evidence).map(|(doc,_)|doc)
}
fn validate_asset_inventory(dir:&Path,manifest:&CaptureManifestV1,evidence:&ffmpeg::ProbedVideoEvidence)->CoreResult<(ProjectDocument,Vec<String>)> {
    manifest.validate(&manifest.capture_id)?;
    let CaptureStateV1::Finalized{video_timing,initial_project,..}=&manifest.state else {return Err(invalid("capture.json","Native capture is incomplete; recover or finalize it before opening"))};
    let mut doc=initial_project.clone();
    let mut inventory=vec!["screen.mp4".to_owned()];
    asset(dir,"screen.mp4")?.ok_or_else(||invalid("video","Native screen.mp4 is missing"))?;
    require((evidence.video.width,evidence.video.height)==(doc.video.width,doc.video.height)&&equal_time(evidence.endpoint,video_timing.endpoint.packet_time())?,"video","Saved video geometry or exact endpoint does not match capture authority")?;
    if doc.video.cursor==CursorMode::Separate {
        let index=asset(dir,"cursor/index.json")?.ok_or_else(||invalid("cursor","Native cursor index is missing"))?;
        let mut bytes=Vec::new();fs::File::open(index).and_then(|f|f.take(cursor::MAX_INDEX_BYTES+1).read_to_end(&mut bytes)).map_err(io_error)?;
        require(bytes.len() as u64<=cursor::MAX_INDEX_BYTES,"cursor.index","Cursor index exceeds 64 KiB")?;
        let index:cursor::CursorIndex=serde_json::from_slice(&bytes).map_err(|_|invalid("cursor.index","Invalid native cursor index"))?;
        for path in std::iter::once(&index.samples_path).chain(index.shapes.iter().map(|shape|&shape.path)) {
            require(path.starts_with("cursor/"),"cursor.path","Native cursor assets must remain under cursor/")?;asset(dir,path)?;
            inventory.push(path.clone());
        }
        match cursor::inspect_capture_cursor(dir,&doc) {
            cursor::CursorAssetReport::Available{native_available,..}=>require(native_available||(!doc.cursor_settings.enabled&&manifest.diagnostic.is_some()),"cursor","Unavailable native images require declared disabled capture degradation")?,
            cursor::CursorAssetReport::Unavailable(error)=>return Err(error),
            cursor::CursorAssetReport::Unchecked=>return Err(invalid("cursor","Native cursor assets were not validated")),
        }
        inventory.push("cursor/index.json".into());
        if doc.cursor_settings.enabled {cursor::prepare_cursor(dir,&doc,&AtomicBool::new(false))?;}
    }
    project_optional_audio(dir,&mut doc)?;
    if doc.microphone.is_some() {inventory.insert(1,"mic.wav".into());}
    Ok((doc,inventory))
}

fn project_optional_audio(dir:&Path,doc:&mut ProjectDocument)->CoreResult<()> {
    if doc.microphone.is_some() {
        let usable=asset(dir,"mic.wav")?.is_some_and(|path|crate::audio::is_usable_wav(&path));
        if !usable {doc.microphone=None;if !doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio) {doc.warnings.push(ProjectWarning::InvalidOptionalAudio);}}
    }
    Ok(())
}

/// Validate and sync the immutable native assets before committing their final authority.
/// Returns the exact optional-audio-projected manifest committed by the publisher.
pub fn publish_finalized_assets(dir:&Path,manifest:&CaptureManifestV1,evidence:&ffmpeg::ProbedVideoEvidence)->CoreResult<CaptureManifestV1> {
    publish_finalized_with(dir,manifest,evidence,|path,directory|if directory {store::sync_directory(path)} else {fs::File::open(path)?.sync_all()},publish)
}
fn publish_finalized_with(dir:&Path,manifest:&CaptureManifestV1,evidence:&ffmpeg::ProbedVideoEvidence,
    mut sync:impl FnMut(&Path,bool)->std::io::Result<()>,publish:impl FnOnce(&Path,&CaptureManifestV1)->CoreResult<()>)->CoreResult<CaptureManifestV1> {
    let (doc,inventory)=validate_asset_inventory(dir,manifest,evidence)?;
    let mut committed=manifest.clone();
    let CaptureStateV1::Finalized{initial_project,..}=&mut committed.state else {return Err(invalid("capture.json","Finalized publication requires a finalized capture"))};
    *initial_project=doc;
    let mut directories=std::collections::BTreeSet::new();
    for relative in inventory {
        let path=asset(dir,&relative)?.ok_or_else(||invalid("capture.path",format!("Required native asset {relative} is missing; retain this capture for recovery")))?;
        sync(&path,false).map_err(io_error)?;
        let mut parent=path.parent();
        while let Some(path)=parent {if path==dir {break;}directories.insert(path.to_path_buf());parent=path.parent();}
    }
    // Even an empty native shape directory was created by the producer.
    if initial_project.video.cursor==CursorMode::Separate {
        let shapes=dir.join("cursor/shapes");
        if fs::symlink_metadata(&shapes).is_ok() {
            require(fs::symlink_metadata(&shapes).map_err(io_error)?.is_dir()&&shapes.canonicalize().map_err(io_error)?==shapes,"cursor.path","Native shape directory must be a real contained directory")?;
            directories.insert(shapes);
        }
    }
    let mut directories:Vec<_>=directories.into_iter().collect();
    directories.sort_by_key(|path|std::cmp::Reverse(path.components().count()));
    for path in &directories {sync(path,true).map_err(io_error)?;}
    sync(dir,true).map_err(io_error)?;
    publish(dir,&committed)?;Ok(committed)
}

/// Publish only capture.json. The future producer must sync referenced assets before finalization.
/// Unix publication syncs the session, and incomplete publication also syncs its parent entry.
pub fn publish(dir:&Path,manifest:&CaptureManifestV1)->CoreResult<()> {
    publish_with(dir,manifest,|_|Ok(()),|_|Ok(()),store::sync_directory)
}
fn publish_with(dir:&Path,manifest:&CaptureManifestV1,before_write:impl FnOnce(&Path)->std::io::Result<()>,before_rename:impl FnOnce(&Path)->std::io::Result<()>,mut sync:impl FnMut(&Path)->std::io::Result<()>)->CoreResult<()> {
    let real=dir.canonicalize().map_err(io_error)?;
    if real!=dir||!fs::symlink_metadata(dir).map_err(io_error)?.is_dir() {return Err(path_error("Capture publisher requires a real session directory"));}
    let id=dir.file_name().and_then(|v|v.to_str()).ok_or_else(||path_error("Capture directory must have a UTF-8 ID"))?;
    manifest.validate(id)?;
    let store=store::ProjectStore::new(dir.parent().ok_or_else(||path_error("Capture directory needs a parent"))?)?;
    let _lock=store.lock(id,"document")?;
    if let Some(previous)=read(dir,id)? {
        require(matches!(previous.state,CaptureStateV1::Incomplete{..}),"capture.json","Finalized capture authority is immutable; reload before retrying")?;
        require(previous.capture_id==manifest.capture_id&&previous.source==manifest.source,"capture.json","Capture identity and source are immutable; reload before retrying")?;
    }
    let bytes=serde_json::to_vec(manifest).map_err(|error|invalid("capture.json",error.to_string()))?;
    require(bytes.len() as u64<=MAX_JSON_BYTES,"capture.json","Native capture JSON exceeds 8 MiB")?;
    store::replace_bytes_with(dir,"capture.json",&bytes,before_write,before_rename)?;
    let durability=(||{sync(dir)?;if matches!(manifest.state,CaptureStateV1::Incomplete{..}) {sync(dir.parent().ok_or_else(||std::io::Error::other("Missing session parent"))?)?;}Ok::<(),std::io::Error>(())})();
    durability.map_err(|error|CoreError{code:ErrorCode::Io,message:format!("Capture authority was committed, but directory synchronization failed; durability is unconfirmed. Reload capture.json before retrying: {error}"),field:Some("capture.json".into())})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64,Ordering};
    static NEXT:AtomicU64=AtomicU64::new(0);
    struct Fixture {root:PathBuf,dir:PathBuf}
    impl Fixture {
        fn new()->Self {
            let root=std::env::temp_dir().join(format!("focusflow-capture-publish-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
            let dir=root.join("fixture");fs::create_dir_all(&dir).unwrap();Self{root,dir}
        }
    }
    impl Drop for Fixture {fn drop(&mut self){fs::remove_dir_all(&self.root).unwrap();}}
    fn incomplete()->CaptureManifestV1 {
        serde_json::from_value(serde_json::json!({"schemaVersion":1,"captureId":"fixture","source":{"backend":"linuxX11","kind":"region","initialBounds":{"x":0,"y":0,"width":100,"height":50},"windowId":null},"diagnostic":null,"state":{"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"}})).unwrap()
    }
    fn finalized()->CaptureManifestV1 {
        let mut value=serde_json::to_value(incomplete()).unwrap();
        value["diagnostic"]=serde_json::json!({"stage":"clock","message":"Video retained without a validated cursor clock."});
        value["state"]=serde_json::json!({"status":"finalized","videoTiming":{"endpoint":{"provenance":"savedPacketFirstAndTail","endPts":30,"timeBase":{"numerator":1,"denominator":30},"interval":"halfOpen"},"clock":null},"cursorTiming":null,
        "initialProject":{"schemaVersion":3,"projectId":"fixture","revision":0,"video":{"path":"screen.mp4","width":100,"height":50,"durationUs":1000000,"cursor":"absent"},"cursor":null,"cursorSettings":{"enabled":false,"scale":1.0,"smoothingUs":0,"style":"native"},"microphone":null,"timing":{"provenance":"legacyUnknown","audioOffsetUs":null},"camera":{"panTransitionUs":200000,"settleUs":100000},"frameStyle":{"canvas":"source","background":{"type":"solid","color":"#0b0d10"},"paddingPx":0,"cornerRadiusPx":0,"shadow":false},"clips":[{"id":"clip-0","inUs":0,"outUs":1000000}],"zooms":[],"clicks":[],"warnings":["legacyTimingUnknown","cursorCaptureIncomplete"]}});
        decode(&serde_json::to_vec(&value).unwrap(),"fixture").unwrap()
    }
    fn failed()->std::io::Result<()> {Err(std::io::Error::other("injected publication failure"))}
    fn evidence()->ffmpeg::ProbedVideoEvidence {
        ffmpeg::ProbedVideoEvidence{video:ffmpeg::ProbedVideo{width:100,height:50,duration_us:1_000_000},endpoint:ffmpeg::PacketTime{ticks:30,numerator:1,denominator:30},first_packet_duration:ffmpeg::PacketTime{ticks:1,numerator:1,denominator:30}}
    }
    fn cursor_candidate(dir:&Path)->CaptureManifestV1 {
        fs::create_dir_all(dir.join("cursor/shapes")).unwrap();
        for name in ["screen.mp4","cursor/raw.ndjson",".unrelated.tmp"] {fs::write(dir.join(name),name).unwrap();}
        let mut value=serde_json::to_value(finalized()).unwrap();value["diagnostic"]=serde_json::Value::Null;
        value["state"]["videoTiming"]["clock"]=serde_json::json!({"sourceClock":"unixMicrosecondsPreImageCopyRequest","epoch":"captureLaunchMonotonicNs","anchor":{"lowNs":"0","highExclusiveNs":"1"},"completedFrames":30,"cadence":{"numerator":1,"denominator":30},"provenance":"completedEncoderGridAndSavedPacketCadence","interactions":"deliveryTimeRebasedOnce","audio":"unmeasured"});
        value["state"]["cursorTiming"]=serde_json::json!({"clock":"videoFrameZeroRelativeUs","observation":"x11RequestBracketMidpoint","initialState":"boundedPreRollHold","visibility":"sameScreenNativeImageOrArrow1xIntersectsCrop","serverVisibility":"unknown"});
        let doc=&mut value["state"]["initialProject"];doc["video"]["cursor"]=serde_json::json!("separate");doc["cursor"]=serde_json::json!({"indexPath":"cursor/index.json"});doc["cursorSettings"]["enabled"]=serde_json::json!(true);doc["warnings"]=serde_json::json!(["legacyTimingUnknown"]);
        fs::write(dir.join("cursor/index.json"),serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":1_000_000,"samplesPath":"cursor/samples.ndjson","shapes":[{"id":"a","path":"cursor/shapes/a.pam","width":1,"height":1,"hotspotX":0,"hotspotY":0},{"id":"unused","path":"cursor/shapes/unused.pam","width":1,"height":1,"hotspotX":0,"hotspotY":0}]}).to_string()).unwrap();
        fs::write(dir.join("cursor/samples.ndjson"),"{\"atUs\":0,\"x\":0.5,\"y\":0.5,\"visible\":true,\"shapeId\":\"a\",\"discontinuity\":false}\n").unwrap();
        let mut pam=b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();pam.extend([255;4]);for name in ["a","unused"] {fs::write(dir.join(format!("cursor/shapes/{name}.pam")),&pam).unwrap();}
        decode(&serde_json::to_vec(&value).unwrap(),"fixture").unwrap()
    }
    #[test]
    fn valid_optional_audio_is_synced_and_failure_keeps_previous_authority() {
        let f=Fixture::new();let mut candidate=cursor_candidate(&f.dir);publish(&f.dir,&incomplete()).unwrap();let before=fs::read(f.dir.join("capture.json")).unwrap();
        let spec=hound::WavSpec{channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int};
        let mut wav=hound::WavWriter::create(f.dir.join("mic.wav"),spec).unwrap();wav.write_sample(100i16).unwrap();wav.finalize().unwrap();
        if let CaptureStateV1::Finalized{initial_project,..}=&mut candidate.state {initial_project.microphone=Some(crate::project::AudioAsset{path:"mic.wav".into()});}
        let mut calls=vec![];
        let result=publish_finalized_with(&f.dir,&candidate,&evidence(),|path,directory|{calls.push(path.strip_prefix(&f.dir).unwrap().to_path_buf());if path.ends_with("mic.wav") {failed()} else if directory {store::sync_directory(path)} else {fs::File::open(path)?.sync_all()}},|_,_|panic!("publication after failed WAV synchronization"));
        assert!(result.is_err());assert_eq!(calls,vec![PathBuf::from("screen.mp4"),PathBuf::from("mic.wav")]);assert_eq!(fs::read(f.dir.join("capture.json")).unwrap(),before);
        let committed=publish_finalized_assets(&f.dir,&candidate,&evidence()).unwrap();assert!(committed.finalized_project().unwrap().microphone.is_some());
    }
    #[test]
    fn finalized_syncs_references_then_nested_directories_and_commits_once() {
        let f=Fixture::new();let candidate=cursor_candidate(&f.dir);publish(&f.dir,&incomplete()).unwrap();
        let mut calls=vec![];let mut published=0;
        let committed=publish_finalized_with(&f.dir,&candidate,&evidence(),|path,directory|{calls.push((path.strip_prefix(&f.dir).unwrap().to_string_lossy().into_owned(),directory));if directory {store::sync_directory(path)} else {fs::File::open(path)?.sync_all()}},|dir,m|{published+=1;publish(dir,m)}).unwrap();
        assert_eq!(calls,vec![("screen.mp4".into(),false),("cursor/samples.ndjson".into(),false),("cursor/shapes/a.pam".into(),false),("cursor/shapes/unused.pam".into(),false),("cursor/index.json".into(),false),("cursor/shapes".into(),true),("cursor".into(),true),("".into(),true)]);
        assert_eq!(published,1);assert_eq!(committed,candidate);assert_eq!(read(&f.dir,"fixture").unwrap().unwrap(),candidate);
    }
    #[test]
    fn every_required_asset_and_directory_sync_failure_preserves_incomplete_authority() {
        for fail in ["screen.mp4","cursor/samples.ndjson","cursor/shapes/a.pam","cursor/shapes/unused.pam","cursor/index.json","cursor/shapes","cursor",""] {
            let f=Fixture::new();let candidate=cursor_candidate(&f.dir);publish(&f.dir,&incomplete()).unwrap();let before=fs::read(f.dir.join("capture.json")).unwrap();
            let result=publish_finalized_with(&f.dir,&candidate,&evidence(),|path,directory|{if path.strip_prefix(&f.dir).unwrap()==Path::new(fail) {failed()} else if directory {store::sync_directory(path)} else {fs::File::open(path)?.sync_all()}},|_,_|panic!("publication after failed asset synchronization"));
            assert!(result.is_err(),"{fail}");assert_eq!(fs::read(f.dir.join("capture.json")).unwrap(),before);
            for name in ["screen.mp4","cursor/raw.ndjson",".unrelated.tmp"] {assert_eq!(fs::read(f.dir.join(name)).unwrap(),name.as_bytes());}assert!(!f.dir.join("project.json").exists());
        }
    }
    #[test]
    fn finalized_wrapper_preserves_publisher_precommit_and_postcommit_failure_boundaries() {
        for stage in ["write","rename","sync"] {
            let f=Fixture::new();let candidate=cursor_candidate(&f.dir);publish(&f.dir,&incomplete()).unwrap();let before=fs::read(f.dir.join("capture.json")).unwrap();
            let result=publish_finalized_with(&f.dir,&candidate,&evidence(),|path,directory|if directory {store::sync_directory(path)}else{fs::File::open(path)?.sync_all()},|dir,m|publish_with(dir,m,|_|if stage=="write" {failed()}else{Ok(())},|_|if stage=="rename" {failed()}else{Ok(())},|_|failed()));
            let error=result.unwrap_err();
            if stage=="sync" {assert!(error.message.contains("durability is unconfirmed")&&error.message.contains("Reload"));assert_eq!(read(&f.dir,"fixture").unwrap().unwrap(),candidate);assert!(publish_finalized_assets(&f.dir,&candidate,&evidence()).is_err());}
            else {assert_eq!(fs::read(f.dir.join("capture.json")).unwrap(),before);}
            assert_eq!(fs::read(f.dir.join("cursor/raw.ndjson")).unwrap(),b"cursor/raw.ndjson");assert_eq!(fs::read(f.dir.join("screen.mp4")).unwrap(),b"screen.mp4");
        }
    }
    #[test]
    fn native_publication_requires_even_unused_references_and_strict_disabled_positions() {
        for failure in ["missing","corrupt","position"] {
            let f=Fixture::new();let mut candidate=cursor_candidate(&f.dir);publish(&f.dir,&incomplete()).unwrap();let before=fs::read(f.dir.join("capture.json")).unwrap();
            match failure {"missing"=>fs::remove_file(f.dir.join("cursor/shapes/unused.pam")).unwrap(),"corrupt"=>fs::write(f.dir.join("cursor/shapes/unused.pam"),b"broken").unwrap(),_=>fs::write(f.dir.join("cursor/samples.ndjson"),b"{\"atUs\":0,\"x\":999,\"y\":0,\"visible\":false,\"shapeId\":null,\"discontinuity\":true}\n").unwrap()}
            assert!(publish_finalized_assets(&f.dir,&candidate,&evidence()).is_err());
            candidate.diagnostic=Some(CaptureDiagnosticV1{stage:DiagnosticStage::Cursor,message:"Native cursor capture interrupted".into()});
            if let CaptureStateV1::Finalized{initial_project,..}=&mut candidate.state {initial_project.cursor_settings.enabled=false;initial_project.warnings.push(ProjectWarning::CursorCaptureIncomplete);}
            if failure=="missing" {assert!(validate_finalized_assets(&f.dir,&candidate,&evidence()).is_ok());assert!(publish_finalized_assets(&f.dir,&candidate,&evidence()).is_err());}
            if failure=="position" {assert!(publish_finalized_assets(&f.dir,&candidate,&evidence()).is_err());}
            assert_eq!(fs::read(f.dir.join("capture.json")).unwrap(),before);
        }
    }
    #[test]
    fn finalized_asset_publication_projects_audio_and_preserves_sources() {
        let fixture=Fixture::new();let dir=&fixture.dir;
        fs::write(dir.join("screen.mp4"),b"generated evidence supplied by caller").unwrap();
        fs::write(dir.join("mic.wav"),b"broken optional audio").unwrap();
        publish(dir,&incomplete()).unwrap();
        let mut candidate=finalized();
        if let CaptureStateV1::Finalized{initial_project,..}=&mut candidate.state {
            initial_project.microphone=Some(crate::project::AudioAsset{path:"mic.wav".into()});
        }
        let evidence=ffmpeg::ProbedVideoEvidence{video:ffmpeg::ProbedVideo{width:100,height:50,duration_us:1_000_000},endpoint:ffmpeg::PacketTime{ticks:30,numerator:1,denominator:30},first_packet_duration:ffmpeg::PacketTime{ticks:1,numerator:1,denominator:30}};
        let committed=publish_finalized_assets(dir,&candidate,&evidence).unwrap();
        assert_eq!(read(dir,"fixture").unwrap().unwrap(),committed);
        let doc=committed.finalized_project().unwrap();assert!(doc.microphone.is_none());assert!(doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio));
        assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),b"generated evidence supplied by caller");
        assert_eq!(fs::read(dir.join("mic.wav")).unwrap(),b"broken optional audio");assert!(!dir.join("project.json").exists());
    }
    #[test]
    fn publisher_transitions_are_monotonic_and_preserve_identity_and_other_files() {
        let fixture=Fixture::new();let dir=&fixture.dir;let mut start=incomplete();
        for name in ["project.json","screen.mp4","mic.wav","unrelated.tmp"] {fs::write(dir.join(name),name.as_bytes()).unwrap();}
        publish(dir,&start).unwrap();let first=fs::read(dir.join("capture.json")).unwrap();
        start.source.initial_bounds.x=2;assert!(publish(dir,&start).is_err());assert_eq!(fs::read(dir.join("capture.json")).unwrap(),first);
        start=incomplete();start.diagnostic=Some(CaptureDiagnosticV1{stage:DiagnosticStage::Starting,message:"Preparing owned capture resources".into()});publish(dir,&start).unwrap();
        publish(dir,&finalized()).unwrap();let final_bytes=fs::read(dir.join("capture.json")).unwrap();
        for retry in [incomplete(),finalized()] {assert!(publish(dir,&retry).unwrap_err().message.contains("reload"));assert_eq!(fs::read(dir.join("capture.json")).unwrap(),final_bytes);}
        for name in ["project.json","screen.mp4","mic.wav","unrelated.tmp"] {assert_eq!(fs::read(dir.join(name)).unwrap(),name.as_bytes());}
        let direct=Fixture::new();publish(&direct.dir,&finalized()).unwrap();assert!(read(&direct.dir,"fixture").unwrap().unwrap().finalized_project().is_ok());
    }
    #[test]
    fn publisher_does_not_replace_invalid_authority_or_invalid_candidate() {
        let fixture=Fixture::new();let dir=&fixture.dir;
        for bytes in [b"broken".as_slice(),br#"{"schemaVersion":2}"#.as_slice()] {
            fs::write(dir.join("capture.json"),bytes).unwrap();assert!(publish(dir,&incomplete()).is_err());assert_eq!(fs::read(dir.join("capture.json")).unwrap(),bytes);
        }
        fs::remove_file(dir.join("capture.json")).unwrap();let mut bad=incomplete();bad.capture_id="other".into();assert!(publish(dir,&bad).is_err());assert!(!dir.join("capture.json").exists());
        let mut bad=finalized();if let CaptureStateV1::Finalized{initial_project,..}=&mut bad.state {initial_project.revision=1;}assert!(publish(dir,&bad).is_err());assert!(!dir.join("capture.json").exists());
        fs::create_dir(dir.join("capture.json")).unwrap();assert!(publish(dir,&incomplete()).is_err());assert!(dir.join("capture.json").is_dir());
    }
    #[test]
    fn publication_before_write_and_rename_failures_keep_authority_and_clean_only_owned_temp() {
        for before_write in [true,false] {
            let fixture=Fixture::new();let dir=&fixture.dir;publish(dir,&incomplete()).unwrap();let original=fs::read(dir.join("capture.json")).unwrap();
            let abandoned=dir.join(".capture.json-unrelated.tmp");fs::write(&abandoned,b"owned elsewhere").unwrap();
            let error=publish_with(dir,&finalized(),|path|{assert!(path.is_file());assert_eq!(fs::metadata(path)?.len(),0);if before_write {fs::write(path,b"partial temporary write")?;failed()}else{Ok(())}},|path|{assert_eq!(fs::read(path)?,serde_json::to_vec(&finalized()).unwrap());failed()},|_|panic!("sync after failed rename"));
            assert!(error.is_err());assert_eq!(fs::read(dir.join("capture.json")).unwrap(),original);assert_eq!(fs::read(abandoned).unwrap(),b"owned elsewhere");assert_eq!(fs::read_dir(dir).unwrap().count(),2);
        }
    }
    #[test]
    fn publication_sync_failures_retain_committed_bytes_and_require_reload() {
        for failure_at in [0,1] {
            let fixture=Fixture::new();let dir=&fixture.dir;let mut calls=vec![];
            let error=publish_with(dir,&incomplete(),|_|Ok(()),|_|Ok(()),|path|{calls.push(path.to_path_buf());if calls.len()-1==failure_at {failed()}else{store::sync_directory(path)}}).unwrap_err();
            assert!(error.message.contains("committed")&&error.message.contains("durability is unconfirmed")&&error.message.contains("Reload"));
            assert_eq!(read(dir,"fixture").unwrap().unwrap(),incomplete());assert_eq!(calls[0],*dir);if failure_at==1 {assert_eq!(calls[1],fixture.root);}
            let store=store::ProjectStore::new(&fixture.root).unwrap();let defaults=crate::legacy::LegacyDefaults{scale:2.0,intro_us:0,hold_us:0,outro_us:0,pan_transition_us:0,settle_us:0};
            assert!(store.open_or_import("fixture",Path::new("absent-ffmpeg"),defaults).unwrap_err().message.contains("incomplete"));assert!(!dir.join("project.json").exists());
        }
        let fixture=Fixture::new();publish(&fixture.dir,&incomplete()).unwrap();
        let error=publish_with(&fixture.dir,&finalized(),|_|Ok(()),|_|Ok(()),|_|failed()).unwrap_err();assert!(error.message.contains("Reload"));
        let bytes=fs::read(fixture.dir.join("capture.json")).unwrap();assert!(publish(&fixture.dir,&incomplete()).is_err());assert_eq!(fs::read(fixture.dir.join("capture.json")).unwrap(),bytes);
    }
    #[test]
    fn incomplete_syncs_parent_but_final_only_syncs_session_and_never_claims_asset_sync() {
        let fixture=Fixture::new();let mut paths=vec![];
        publish_with(&fixture.dir,&incomplete(),|_|Ok(()),|_|Ok(()),|path|{paths.push(path.to_path_buf());store::sync_directory(path)}).unwrap();assert_eq!(paths,vec![fixture.dir.clone(),fixture.root.clone()]);paths.clear();
        publish_with(&fixture.dir,&finalized(),|_|Ok(()),|_|Ok(()),|path|{paths.push(path.to_path_buf());store::sync_directory(path)}).unwrap();assert_eq!(paths,vec![fixture.dir.clone()]);assert!(!fixture.dir.join("screen.mp4").exists());
    }
    #[test]
    fn oversized_publication_retains_previous_authority() {
        let fixture=Fixture::new();publish(&fixture.dir,&incomplete()).unwrap();let before=fs::read(fixture.dir.join("capture.json")).unwrap();
        let mut manifest=finalized();if let CaptureStateV1::Finalized{initial_project,..}=&mut manifest.state {
            initial_project.warnings=vec![ProjectWarning::LegacyTimingUnknown;600_000];initial_project.warnings.push(ProjectWarning::CursorCaptureIncomplete);
        }
        let error=publish(&fixture.dir,&manifest).unwrap_err();assert!(error.message.contains("8 MiB"));assert_eq!(fs::read(fixture.dir.join("capture.json")).unwrap(),before);assert_eq!(fs::read_dir(&fixture.dir).unwrap().count(),1);
    }
    #[test]
    fn publisher_uses_the_same_document_lock_as_import() {
        let fixture=Fixture::new();let store=store::ProjectStore::new(&fixture.root).unwrap();let _guard=store.lock("fixture","document").unwrap();
        assert_eq!(publish(&fixture.dir,&incomplete()).unwrap_err().code,ErrorCode::Busy);assert!(!fixture.dir.join("capture.json").exists());
    }
    #[cfg(unix)]
    #[test]
    fn publisher_rejects_real_and_dangling_authority_aliases() {
        use std::os::unix::fs::symlink;
        for dangling in [false,true] {
            let fixture=Fixture::new();let target=fixture.dir.join("target.json");if !dangling {fs::write(&target,serde_json::to_vec(&incomplete()).unwrap()).unwrap();}
            symlink(&target,fixture.dir.join("capture.json")).unwrap();assert!(publish(&fixture.dir,&incomplete()).is_err());assert!(fs::symlink_metadata(fixture.dir.join("capture.json")).unwrap().file_type().is_symlink());
        }
        let fixture=Fixture::new();let alias=fixture.root.join("alias");symlink(&fixture.dir,&alias).unwrap();assert!(publish(&alias,&incomplete()).is_err());assert!(!fixture.dir.join("capture.json").exists());
    }
}
