use serde::{Deserialize, Serialize};
use crate::{CoreResult, CoreError, ErrorCode};
pub const MAX_DURATION_US: u64 = 86_400_000_000;
pub const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_REVISION: u64 = 9_007_199_254_740_991;
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDocument {
    pub frame_style: FrameStyle,
    pub schema_version: u32,
    pub project_id: String,
    pub revision: u64,
    pub video: VideoAsset,
    pub cursor: Option<CursorAsset>,
    pub cursor_settings: CursorSettings,
    pub audio_tracks: Option<AudioTracks>,
    pub microphone: Option<AudioAsset>,
    pub timing: Timing,
    pub camera: CameraDefaults,
    pub clips: Vec<Clip>,
    pub zooms: Vec<ZoomElement>,
    pub clicks: Vec<Click>,
    pub warnings: Vec<ProjectWarning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoAsset {
    pub cursor: CursorMode,
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub duration_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioAsset {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioTracks {
    #[serde(deserialize_with="required_option")]
    pub microphone: Option<AudioTrack>,
    #[serde(deserialize_with="required_option")]
    pub system: Option<AudioTrack>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTrackKind { Microphone, System }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioTrack {
    pub path: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_count: u64,
    pub timing: AudioTrackTiming,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag="type", rename_all="camelCase", deny_unknown_fields)]
pub enum AudioTrackTiming {
    Unknown {},
    Estimated { #[serde(rename="offsetUs")] offset_us: i64, #[serde(rename="uncertaintyUs")] uncertainty_us: u64 },
    Mapped { #[serde(rename="clockDomain")] clock_domain: AudioClockDomain, #[serde(rename="ticksPerSecond")] ticks_per_second: u64, provenance: AudioTimingProvenance, segments: Vec<AudioTimingSegment> },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum AudioClockDomain { Synthetic, Qpc, Monotonic }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum AudioTimingProvenance { Synthetic, Native }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct AudioTimingSegment {
    pub generation: u32,
    pub source_start_frame: u64,
    pub source_end_frame: u64,
    pub native_start_tick: u64,
    pub native_end_tick: u64,
    pub video_start_us: i64,
    pub video_end_us: i64,
    pub uncertainty_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Timing {
    pub provenance: TimingProvenance,
    pub audio_offset_us: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraDefaults {
    pub pan_transition_us: u64,
    pub settle_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Clip {
    pub id: String,
    pub in_us: u64,
    pub out_us: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ZoomElement {
    pub mode: ZoomMode,
    pub id: String,
    pub clip_id: String,
    pub start_us: u64,
    pub end_us: u64,
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub intro_us: u64,
    pub hold_us: u64,
    pub outro_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ZoomMode { Fixed, Follow }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Click {
    pub at_us: u64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimingProvenance {
    LegacyUnknown,
    Measured,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectWarning {
    InvalidMicrophoneAudio,
    InvalidSystemAudio,
    MicrophoneTimingUnknown,
    SystemTimingUnknown,
    MicrophoneTimingEstimated,
    SystemTimingEstimated,
    LegacyTimingUnknown,
    EmbeddedCursor,
    InvalidOptionalAudio,
    CursorCaptureIncomplete,
    LegacyScaleOverridden,
    LegacyMetadataMissing,
    LegacyInteractionClipped,
    LegacyZoomOverlapNormalized,
}
pub fn validate(doc: &ProjectDocument) -> CoreResult<()> {
    if !matches!(doc.schema_version, 1 | 2 | 3 | 4 | 5) {
        return Err(CoreError { code: ErrorCode::UnsupportedSchema, message: "Only project schema versions 1, 2, 3, 4 and 5 are supported".into(), field: Some("schemaVersion".into()) });
    }
    if doc.schema_version == 5 {
        require(doc.microphone.is_none() && doc.timing.provenance == TimingProvenance::LegacyUnknown && doc.timing.audio_offset_us.is_none(), "audioTracks", "Schema 5 cannot contain legacy microphone/timing metadata")?;
        let tracks = doc.audio_tracks.as_ref().ok_or_else(|| invalid("audioTracks", "Schema 5 requires audioTracks"))?;
        for (name, track) in [("microphone", &tracks.microphone), ("system", &tracks.system)] {
            if let Some(track) = track { validate_audio_track(track, &format!("audioTracks.{name}"))?; }
        }
    } else {
        require(doc.audio_tracks.is_none(), "audioTracks", "Legacy schemas cannot contain audioTracks")?;
        require(doc.warnings.iter().all(legacy_warning), "warnings", "Legacy schemas cannot contain schema 5 audio warnings")?;
    }
    require(doc.schema_version != 1 || doc.frame_style == FrameStyle::default(), "frameStyle", "Schema 1 requires the identity frame style")?;
    validate_cursor_settings(&doc.cursor_settings)?;
    require(doc.schema_version >= 3 || (doc.video.cursor == CursorMode::Embedded && doc.cursor.is_none() && doc.cursor_settings == CursorSettings::default()), "cursor", "Old schemas require embedded cursor defaults")?;
    match doc.video.cursor {
        CursorMode::Separate => {
            let asset = doc.cursor.as_ref().ok_or_else(|| invalid("cursor", "Separate cursor requires an index reference"))?;
            require(valid_asset_name(&asset.index_path) && asset.index_path.len() <= 512, "cursor.indexPath", "Expected a relative cursor index path of at most 512 bytes")?;
        }
        CursorMode::Embedded | CursorMode::Absent => require(doc.cursor.is_none() && !doc.cursor_settings.enabled, "cursorSettings.enabled", "Only separate cursor video can enable a cursor overlay")?,
    }
    crate::frame::geometry(&doc.frame_style, doc.video.width, doc.video.height)?;
    require(valid_id(&doc.project_id), "projectId", "Invalid project ID")?;
    require(doc.revision <= MAX_REVISION, "revision", "Revision exceeds the supported integer range")?;
    require(valid_asset_name(&doc.video.path), "video.path", "Expected a relative asset path")?;
    validate_video(doc.video.width, doc.video.height, doc.video.duration_us)?;
    if let Some(audio) = &doc.microphone {
        require(valid_asset_name(&audio.path), "microphone.path", "Expected a relative asset path")?;
    }
    if doc.timing.provenance == TimingProvenance::LegacyUnknown {
        require(
            doc.timing.audio_offset_us.is_none(),
            "timing.audioOffsetUs",
            "Legacy timing has no measured audio offset",
        )?;
    }
    if let Some(offset) = doc.timing.audio_offset_us {
        require(
            offset.unsigned_abs() <= MAX_DURATION_US,
            "timing.audioOffsetUs",
            "Audio offset exceeds 24 hours",
        )?;
    }
    require(
        doc.camera.pan_transition_us <= MAX_DURATION_US,
        "camera.panTransitionUs",
        "Transition exceeds 24 hours",
    )?;
    require(doc.camera.settle_us <= MAX_DURATION_US, "camera.settleUs", "Settle time exceeds 24 hours")?;
    require(!doc.clips.is_empty() && doc.clips.len() <= 4096, "clips", "Expected between 1 and 4096 clips")?;
    require(doc.zooms.len() <= 16384, "zooms", "At most 16384 zooms are supported")?;
    let mut ids = std::collections::HashSet::new();
    let mut output_duration = 0_u64;
    for (i, clip) in doc.clips.iter().enumerate() {
        let field = format!("clips[{i}]");
        require(
            valid_id(&clip.id) && ids.insert(clip.id.as_str()),
            &format!("{field}.id"),
            "Clip IDs must be valid and unique",
        )?;
        require(
            clip.in_us < clip.out_us,
            &format!("{field}.inUs"),
            "Clip must contain a nonempty source interval",
        )?;
        require(
            clip.out_us <= doc.video.duration_us,
            &format!("{field}.outUs"),
            "Clip exceeds source duration",
        )?;
        output_duration = output_duration.checked_add(clip.out_us - clip.in_us).ok_or_else(|| invalid("clips", "Output duration overflow"))?;
    }
    require(output_duration <= MAX_DURATION_US, "clips", "Output duration exceeds 24 hours")?;
    ids.clear();
    for (i, zoom) in doc.zooms.iter().enumerate() {
        let field = format!("zooms[{i}]");
        require(
            valid_id(&zoom.id) && ids.insert(zoom.id.as_str()),
            &format!("{field}.id"),
            "Zoom IDs must be valid and unique",
        )?;
        validate_zoom(zoom, doc, &field)?;
    }
    let mut spans: Vec<_> = doc.zooms.iter().enumerate().collect();
    spans.sort_unstable_by_key(|(_, zoom)| (zoom.clip_id.as_str(), zoom.start_us));
    for pair in spans.windows(2) {
        let (_, previous) = pair[0];
        let (index, next) = pair[1];
        require(
            previous.clip_id != next.clip_id || previous.end_us <= next.start_us,
            &format!("zooms[{index}].startUs"),
            "Active zoom spans must not overlap within a clip",
        )?;
    }
    for (i, click) in doc.clicks.iter().enumerate() {
        let field = format!("clicks[{i}]");
        require(
            click.at_us <= doc.video.duration_us,
            &format!("{field}.atUs"),
            "Click exceeds source duration",
        )?;
        coordinates(click.x, click.y, &field)?;
    }
    Ok(())
}

pub(crate) fn validate_zoom(zoom: &ZoomElement, doc: &ProjectDocument, field: &str) -> CoreResult<()> {
    require(
        zoom.mode == ZoomMode::Fixed || (doc.schema_version >= 4 && doc.video.cursor == CursorMode::Separate && doc.cursor.is_some()),
        &format!("{field}.mode"),
        "Follow zoom requires schema 4 or later and separate cursor telemetry",
    )?;
    require(valid_id(&zoom.id), &format!("{field}.id"), "Invalid zoom ID")?;
    let clip = doc.clips.iter().find(|clip| clip.id == zoom.clip_id).ok_or_else(|| invalid(&format!("{field}.clipId"), "Zoom references an unknown clip"))?;
    require(
        zoom.start_us >= clip.in_us && zoom.start_us < zoom.end_us,
        &format!("{field}.startUs"),
        "Zoom start must be inside its clip and before its end",
    )?;
    require(zoom.end_us <= clip.out_us, &format!("{field}.endUs"), "Zoom end exceeds its clip")?;
    coordinates(zoom.x, zoom.y, field)?;
    require(
        zoom.scale.is_finite() && (1.0..=8.0).contains(&zoom.scale),
        &format!("{field}.scale"),
        "Scale must be finite and between 1 and 8",
    )?;
    for (name, value) in [("introUs", zoom.intro_us), ("holdUs", zoom.hold_us), ("outroUs", zoom.outro_us)] {
        require(value <= MAX_DURATION_US, &format!("{field}.{name}"), "Envelope time exceeds 24 hours")?;
    }
    require(
        zoom.end_us.checked_add(zoom.hold_us).and_then(|end| end.checked_add(zoom.outro_us)).is_some(),
        &field,
        "Zoom envelope overflow",
    )?;
    Ok(())
}

pub(crate) fn validate_video(width: u32, height: u32, duration_us: u64) -> CoreResult<()> {
    require((1..=16384).contains(&width), "video.width", "Width must be between 1 and 16384")?;
    require((1..=16384).contains(&height), "video.height", "Height must be between 1 and 16384")?;
    require(
        (1..=MAX_DURATION_US).contains(&duration_us),
        "video.durationUs",
        "Video duration must be positive and at most 24 hours",
    )
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
pub(crate) fn valid_asset_name(path: &str) -> bool {
    !path.contains(['\\', ':', '\0']) && !path.split('/').any(|part| part.is_empty() || part == "." || part == "..")
}
fn coordinates(x: f64, y: f64, field: &str) -> CoreResult<()> {
    require(
        x.is_finite() && (0.0..=1.0).contains(&x),
        &format!("{field}.x"),
        "X must be finite and normalized",
    )?;
    require(
        y.is_finite() && (0.0..=1.0).contains(&y),
        &format!("{field}.y"),
        "Y must be finite and normalized",
    )
}
pub(crate) fn require(condition: bool, field: &str, message: &str) -> CoreResult<()> {
    if condition { Ok(()) } else { Err(invalid(field, message)) }
}
pub(crate) fn invalid(field: &str, message: impl Into<String>) -> CoreError {
    CoreError { code: ErrorCode::InvalidDocument, message: message.into(), field: Some(field.into()) }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameStyle {
    pub canvas: CanvasFormat,
    pub background: FrameBackground,
    pub padding_px: u32,
    pub corner_radius_px: u32,
    pub shadow: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CanvasFormat { Source, Landscape, Portrait, Square }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum FrameBackground { Solid { color: String }, Gradient { start: String, end: String } }
impl Default for FrameStyle {
    fn default() -> Self { Self { canvas: CanvasFormat::Source,
        background: FrameBackground::Solid { color: "#0b0d10".into() },
        padding_px: 0, corner_radius_px: 0, shadow: false } }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditableState {
    pub clips: Vec<Clip>,
    #[serde(deserialize_with="deserialize_edit_zooms")]
    pub zooms: Vec<ZoomElement>,
    pub frame_style: FrameStyle,
    pub cursor_settings: CursorSettings,
}
impl EditableState {
    pub fn from_document(doc: &ProjectDocument) -> Self {
        Self { clips: doc.clips.clone(), zooms: doc.zooms.clone(), frame_style: doc.frame_style.clone(), cursor_settings: doc.cursor_settings.clone() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum CursorMode { Embedded, Separate, Absent }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum CursorStyle { Native, Arrow }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct CursorAsset { pub index_path: String }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct CursorSettings { pub enabled: bool, pub scale: f64, pub smoothing_us: u64, pub style: CursorStyle }
impl Default for CursorSettings {
    fn default()->Self { Self {enabled:false,scale:1.0,smoothing_us:0,style:CursorStyle::Native} }
}
pub fn validate_cursor_settings(settings:&CursorSettings)->CoreResult<()> {
    require(settings.scale.is_finite() && (0.5..=4.0).contains(&settings.scale),"cursorSettings.scale","Cursor scale must be finite and between 0.5 and 4")?;
    require(settings.smoothing_us<=500_000,"cursorSettings.smoothingUs","Cursor smoothing must be between 0 and 500000 microseconds")
}

// Old project files are closed fixed-only objects; only edit/history inputs may omit mode.
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
struct FixedZoom {
    id: String,
    clip_id: String,
    start_us: u64,
    end_us: u64,
    x: f64,
    y: f64,
    scale: f64,
    intro_us: u64,
    hold_us: u64,
    outro_us: u64,
}
impl From<FixedZoom> for ZoomElement {
    fn from(z: FixedZoom) -> Self {
        Self { mode: ZoomMode::Fixed, id: z.id, clip_id: z.clip_id, start_us: z.start_us,
            end_us: z.end_us, x: z.x, y: z.y, scale: z.scale, intro_us: z.intro_us,
            hold_us: z.hold_us, outro_us: z.outro_us }
    }
}
impl From<&ZoomElement> for FixedZoom {
    fn from(z: &ZoomElement) -> Self {
        Self { id: z.id.clone(), clip_id: z.clip_id.clone(), start_us: z.start_us,
            end_us: z.end_us, x: z.x, y: z.y, scale: z.scale, intro_us: z.intro_us,
            hold_us: z.hold_us, outro_us: z.outro_us }
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum EditZoom { Explicit(ZoomElement), Legacy(FixedZoom) }
impl From<EditZoom> for ZoomElement {
    fn from(z: EditZoom) -> Self {
        match z { EditZoom::Explicit(z) => z, EditZoom::Legacy(z) => z.into() }
    }
}
pub(crate) fn deserialize_edit_zoom<'de, D: serde::Deserializer<'de>>(de: D) -> Result<ZoomElement, D::Error> {
    EditZoom::deserialize(de).map(Into::into)
}
fn deserialize_edit_zooms<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<ZoomElement>, D::Error> {
    Vec::<EditZoom>::deserialize(de).map(|zooms| zooms.into_iter().map(Into::into).collect())
}

// Separate wire DTOs keep cursor defaults exclusive to old schemas.
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct OldVideo { path:String, width:u32, height:u32, duration_us:u64 }
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct SchemaOneDocument {
    #[serde(default)]
    frame_style: FrameStyle,
    #[serde(deserialize_with="schema_1")]
    schema_version: u32,
    project_id: String,
    revision: u64,
    video: OldVideo,
    microphone: Option<AudioAsset>,
    timing: Timing,
    camera: CameraDefaults,
    clips: Vec<Clip>,
    zooms: Vec<FixedZoom>,
    clicks: Vec<Click>,
    #[serde(deserialize_with="legacy_warnings")]
    warnings: Vec<ProjectWarning>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct SchemaTwoDocument {
    frame_style: FrameStyle,
    #[serde(deserialize_with="schema_2")]
    schema_version: u32,
    project_id: String,
    revision: u64,
    video: OldVideo,
    microphone: Option<AudioAsset>,
    timing: Timing,
    camera: CameraDefaults,
    clips: Vec<Clip>,
    zooms: Vec<FixedZoom>,
    clicks: Vec<Click>,
    #[serde(deserialize_with="legacy_warnings")]
    warnings: Vec<ProjectWarning>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct SchemaThreeDocument {
    frame_style: FrameStyle,
    #[serde(deserialize_with="schema_3")]
    schema_version: u32,
    project_id: String,
    revision: u64,
    video: VideoAsset,
    #[serde(deserialize_with="required_option")]
    cursor: Option<CursorAsset>,
    cursor_settings: CursorSettings,
    microphone: Option<AudioAsset>,
    timing: Timing,
    camera: CameraDefaults,
    clips: Vec<Clip>,
    zooms: Vec<FixedZoom>,
    clicks: Vec<Click>,
    #[serde(deserialize_with="legacy_warnings")]
    warnings: Vec<ProjectWarning>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct CurrentDocument {
    frame_style: FrameStyle,
    #[serde(deserialize_with="schema_4")]
    schema_version: u32,
    project_id: String,
    revision: u64,
    video: VideoAsset,
    #[serde(deserialize_with="required_option")]
    cursor: Option<CursorAsset>,
    cursor_settings: CursorSettings,
    microphone: Option<AudioAsset>,
    timing: Timing,
    camera: CameraDefaults,
    clips: Vec<Clip>,
    zooms: Vec<ZoomElement>,
    clicks: Vec<Click>,
    #[serde(deserialize_with="legacy_warnings")]
    warnings: Vec<ProjectWarning>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
struct SchemaFiveDocument {
    frame_style: FrameStyle,
    #[serde(deserialize_with="schema_5")]
    schema_version: u32,
    project_id: String,
    revision: u64,
    video: VideoAsset,
    #[serde(deserialize_with="required_option")]
    cursor: Option<CursorAsset>,
    cursor_settings: CursorSettings,
    audio_tracks: AudioTracks,
    camera: CameraDefaults,
    clips: Vec<Clip>,
    zooms: Vec<ZoomElement>,
    clicks: Vec<Click>,
    warnings: Vec<ProjectWarning>,
}
fn legacy_warning(warning: &ProjectWarning) -> bool {
    !matches!(warning, ProjectWarning::InvalidMicrophoneAudio | ProjectWarning::InvalidSystemAudio | ProjectWarning::MicrophoneTimingUnknown | ProjectWarning::SystemTimingUnknown | ProjectWarning::MicrophoneTimingEstimated | ProjectWarning::SystemTimingEstimated)
}
fn legacy_warnings<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<ProjectWarning>, D::Error> {
    let warnings = Vec::<ProjectWarning>::deserialize(de)?;
    if warnings.iter().all(legacy_warning) { Ok(warnings) } else { Err(serde::de::Error::custom("Legacy schema cannot contain schema 5 warnings")) }
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(de: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(de)
}
impl<'de> Deserialize<'de> for ProjectDocument {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Five(SchemaFiveDocument),
            Current(CurrentDocument),
            Three(SchemaThreeDocument),
            Two(SchemaTwoDocument),
            One(SchemaOneDocument),
        }
        match Wire::deserialize(de)? {
            Wire::Five(d) => Ok(Self {
                frame_style:d.frame_style, schema_version:d.schema_version, project_id:d.project_id, revision:d.revision,
                video:d.video, cursor:d.cursor, cursor_settings:d.cursor_settings, audio_tracks:Some(d.audio_tracks),
                microphone:None, timing:Timing {provenance:TimingProvenance::LegacyUnknown,audio_offset_us:None},
                camera:d.camera, clips:d.clips, zooms:d.zooms, clicks:d.clicks, warnings:d.warnings,
            }),
            Wire::Current(d) => Ok(Self {
                frame_style: d.frame_style,
                schema_version: d.schema_version,
                project_id: d.project_id,
                revision: d.revision,
                video: d.video,
                cursor: d.cursor,
                cursor_settings: d.cursor_settings,
                audio_tracks: None,
                microphone: d.microphone,
                timing: d.timing,
                camera: d.camera,
                clips: d.clips,
                zooms: d.zooms,
                clicks: d.clicks,
                warnings: d.warnings,
            }),
            Wire::Three(d) => Ok(Self {
                frame_style: d.frame_style,
                schema_version: d.schema_version,
                project_id: d.project_id,
                revision: d.revision,
                video: d.video,
                cursor: d.cursor,
                cursor_settings: d.cursor_settings,
                audio_tracks: None,
                microphone: d.microphone,
                timing: d.timing,
                camera: d.camera,
                clips: d.clips,
                zooms: d.zooms.into_iter().map(Into::into).collect(),
                clicks: d.clicks,
                warnings: d.warnings,
            }),
            Wire::Two(d) => Ok(Self {
                frame_style: d.frame_style,
                schema_version: d.schema_version,
                project_id: d.project_id,
                revision: d.revision,
                video: VideoAsset {
                    cursor: CursorMode::Embedded,
                    path: d.video.path,
                    width: d.video.width,
                    height: d.video.height,
                    duration_us: d.video.duration_us,
                },
                cursor: None,
                cursor_settings: CursorSettings::default(),
                audio_tracks: None,
                microphone: d.microphone,
                timing: d.timing,
                camera: d.camera,
                clips: d.clips,
                zooms: d.zooms.into_iter().map(Into::into).collect(),
                clicks: d.clicks,
                warnings: d.warnings,
            }),
            Wire::One(d) => Ok(Self {
                frame_style: d.frame_style,
                schema_version: d.schema_version,
                project_id: d.project_id,
                revision: d.revision,
                video: VideoAsset {
                    cursor: CursorMode::Embedded,
                    path: d.video.path,
                    width: d.video.width,
                    height: d.video.height,
                    duration_us: d.video.duration_us,
                },
                cursor: None,
                cursor_settings: CursorSettings::default(),
                audio_tracks: None,
                microphone: d.microphone,
                timing: d.timing,
                camera: d.camera,
                clips: d.clips,
                zooms: d.zooms.into_iter().map(Into::into).collect(),
                clicks: d.clicks,
                warnings: d.warnings,
            }),
        }
    }
}
impl Serialize for ProjectDocument {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        if !matches!(self.schema_version,1..=5){return Err(serde::ser::Error::custom("Unsupported project schema version"));}
        if self.schema_version < 4 && self.zooms.iter().any(|zoom| zoom.mode == ZoomMode::Follow) {
            return Err(serde::ser::Error::custom("Old project schemas cannot represent follow zooms"));
        }
        if self.schema_version <= 4 && (self.audio_tracks.is_some() || !self.warnings.iter().all(legacy_warning)) {
            return Err(serde::ser::Error::custom("Legacy schema cannot represent schema 5 audio metadata"));
        }
        if self.schema_version == 5 && (self.audio_tracks.is_none() || self.microphone.is_some() || self.timing.provenance != TimingProvenance::LegacyUnknown || self.timing.audio_offset_us.is_some()) {
            return Err(serde::ser::Error::custom("Schema 5 requires audioTracks and cannot represent legacy timing"));
        }
        let mut state = serializer.serialize_struct("ProjectDocument", if self.schema_version < 3 { 12 } else if self.schema_version == 5 { 13 } else { 14 })?;
        state.serialize_field("frameStyle", &self.frame_style)?;
        state.serialize_field("schemaVersion", &self.schema_version)?;
        state.serialize_field("projectId", &self.project_id)?;
        state.serialize_field("revision", &self.revision)?;
        if self.schema_version < 3 {
            state.serialize_field("video", &OldVideo {
                path: self.video.path.clone(), width: self.video.width,
                height: self.video.height, duration_us: self.video.duration_us,
            })?;
        } else {
            state.serialize_field("video", &self.video)?;
        }
        if self.schema_version >= 3 { state.serialize_field("cursor", &self.cursor)?; }
        if self.schema_version >= 3 { state.serialize_field("cursorSettings", &self.cursor_settings)?; }
        if self.schema_version == 5 {
            state.serialize_field("audioTracks", &self.audio_tracks)?;
        } else {
            state.serialize_field("microphone", &self.microphone)?;
            state.serialize_field("timing", &self.timing)?;
        }
        state.serialize_field("camera", &self.camera)?;
        state.serialize_field("clips", &self.clips)?;
        if self.schema_version < 4 {
            state.serialize_field("zooms", &self.zooms.iter().map(FixedZoom::from).collect::<Vec<_>>())?;
        } else {
            state.serialize_field("zooms", &self.zooms)?;
        }
        state.serialize_field("clicks", &self.clicks)?;
        state.serialize_field("warnings", &self.warnings)?;
        state.end()
    }
}
fn schema_version<'de, D: serde::Deserializer<'de>>(de: D, expected: u32) -> Result<u32, D::Error> {
    let value = u32::deserialize(de)?;
    if value == expected { Ok(value) } else { Err(serde::de::Error::custom("Unexpected schema version")) }
}
fn schema_1<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u32, D::Error> { schema_version(de, 1) }
fn schema_2<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u32, D::Error> { schema_version(de, 2) }
fn schema_3<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u32, D::Error> { schema_version(de, 3) }

fn schema_4<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u32, D::Error> { schema_version(de, 4) }

fn schema_5<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u32, D::Error> { schema_version(de, 5) }

fn validate_audio_track(track: &AudioTrack, field: &str) -> CoreResult<()> {
    require(valid_asset_name(&track.path) && track.path.len() <= 512, &format!("{field}.path"), "Expected a contained relative asset path of at most 512 bytes")?;
    require((8000..=192000).contains(&track.sample_rate), &format!("{field}.sampleRate"), "Sample rate must be between 8000 and 192000")?;
    require((1..=2).contains(&track.channels), &format!("{field}.channels"), "Expected mono or stereo audio")?;
    require(track.frame_count > 0 && u128::from(track.frame_count) <= u128::from(track.sample_rate) * u128::from(MAX_DURATION_US) / 1_000_000, &format!("{field}.frameCount"), "Frame count must be positive and at most 24 hours")?;
    let bounded = |value: i64| value.unsigned_abs() <= MAX_DURATION_US;
    match &track.timing {
        AudioTrackTiming::Unknown {} => (),
        AudioTrackTiming::Estimated {offset_us,uncertainty_us} => {
            require(bounded(*offset_us), &format!("{field}.timing.offsetUs"), "Offset exceeds 24 hours")?;
            require(*uncertainty_us <= MAX_DURATION_US, &format!("{field}.timing.uncertaintyUs"), "Uncertainty exceeds 24 hours")?;
        }
        AudioTrackTiming::Mapped {ticks_per_second,segments,..} => {
            require((1..=1_000_000_000).contains(ticks_per_second), &format!("{field}.timing.ticksPerSecond"), "Tick frequency must be between 1 and 1000000000")?;
            require((1..=1024).contains(&segments.len()), &format!("{field}.timing.segments"), "Expected between 1 and 1024 measured segments")?;
            for (index,segment) in segments.iter().enumerate() {
                let f=format!("{field}.timing.segments[{index}]");
                require(segment.source_start_frame < segment.source_end_frame && segment.source_end_frame <= track.frame_count, &format!("{f}.sourceEndFrame"), "Source interval must be nonempty and inside the WAV")?;
                require(segment.native_start_tick < segment.native_end_tick && segment.native_end_tick <= MAX_REVISION, &format!("{f}.nativeEndTick"), "Native interval must increase within the supported integer range")?;
                require(bounded(segment.video_start_us) && bounded(segment.video_end_us) && segment.video_start_us < segment.video_end_us, &format!("{f}.videoEndUs"), "Video interval must increase within 24 hours of video zero")?;
                require(segment.uncertainty_us <= MAX_DURATION_US, &format!("{f}.uncertaintyUs"), "Uncertainty exceeds 24 hours")?;
                let source=u128::from(segment.source_end_frame-segment.source_start_frame)*1_000_000;
                let nominal=u128::from(track.sample_rate)*u128::try_from(i128::from(segment.video_end_us)-i128::from(segment.video_start_us)).map_err(|_|invalid(&f,"Invalid interval"))?;
                require(nominal*1000 >= source*995 && nominal*1000 <= source*1005, &format!("{f}.videoEndUs"), "Mapped video duration / nominal source duration must be within 0.995..1.005; segment producer discontinuities explicitly")?;
                if let Some(previous)=index.checked_sub(1).map(|i|&segments[i]) {
                    require(segment.source_start_frame >= previous.source_end_frame, &format!("{f}.sourceStartFrame"), "Source intervals must never overlap or move backwards")?;
                    require(segment.video_start_us >= previous.video_end_us, &format!("{f}.videoStartUs"), "Video intervals must never overlap or move backwards")?;
                    require(segment.generation >= previous.generation, &format!("{f}.generation"), "Generations cannot decrease")?;
                    require(segment.generation > previous.generation || segment.native_start_tick >= previous.native_end_tick, &format!("{f}.nativeStartTick"), "Native ticks may reset only in a new generation")?;
                }
            }
        }
    }
    Ok(())
}
