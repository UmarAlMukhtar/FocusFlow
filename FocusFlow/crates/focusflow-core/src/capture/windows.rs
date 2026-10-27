//! Owned WGC summary authority. The producer audits native history; import checks saved media.
use super::*;
use std::{io::Write, time::{Duration, Instant}};

const MARKER_NAME: &str = "capture.windows-v2";
const MARKER_BYTES: &[u8] = b"focusflow-windows-capture-v2\n";

literals!(WindowsBackend { WindowsWgcOwned => "windowsWgcOwned" });
literals!(WindowsSourceKind { Window => "window" });
literals!(EmbeddedCursor { Embedded => "embedded" });
literals!(WindowsEndpointProvenance { SavedPacketFullGrid => "savedPacketFullGrid" });
literals!(WindowsClockProvenance { OwnedWgcCausalCfrV1 => "ownedWgcCausalCfrV1" });
literals!(WindowsCutoffReason { Stop => "stop", SourceClosed => "sourceClosed" });
literals!(Unmeasured { Unmeasured => "unmeasured" });

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullOnly;
impl Serialize for NullOnly {
    fn serialize<S:serde::Serializer>(&self, serializer:S)->Result<S::Ok,S::Error> { serializer.serialize_unit() }
}
impl<'de> Deserialize<'de> for NullOnly {
    fn deserialize<D:serde::Deserializer<'de>>(de:D)->Result<Self,D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value=NullOnly;
            fn expecting(&self,f:&mut std::fmt::Formatter)->std::fmt::Result {f.write_str("explicit JSON null")}
            fn visit_unit<E:serde::de::Error>(self)->Result<NullOnly,E> {Ok(NullOnly)}
        }
        de.deserialize_any(Visitor)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all="camelCase")]
pub struct WindowsCaptureManifestV2 {
    pub schema_version:u32, pub capture_id:String, pub source:WindowsSourceV2,
    pub initial_defaults:WindowsInitialDefaults, pub diagnostic:Option<CaptureDiagnosticV1>, pub state:WindowsCaptureStateV2,
}
impl<'de> Deserialize<'de> for WindowsCaptureManifestV2 {
    fn deserialize<D:serde::Deserializer<'de>>(de:D)->Result<Self,D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all="camelCase",deny_unknown_fields)]
        struct Wire {
            schema_version:u32, capture_id:String, source:WindowsSourceV2, initial_defaults:WindowsInitialDefaults,
            #[serde(deserialize_with="required")]
            diagnostic:Option<CaptureDiagnosticV1>, state:WindowsCaptureStateV2,
        }
        let UniqueJson(value)=UniqueJson::deserialize(de)?;
        named_object_shapes(&value,&mut Vec::new()).map_err(serde::de::Error::custom)?;
        for path in ["/initialDefaults/frameStyle/canvas","/initialDefaults/frameStyle/background/type"] {
            if !value.pointer(path).is_some_and(serde_json::Value::is_string) {return Err(serde::de::Error::custom("Initial defaults literal fields require JSON strings"));}
        }
        let wire:Wire=serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {schema_version:wire.schema_version,capture_id:wire.capture_id,source:wire.source,initial_defaults:wire.initial_defaults,diagnostic:wire.diagnostic,state:wire.state})
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WindowsSourceV2 {
    pub backend:WindowsBackend, pub kind:WindowsSourceKind, pub window_id:String,
    #[serde(deserialize_with="required")]
    pub observed_window_bounds:Option<Bounds>,
    pub raw_size:PixelSize, pub encoded_size:PixelSize, pub content_rect:Bounds, pub pointer_origin:NullOnly,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct PixelSize {pub width:u32,pub height:u32}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WindowsInitialDefaults {pub frame_style:project::FrameStyle,pub camera:project::CameraDefaults}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag="status",rename_all="camelCase",deny_unknown_fields)]
pub enum WindowsCaptureStateV2 {
    #[serde(rename_all="camelCase")]
    Incomplete {video_path:VideoPath,video_cursor:EmbeddedCursor},
    #[serde(rename_all="camelCase")]
    Finalized {
        video_timing:WindowsVideoTimingV2, cursor_timing:NullOnly,
        #[serde(deserialize_with="strict_project")]
        initial_project:ProjectDocument,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WindowsVideoTimingV2 {pub endpoint:WindowsSavedEndpointV2,pub clock:WindowsOwnedClockV1}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WindowsSavedEndpointV2 {pub provenance:WindowsEndpointProvenance,pub end_pts:u64,pub time_base:TimeBase,pub interval:Interval}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WindowsOwnedClockV1 {
    pub provenance:WindowsClockProvenance, pub qpc_frequency:String, pub admission_qpc:String,
    #[serde(rename="epoch100ns")]
    pub epoch_100ns:String,
    pub cutoff_qpc:String, pub cutoff_reason:WindowsCutoffReason, pub completed_frames:u64,
    pub worker_exit_code:i32, pub encoder_exit_code:i32, pub interactions:Unmeasured, pub audio:Unmeasured,
}
impl WindowsSavedEndpointV2 {
    fn packet_time(&self)->ffmpeg::PacketTime {ffmpeg::PacketTime {ticks:self.end_pts,numerator:self.time_base.numerator,denominator:self.time_base.denominator}}
    pub fn duration_us(&self)->CoreResult<u64> {endpoint_duration_us(self.end_pts,&self.time_base)}
}
fn canonical_decimal(text:&str, min:u64, max:u64, field:&str)->CoreResult<u64> {
    require(!text.is_empty()&&text.len()<=20&&text.bytes().all(|b|b.is_ascii_digit())&&(text=="0"||!text.starts_with('0')),field,"Expected a canonical unsigned decimal string")?;
    let value=text.parse::<u64>().map_err(|_|invalid(field,"Decimal exceeds its integer range"))?;
    require((min..=max).contains(&value),field,"Decimal is outside the permitted range")?;Ok(value)
}
impl WindowsOwnedClockV1 {
    fn validate(&self,endpoint:&WindowsSavedEndpointV2)->CoreResult<()> {
        let frequency=canonical_decimal(&self.qpc_frequency,1,i64::MAX as u64,"videoTiming.clock.qpcFrequency")?;
        let admission=canonical_decimal(&self.admission_qpc,0,i64::MAX as u64,"videoTiming.clock.admissionQpc")?;
        let epoch=canonical_decimal(&self.epoch_100ns,0,i64::MAX as u64,"videoTiming.clock.epoch100ns")?;
        let cutoff=canonical_decimal(&self.cutoff_qpc,0,i64::MAX as u64,"videoTiming.clock.cutoffQpc")?;
        let epoch=product(&[epoch.into(),frequency.into()])?;
        let admission_time=product(&[admission.into(),10_000_000])?;
        require(epoch>=admission_time&&cutoff>=admission,"videoTiming.clock","First native timestamp must follow admission and cutoff must not precede admission")?;
        let delta=product(&[cutoff.into(),10_000_000])?.checked_sub(epoch).ok_or_else(||invalid("videoTiming.clock","Cutoff precedes the first native timestamp"))?;
        let n=self.completed_frames;
        require((1..=2_592_000).contains(&n),"videoTiming.clock.completedFrames","Expected 1..2592000 completed frames")?;
        let cell=product(&[frequency.into(),10_000_000])?;
        let elapsed=product(&[30,delta])?;
        require(product(&[n.into(),cell])?<=elapsed&&elapsed<product(&[u128::from(n)+1,cell])?,"videoTiming.clock","Completed frames must equal the exact full-cell cutoff")?;
        require(self.worker_exit_code==0&&self.encoder_exit_code==0,"videoTiming.clock","Both owned worker and encoder must exit successfully")?;
        require(equal_time(endpoint.packet_time(),ffmpeg::PacketTime {ticks:n,numerator:1,denominator:30})?,"videoTiming.endpoint","Saved endpoint must equal completedFrames / 30")
    }
}
impl WindowsCaptureManifestV2 {
    pub fn validate(&self,id:&str)->CoreResult<()> {
        if self.schema_version!=2 {return Err(CoreError {code:ErrorCode::UnsupportedSchema,message:"Only Windows capture schema 2 is supported".into(),field:Some("schemaVersion".into())});}
        require(project::valid_id(id)&&self.capture_id==id,"captureId","Capture ID must equal its real containing directory ID")?;
        let source=&self.source;
        canonical_decimal(&source.window_id,1,u64::MAX,"source.windowId")?;
        for (raw,encoded) in [(source.raw_size.width,source.encoded_size.width),(source.raw_size.height,source.encoded_size.height)] {
            require((2..=16384).contains(&raw)&&(2..=16384).contains(&encoded)&&encoded==raw+raw%2,"source.encodedSize","Raw dimensions must be 2..16384 with only right/bottom even padding")?;
        }
        require(source.content_rect==Bounds {x:0,y:0,width:source.raw_size.width,height:source.raw_size.height},"source.contentRect","Content must cover exactly the raw pixels at origin zero")?;
        if let Some(b)=&source.observed_window_bounds {
            require((1..=i32::MAX as u32).contains(&b.width)&&(1..=i32::MAX as u32).contains(&b.height),"source.observedWindowBounds","Observed dimensions must be positive signed-32-bit values")?;
            require(b.x.checked_add(b.width as i32).is_some()&&b.y.checked_add(b.height as i32).is_some(),"source.observedWindowBounds","Observed bounds overflow signed coordinates")?;
        }
        crate::frame::geometry(&self.initial_defaults.frame_style,source.encoded_size.width,source.encoded_size.height)?;
        require(self.initial_defaults.camera.pan_transition_us<=project::MAX_DURATION_US&&self.initial_defaults.camera.settle_us<=project::MAX_DURATION_US,"initialDefaults.camera","Camera times must not exceed 24 hours")?;
        if let Some(d)=&self.diagnostic {require(!d.message.is_empty()&&d.message.len()<=1024,"diagnostic.message","Capture diagnostic must contain 1..1024 UTF-8 bytes")?;}
        if let WindowsCaptureStateV2::Finalized {video_timing,initial_project:doc,..}=&self.state {
            let duration=video_timing.endpoint.duration_us()?;video_timing.clock.validate(&video_timing.endpoint)?;
            project::validate(doc)?;
            require(doc.schema_version==3&&doc.revision==0&&doc.project_id==id,"initialProject","Windows snapshot must be schema 3, revision 0 and use the capture ID")?;
            require(doc.video.path=="screen.mp4"&&(doc.video.width,doc.video.height)==(source.encoded_size.width,source.encoded_size.height)&&doc.video.duration_us==duration,"initialProject.video","Video must match encoded dimensions, endpoint and screen.mp4")?;
            require(doc.clips.len()==1&&doc.clips[0].id=="clip-0"&&doc.clips[0].in_us==0&&doc.clips[0].out_us==duration&&doc.zooms.is_empty()&&doc.clicks.is_empty(),"initialProject.clips","Windows snapshot requires one full clip-0 and no automatic zooms or clicks")?;
            require(doc.video.cursor==CursorMode::Embedded&&doc.cursor.is_none()&&doc.cursor_settings==project::CursorSettings::default(),"initialProject.cursor","Windows snapshot requires embedded cursor, null cursor and default disabled cursor settings")?;
            require(doc.frame_style==self.initial_defaults.frame_style&&doc.camera==self.initial_defaults.camera,"initialProject","Initial frame and camera defaults must equal the frozen Start values")?;
            require(doc.microphone.as_ref().is_none_or(|mic|mic.path=="mic.wav"),"initialProject.microphone","Native microphone must reference mic.wav")?;
            require(doc.timing.provenance==TimingProvenance::LegacyUnknown&&doc.timing.audio_offset_us.is_none(),"initialProject.timing","Windows audio timing must remain legacyUnknown with null offset")?;
            require(doc.warnings.contains(&ProjectWarning::EmbeddedCursor)&&doc.warnings.contains(&ProjectWarning::LegacyTimingUnknown)
                &&doc.warnings.iter().all(|w|matches!(w,ProjectWarning::EmbeddedCursor|ProjectWarning::LegacyTimingUnknown|ProjectWarning::InvalidOptionalAudio))
                &&doc.warnings.iter().enumerate().all(|(i,w)|!doc.warnings[..i].contains(w)),"initialProject.warnings","Expected embeddedCursor and legacyTimingUnknown, optional invalidOptionalAudio, without duplicates")?;
        }
        Ok(())
    }
    pub fn finalized_project(&self)->CoreResult<&ProjectDocument> {
        match &self.state {WindowsCaptureStateV2::Finalized {initial_project,..}=>Ok(initial_project),_=>Err(recovery("Windows capture is incomplete"))}
    }
}
fn recovery(message:&str)->CoreError {invalid("capture.json",format!("{message}; retain capture files and recover or finalize before opening"))}
pub fn decode_windows(bytes:&[u8],id:&str)->CoreResult<WindowsCaptureManifestV2> {
    require(bytes.len() as u64<=MAX_JSON_BYTES,"capture.json","Native capture JSON exceeds 8 MiB")?;
    let manifest:WindowsCaptureManifestV2=serde_json::from_slice(bytes).map_err(|error|{let mut message=format!("Invalid Windows capture; retain files and correct capture.json: {error}");store::truncate_text(&mut message,1024);invalid("capture.json",message)})?;
    manifest.validate(id)?;Ok(manifest)
}
pub(super) fn read_marker(dir:&Path)->CoreResult<bool> {
    let Some(path)=asset(dir,MARKER_NAME)? else {return Ok(false)};
    let mut bytes=Vec::new();fs::File::open(path).and_then(|f|f.take(30).read_to_end(&mut bytes)).map_err(io_error)?;
    require(bytes==MARKER_BYTES,MARKER_NAME,"Windows intent marker is incomplete or invalid; retain files for recovery")?;Ok(true)
}
pub(super) fn import_windows(dir:&Path,manifest:WindowsCaptureManifestV2,ffmpeg:&Path)->CoreResult<ProjectDocument> {
    let WindowsCaptureStateV2::Finalized {video_timing,..}=&manifest.state else {return Err(recovery("Windows capture is incomplete"))};
    let video=asset(dir,"screen.mp4")?.ok_or_else(||recovery("Windows screen.mp4 is missing"))?;
    let grid=ffmpeg::probe_cfr_grid(ffmpeg,&video,video_timing.clock.completed_frames,&AtomicBool::new(false),Instant::now()+Duration::from_secs(600))?;
    validate_windows_finalized_assets(dir,&manifest,&grid)
}
pub fn validate_windows_finalized_assets(dir:&Path,manifest:&WindowsCaptureManifestV2,grid:&ffmpeg::SavedCfrGrid)->CoreResult<ProjectDocument> {
    let id=session_id(dir)?;manifest.validate(id)?;
    let WindowsCaptureStateV2::Finalized {video_timing,initial_project,..}=&manifest.state else {return Err(recovery("Windows capture is incomplete"))};
    asset(dir,"screen.mp4")?.ok_or_else(||recovery("Windows screen.mp4 is missing"))?;
    for packet in [grid.video.endpoint,grid.video.first_packet_duration] {
        endpoint_duration_us(packet.ticks,&TimeBase {numerator:packet.numerator,denominator:packet.denominator})?;
    }
    require(grid.completed_frames==video_timing.clock.completed_frames
        &&equal_time(grid.video.endpoint,video_timing.endpoint.packet_time())?
        &&equal_time(grid.video.first_packet_duration,ffmpeg::PacketTime {ticks:1,numerator:1,denominator:30})?
        &&(grid.video.video.width,grid.video.video.height)==(initial_project.video.width,initial_project.video.height)
        &&grid.video.video.duration_us==initial_project.video.duration_us,"video","Saved full packet grid does not match Windows capture authority")?;
    let mut doc=initial_project.clone();project_optional_audio(dir,&mut doc)?;Ok(doc)
}
fn session_id(dir:&Path)->CoreResult<&str> {
    if dir.canonicalize().map_err(io_error)?!=dir||!fs::symlink_metadata(dir).map_err(io_error)?.is_dir() {return Err(path_error("Capture requires a real session directory"));}
    dir.file_name().and_then(|v|v.to_str()).ok_or_else(||path_error("Capture directory must have a UTF-8 ID"))
}
/// The future recorder may start video only after this succeeds. Never remove the intent marker.
pub fn publish_windows_incomplete(dir:&Path,manifest:&WindowsCaptureManifestV2)->CoreResult<()> {
    require(matches!(manifest.state,WindowsCaptureStateV2::Incomplete {..}),"capture.json","First Windows publication requires incomplete state")?;
    publish_windows_with(dir,manifest,None,write_marker,|_|Ok(()),|_|Ok(()),sync_asset).map(|_|())
}
/// Caller must audit nondecreasing native timestamps, causal selection and publication frontier;
/// selected count = fully written count = N, full-cell cutoff, successful native stop and exact-child cleanup.
/// This checks supplied saved-grid evidence; it cannot reconstruct the worker's QPC history.
pub fn publish_windows_finalized_assets(dir:&Path,manifest:&WindowsCaptureManifestV2,grid:&ffmpeg::SavedCfrGrid)->CoreResult<WindowsCaptureManifestV2> {
    require(matches!(manifest.state,WindowsCaptureStateV2::Finalized {..}),"capture.json","Finalized publication requires finalized state")?;
    publish_windows_with(dir,manifest,Some(grid),write_marker,|_|Ok(()),|_|Ok(()),sync_asset)
}
fn write_marker(file:&mut fs::File)->std::io::Result<()> {file.write_all(MARKER_BYTES)?;file.flush()?;file.sync_all()}
fn sync_asset(path:&Path,directory:bool)->std::io::Result<()> {
    if directory {return store::sync_directory(path)}
    // Windows FlushFileBuffers requires write access; open existing assets without truncation.
    fs::OpenOptions::new().read(true).write(cfg!(windows)).open(path)?.sync_all()
}
// Private fault callbacks exercise the same publication ordering without filesystem-wide fault injection.
fn publish_windows_with(dir:&Path,manifest:&WindowsCaptureManifestV2,grid:Option<&ffmpeg::SavedCfrGrid>,
    marker_write:impl FnOnce(&mut fs::File)->std::io::Result<()>,before_write:impl FnOnce(&Path)->std::io::Result<()>,
    before_rename:impl FnOnce(&Path)->std::io::Result<()>,mut sync:impl FnMut(&Path,bool)->std::io::Result<()>)->CoreResult<WindowsCaptureManifestV2> {
    let id=session_id(dir)?;manifest.validate(id)?;
    let parent=dir.parent().ok_or_else(||path_error("Capture directory needs a parent"))?;
    let store=store::ProjectStore::new(parent)?;let _lock=store.lock(id,"document")?;
    let marker=read_marker(dir)?;
    let previous=if asset(dir,"capture.json")?.is_some() {read_authority(dir,id)?}else{None};
    match previous {
        Some(CaptureAuthority::WindowsV2(previous))=>{
            require(matches!(previous.state,WindowsCaptureStateV2::Incomplete {..}),"capture.json","Finalized Windows authority is immutable; reload before retrying")?;
            require(previous.capture_id==manifest.capture_id&&previous.source==manifest.source&&previous.initial_defaults==manifest.initial_defaults,"capture.json","Capture identity, source and Start defaults are immutable; reload before retrying")?;
        },
        Some(CaptureAuthority::LinuxV1(_))=>return Err(recovery("Windows publication cannot replace Linux authority")),
        None=>require(matches!(manifest.state,WindowsCaptureStateV2::Incomplete {..}),"capture.json","Finalization requires prior Windows incomplete authority")?,
    }
    let mut committed=manifest.clone();
    if let Some(grid)=grid {
        let doc=validate_windows_finalized_assets(dir,manifest,grid)?;
        if let WindowsCaptureStateV2::Finalized {initial_project,..}=&mut committed.state {*initial_project=doc;}
    }
    let bytes=serde_json::to_vec(&committed).map_err(|e|invalid("capture.json",e.to_string()))?;
    require(bytes.len() as u64<=MAX_JSON_BYTES,"capture.json","Native capture JSON exceeds 8 MiB")?;
    if !marker {
        // create_new refuses aliases/conflicts; even a failed short write permanently prevents legacy import.
        let mut file=fs::OpenOptions::new().write(true).create_new(true).open(dir.join(MARKER_NAME)).map_err(io_error)?;
        marker_write(&mut file).map_err(io_error)?;
    } else {sync(&dir.join(MARKER_NAME),false).map_err(io_error)?;}
    sync(dir,true).map_err(io_error)?;
    if let WindowsCaptureStateV2::Finalized {initial_project,..}=&committed.state {
        sync(&asset(dir,"screen.mp4")?.ok_or_else(||recovery("Windows screen.mp4 is missing"))?,false).map_err(io_error)?;
        if initial_project.microphone.is_some() {sync(&asset(dir,"mic.wav")?.ok_or_else(||recovery("Retained mic.wav is missing"))?,false).map_err(io_error)?;}
        sync(dir,true).map_err(io_error)?;
    }
    store::replace_bytes_with(dir,"capture.json",&bytes,before_write,before_rename)?;
    let durability=(|| {sync(dir,true)?;if matches!(committed.state,WindowsCaptureStateV2::Incomplete {..}) {sync(parent,true)?;}Ok::<(),std::io::Error>(())})();
    durability.map_err(|error|CoreError {code:ErrorCode::Io,message:format!("Capture authority was committed, but directory synchronization failed; durability is unconfirmed. Reload capture.json before retrying: {error}"),field:Some("capture.json".into())})?;
    Ok(committed)
}

#[cfg(test)]
mod tests;
