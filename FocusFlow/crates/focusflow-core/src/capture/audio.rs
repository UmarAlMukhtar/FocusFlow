//! Linux V4 wraps the existing video authority and retains unplaced system audio.
use super::{
    asset, ffmpeg, invalid, io_error, path_error, project, require, store, CaptureAuthority,
    CaptureManifestV1, CaptureStateV1, ProjectDocument, ProjectWarning, UniqueJson, MAX_JSON_BYTES,
};
use crate::CoreResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    path::Path,
};

const MARKER: &str = ".focusflow-audio-v4-intent";
const MARKER_BYTES: &[u8] = b"focusflow-audio-v4-intent\n";
const MICROPHONE: &str = "mic.wav";
const SYSTEM: &str = "audio/system/system.wav";
const EVIDENCE: &str = "audio/system/system.evidence.partial.json";
const MAX_EVIDENCE_BYTES: u64 = 1024 * 1024;
const SYSTEM_WAV_HEADER_BYTES: u64 = 68;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEnvelopeV4 {
    pub schema_version: u32,
    pub capture_id: String,
    pub state: AudioCaptureStateV4,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum AudioCaptureStateV4 {
    Incomplete {
        video_authority: VideoAuthority,
        audio_requested: AudioRequested,
    },
    Finalized {
        video_authority: VideoAuthority,
        audio_requested: AudioRequested,
        audio: AudioRecords,
        initial_project: ProjectDocument,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum VideoAuthority {
    LinuxV1 { manifest: CaptureManifestV1 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioRequested {
    pub microphone: bool,
    pub system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioRecords {
    pub microphone: AudioRecord<MicrophoneAsset>,
    pub system: AudioRecord<SystemAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum AudioRecord<T> {
    Disabled {},
    Unavailable { reason: String },
    Validated { asset: T },
}

literals!(MicrophoneProducer { CpalMicrophoneV1 => "cpalMicrophoneV1" });
literals!(SystemProducer { LinuxSystemV1 => "linuxSystemV1" });
literals!(MicrophoneRenderPolicy { LegacyZero => "legacyZero" });
literals!(SystemRenderPolicy { WithheldUnverified => "withheldUnverified" });
literals!(OriginPolicy { DiscardWarmupUntilCommit => "discardWarmupUntilCommit" });
literals!(SampleVideoRelation { Unknown => "unknown" });
literals!(WavEncoding { PcmInteger => "pcmInteger", IeeeFloat => "ieeeFloat" });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum UnplacedTiming {
    Unknown {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MicrophoneAsset {
    pub path: String,
    pub producer: MicrophoneProducer,
    pub render_policy: MicrophoneRenderPolicy,
    pub timing: UnplacedTiming,
    pub inventory: WavInventory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemAsset {
    pub path: String,
    pub producer: SystemProducer,
    pub render_policy: SystemRenderPolicy,
    pub timing: UnplacedTiming,
    pub inventory: WavInventory,
    pub evidence_path: String,
    pub origin: SystemOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemOrigin {
    pub policy: OriginPolicy,
    pub sample_video_relation: SampleVideoRelation,
    pub selection_epoch: String,
    pub admission_id: String,
    pub parent_send_ns: u64,
    pub child_commit_before_ns: u64,
    pub child_commit_after_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WavInventory {
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: WavEncoding,
    pub bits_per_sample: u16,
    pub frame_count: u64,
    pub file_bytes: u64,
}

impl WavInventory {
    fn validate(&self, field: &str) -> CoreResult<()> {
        require(
            (8_000..=192_000).contains(&self.sample_rate)
                && (1..=2).contains(&self.channels)
                && matches!((&self.encoding, self.bits_per_sample),
                    (WavEncoding::PcmInteger, 16 | 24 | 32) | (WavEncoding::IeeeFloat, 32)),
            field,
            "WAV inventory requires a supported rate, mono or stereo, and PCM integer or finite float encoding",
        )?;
        require(
            self.frame_count > 0 && self.frame_count <= u64::from(self.sample_rate) * 86_400,
            field,
            "WAV inventory requires a positive frame count within 24 hours",
        )?;
        let minimum_bytes = 44 + self.frame_count * u64::from(self.channels) * u64::from(self.bits_per_sample / 8);
        require(
            (minimum_bytes..=u64::from(u32::MAX) + 8).contains(&self.file_bytes),
            field,
            "WAV inventory byte count must hold the declared samples within RIFF limits",
        )
    }
}

fn validate_request<T>(record: &AudioRecord<T>, requested: bool, field: &str) -> CoreResult<()> {
    require(
        requested != matches!(record, AudioRecord::Disabled {}),
        field,
        "Audio record must agree with the original request",
    )?;
    if let AudioRecord::Unavailable { reason } = record {
        require(!reason.trim().is_empty() && reason.len() <= 1024, field, "Audio failure reason must contain 1..1024 UTF-8 bytes")?;
    }
    Ok(())
}

fn valid_correlation_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn derive(video: &CaptureManifestV1, requested: AudioRequested, audio: &AudioRecords) -> CoreResult<ProjectDocument> {
    video.validate(&video.capture_id)?;
    let mut doc = video.finalized_project()?.clone();
    validate_request(&audio.microphone, requested.microphone, "audio.microphone")?;
    validate_request(&audio.system, requested.system, "audio.system")?;
    let legacy_mic_failed = doc.warnings.contains(&ProjectWarning::InvalidOptionalAudio);
    doc.warnings.retain(|warning| !matches!(warning, ProjectWarning::InvalidOptionalAudio | ProjectWarning::LegacyTimingUnknown));
    let microphone = match &audio.microphone {
        AudioRecord::Disabled {} => {
            require(doc.microphone.is_none() && !legacy_mic_failed, "audio.microphone", "Disabled microphone conflicts with the video snapshot")?;
            None
        }
        AudioRecord::Unavailable { .. } => {
            doc.warnings.push(ProjectWarning::InvalidMicrophoneAudio);
            None
        }
        AudioRecord::Validated { asset } => {
            require(
                asset.path == MICROPHONE && doc.microphone.is_some() && !legacy_mic_failed,
                "audio.microphone.path",
                "Validated CPAL microphone must match the nested mic.wav reference",
            )?;
            asset.inventory.validate("audio.microphone.inventory")?;
            doc.warnings.push(ProjectWarning::MicrophoneTimingUnknown);
            Some(project::AudioTrack {
                path: asset.path.clone(),
                sample_rate: asset.inventory.sample_rate,
                channels: asset.inventory.channels,
                frame_count: asset.inventory.frame_count,
                timing: project::AudioTrackTiming::Unknown {},
            })
        }
    };
    match &audio.system {
        AudioRecord::Disabled {} => {}
        AudioRecord::Unavailable { .. } => doc.warnings.push(ProjectWarning::InvalidSystemAudio),
        AudioRecord::Validated { asset } => {
            require(asset.path == SYSTEM && asset.evidence_path == EVIDENCE, "audio.system.path", "System assets require their fixed session paths")?;
            let inventory = &asset.inventory;
            inventory.validate("audio.system.inventory")?;
            require(
                inventory.sample_rate == 48_000 && inventory.channels == 2
                    && inventory.encoding == WavEncoding::IeeeFloat && inventory.bits_per_sample == 32
                    && inventory.file_bytes == SYSTEM_WAV_HEADER_BYTES + inventory.frame_count * 8
                    && inventory.file_bytes <= u64::from(u32::MAX),
                "audio.system.inventory",
                "Linux system audio requires its exact stereo 48 kHz float RIFF inventory",
            )?;
            let origin = &asset.origin;
            require(
                valid_correlation_id(&origin.selection_epoch) && valid_correlation_id(&origin.admission_id)
                    && origin.parent_send_ns <= origin.child_commit_before_ns
                    && origin.child_commit_before_ns <= origin.child_commit_after_ns,
                "audio.system.origin",
                "Expected matching selection and admission IDs with ordered Commit observations",
            )?;
            doc.warnings.push(ProjectWarning::SystemTimingUnknown);
        }
    }
    doc.schema_version = 5;
    doc.microphone = None;
    doc.audio_tracks = Some(project::AudioTracks { microphone, system: None });
    project::validate(&doc)?;
    Ok(doc)
}

impl CaptureEnvelopeV4 {
    /// Build the canonical snapshot from the same Linux V1 manifest the recorder already produces.
    pub fn new_finalized(video: CaptureManifestV1, audio_requested: AudioRequested, audio: AudioRecords) -> CoreResult<Self> {
        let initial_project = derive(&video, audio_requested, &audio)?;
        Ok(Self {
            schema_version: 4,
            capture_id: video.capture_id.clone(),
            state: AudioCaptureStateV4::Finalized {
                video_authority: VideoAuthority::LinuxV1 { manifest: video },
                audio_requested,
                audio,
                initial_project,
            },
        })
    }

    fn source(&self) -> (&CaptureManifestV1, AudioRequested) {
        let (video, requested) = match &self.state {
            AudioCaptureStateV4::Incomplete { video_authority, audio_requested }
            | AudioCaptureStateV4::Finalized { video_authority, audio_requested, .. } => (video_authority, audio_requested),
        };
        let VideoAuthority::LinuxV1 { manifest } = video;
        (manifest, *requested)
    }

    pub fn validate(&self, id: &str) -> CoreResult<()> {
        require(self.schema_version == 4, "schemaVersion", "Expected capture envelope version 4")?;
        require(self.capture_id == id, "captureId", "Audio capture ID must equal its containing directory ID")?;
        let (video, requested) = self.source();
        match &self.state {
            AudioCaptureStateV4::Incomplete { .. } => {
                video.validate(id)?;
                require(matches!(video.state, CaptureStateV1::Incomplete { .. }), "videoAuthority", "Incomplete audio capture requires incomplete video authority")
            }
            AudioCaptureStateV4::Finalized { audio, initial_project, .. } => {
                require(video.capture_id == id, "videoAuthority.captureId", "Nested Linux capture ID must match the V4 authority")?;
                require(
                    &derive(video, requested, audio)? == initial_project,
                    "initialProject",
                    "Saved schema-5 initial project must equal the canonical V4 snapshot",
                )
            }
        }
    }

    pub fn finalized_project(&self) -> CoreResult<&ProjectDocument> {
        match &self.state {
            AudioCaptureStateV4::Finalized { initial_project, .. } => Ok(initial_project),
            _ => Err(invalid("capture.json", "Audio capture is incomplete; retain files for recovery before opening")),
        }
    }
}

fn decode_error(error: serde_json::Error) -> crate::CoreError {
    let mut message = format!("Invalid V4 audio capture; retain files and correct capture.json: {error}");
    store::truncate_text(&mut message, 1024);
    invalid("capture.json", message)
}

pub fn decode(bytes: &[u8], id: &str) -> CoreResult<CaptureEnvelopeV4> {
    require(bytes.len() as u64 <= MAX_JSON_BYTES, "capture.json", "Native capture JSON exceeds 8 MiB")?;
    let UniqueJson(value) = serde_json::from_slice(bytes).map_err(decode_error)?;
    decode_value(value, id)
}

pub(super) fn decode_value(value: Value, id: &str) -> CoreResult<CaptureEnvelopeV4> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Wire { schema_version: u32, capture_id: String, state: AudioCaptureStateV4 }
    named_shapes(&value, &mut Vec::new())?;
    if let Some(project) = value.pointer("/state/initialProject") {
        for path in ["/video/cursor", "/cursorSettings/style", "/frameStyle/canvas", "/frameStyle/background/type"] {
            require(project.pointer(path).is_some_and(Value::is_string), "initialProject", "Snapshot literal fields require JSON strings")?;
        }
        require(project.get("warnings").and_then(Value::as_array).is_some_and(|warnings| warnings.iter().all(Value::is_string)), "initialProject.warnings", "Snapshot warnings require JSON strings")?;
        require(project.get("zooms").and_then(Value::as_array).is_some_and(|zooms| zooms.iter().all(|zoom| zoom["mode"].is_string())), "initialProject.zooms", "Snapshot zoom mode requires a JSON string")?;
    }
    let wire: Wire = serde_json::from_value(value).map_err(decode_error)?;
    let envelope = CaptureEnvelopeV4 { schema_version: wire.schema_version, capture_id: wire.capture_id, state: wire.state };
    envelope.validate(id)?;
    Ok(envelope)
}

fn named_shapes<'a>(value: &'a Value, path: &mut Vec<&'a str>) -> CoreResult<()> {
    match value {
        Value::Array(items) => {
            require(
                matches!(path.as_slice(), ["state", "initialProject", "clips" | "zooms" | "clicks" | "warnings"]
                    | ["state", "videoAuthority", "manifest", "state", "initialProject", "clips" | "zooms" | "clicks" | "warnings"]),
                "capture.json",
                "V4 named objects require JSON objects, not positional arrays",
            )?;
            path.push("*");
            for item in items {
                named_shapes(item, path)?;
            }
            path.pop();
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                path.push(key);
                named_shapes(value, path)?;
                path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn read_marker(dir: &Path) -> CoreResult<bool> {
    let Some(path) = asset(dir, MARKER)? else { return Ok(false) };
    let mut bytes = Vec::new();
    File::open(path).and_then(|f| f.take(MARKER_BYTES.len() as u64 + 1).read_to_end(&mut bytes)).map_err(io_error)?;
    require(bytes == MARKER_BYTES, MARKER, "V4 capture intent is incomplete or invalid; retain files for recovery")?;
    Ok(true)
}

fn wav_file(path: &Path, inventory: &WavInventory, full_scan: bool) -> Option<File> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() != inventory.file_bytes {
        return None;
    }
    let limit = if full_scan { inventory.file_bytes } else { SYSTEM_WAV_HEADER_BYTES };
    let mut reader = hound::WavReader::new(BufReader::new((&file).take(limit))).ok()?;
    let spec = reader.spec();
    let format = match inventory.encoding {
        WavEncoding::PcmInteger => hound::SampleFormat::Int,
        WavEncoding::IeeeFloat => hound::SampleFormat::Float,
    };
    if spec.sample_rate != inventory.sample_rate || spec.channels != inventory.channels
        || spec.bits_per_sample != inventory.bits_per_sample || spec.sample_format != format
        || u64::from(reader.len()) != inventory.frame_count * u64::from(inventory.channels)
    {
        return None;
    }
    if full_scan {
        let usable = match format {
            hound::SampleFormat::Int => reader.samples::<i32>().all(|sample| sample.is_ok()),
            hound::SampleFormat::Float => reader.samples::<f32>().all(|sample| sample.is_ok_and(f32::is_finite)),
        };
        if !usable {
            return None;
        }
    }
    drop(reader);
    Some(file)
}

// These bounds are the Linux product producer's CommitGate lifetime and stereo packet limit.
const SYSTEM_MAX_LIFETIME_NS: u64 = 86_400_000_000_000;
const SYSTEM_MAX_PACKET_FRAMES: u64 = 8192;

fn evidence_selection_matches(value: &Value, epoch: &str) -> bool {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Port<'a> {
        id: u32,
        serial: u64,
        channel: &'a str,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Selection<'a> {
        socket_path: &'a str,
        #[serde(rename = "peer_uid")]
        _peer_uid: u32,
        peer_pid: i32,
        #[serde(rename = "cookie")]
        _cookie: u32,
        node_id: u32,
        node_serial: u64,
        #[serde(borrow)]
        ports: [Port<'a>; 2],
        selection_epoch: &'a str,
    }

    if !value.is_object() || !value["ports"].as_array().is_some_and(|ports| ports.iter().all(Value::is_object)) {
        return false;
    }
    let Ok(selection) = Selection::deserialize(value) else { return false };
    let [left, right] = selection.ports;
    selection.selection_epoch == epoch
        // The evidence records a Linux path, even when this project is opened elsewhere.
        && selection.socket_path.starts_with('/')
        && selection.socket_path.len() <= 512
        && !selection.socket_path.contains('\0')
        && selection.peer_pid > 0
        && selection.node_id != u32::MAX
        && selection.node_serial > 0
        && left.channel == "FL" && right.channel == "FR"
        && left.id != right.id && left.serial != right.serial
        && [left, right].iter().all(|port| {
            port.id != u32::MAX && port.id != selection.node_id
                && port.serial > 0 && port.serial != selection.node_serial
        })
}

struct PacketFacts {
    start: u64,
    end: u64,
    callback_before: u64,
    callback_after: u64,
}

fn decimal_clock(value: &Value) -> Option<u64> {
    let text = value.as_str()?;
    if text.len() > 20 || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    text.parse().ok()
}

fn packet_facts(row: &Value, ordinal: u64, commit_after: u64) -> Option<PacketFacts> {
    if row["kind"] != "monitor" || row["generation"].as_u64() != Some(0)
        || row["packet"].as_u64() != Some(ordinal)
    {
        return None;
    }
    let start = row["sourceStartFrame"].as_u64()?;
    let count = row["frameCount"].as_u64()?;
    let end = start.checked_add(count)?;
    let callback_before = decimal_clock(&row["callbackBeforeNs"])?;
    let callback_after = decimal_clock(&row["callbackAfterNs"])?;
    // Commit and callbacks are observed on the producer's main loop.
    if !(1..=SYSTEM_MAX_PACKET_FRAMES).contains(&count)
        || callback_before < commit_after || callback_after < callback_before
        || callback_after - commit_after >= SYSTEM_MAX_LIFETIME_NS
    {
        return None;
    }
    Some(PacketFacts { start, end, callback_before, callback_after })
}

fn validate_evidence_packets(evidence: &Value, system: &SystemAsset) -> Option<()> {
    let frames = system.inventory.frame_count;
    let packets = evidence["packets"].as_u64()?;
    if !(1..=frames).contains(&packets) {
        return None;
    }
    let first_row = &evidence["first"];
    let latest_row = &evidence["latest"];
    let commit_after = system.origin.child_commit_after_ns;
    let first = packet_facts(first_row, 0, commit_after)?;
    let latest = packet_facts(latest_row, packets - 1, commit_after)?;
    if first.start != 0 || latest.end != frames {
        return None;
    }
    if packets == 1 {
        return (first_row == latest_row).then_some(());
    }
    let interior_frames = latest.start.checked_sub(first.end)?;
    let interior_packets = packets - 2;
    (first.callback_after <= latest.callback_before
        && (interior_packets..=interior_packets * SYSTEM_MAX_PACKET_FRAMES).contains(&interior_frames))
        .then_some(())
}

fn evidence_file(path: &Path, system: &SystemAsset) -> Option<File> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    (&file).take(MAX_EVIDENCE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_EVIDENCE_BYTES {
        return None;
    }
    let UniqueJson(value) = serde_json::from_slice(&bytes).ok()?;
    let origin = &value["origin"];
    let warmup_packets = origin["warmupPackets"].as_u64()?;
    let warmup_frames = origin["warmupFrames"].as_u64()?;
    if value["timing"] != "unknown" || value["sampleVideoRelation"] != "unknown" || value["domain"].as_u64() != Some(0)
        || value["writtenFrames"].as_u64() != Some(system.inventory.frame_count)
        || !matches!(value["terminalReason"].as_str(), Some("normal" | "stopped"))
        || !evidence_selection_matches(&value["selection"], &system.origin.selection_epoch)
        || warmup_packets > warmup_frames || (warmup_packets == 0) != (warmup_frames == 0)
        || origin["timing"] != "unknown" || origin["sampleVideoRelation"] != "unknown"
        || origin["policy"] != "discardWarmupUntilCommit"
        || origin["admission"]["selection_epoch"] != system.origin.selection_epoch
        || origin["admission"]["admission_id"] != system.origin.admission_id
        || origin["parentSendNs"].as_u64() != Some(system.origin.parent_send_ns)
        || origin["childCommitObservationNs"] != serde_json::json!([system.origin.child_commit_before_ns, system.origin.child_commit_after_ns])
        || origin["firstRetainedPacket"] != value["first"]
    {
        return None;
    }
    validate_evidence_packets(&value, system)?;
    Some(file)
}

fn publication_dir(dir: &Path) -> CoreResult<&str> {
    if dir.canonicalize().map_err(io_error)? != dir || !fs::symlink_metadata(dir).map_err(io_error)?.is_dir() {
        return Err(path_error("Capture publisher requires a real session directory"));
    }
    dir.file_name().and_then(|name| name.to_str()).ok_or_else(|| path_error("Capture directory needs a UTF-8 ID"))
}

struct CaptureBytes(Vec<u8>);
impl Write for CaptureBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_JSON_BYTES as usize - self.0.len() {
            return Err(io::Error::other("Native capture JSON exceeds 8 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

fn encode(envelope: &CaptureEnvelopeV4) -> CoreResult<Vec<u8>> {
    let mut bytes = CaptureBytes(Vec::new());
    serde_json::to_writer(&mut bytes, envelope).map_err(decode_error)?;
    Ok(bytes.0)
}

fn publish_bytes(dir: &Path, bytes: &[u8]) -> CoreResult<()> {
    store::replace_bytes_with(dir, "capture.json", bytes, |_| Ok(()), |_| Ok(()))?;
    store::sync_directory(dir).map_err(|error| crate::CoreError {
        code: crate::ErrorCode::Io,
        message: format!("V4 capture authority was committed, but directory synchronization failed; durability is unconfirmed. Reload capture.json before retrying: {error}"),
        field: Some("capture.json".into()),
    })
}

pub fn publish_incomplete(dir: &Path, envelope: &CaptureEnvelopeV4) -> CoreResult<()> {
    let id = publication_dir(dir)?;
    envelope.validate(id)?;
    require(matches!(envelope.state, AudioCaptureStateV4::Incomplete { .. }), "capture.json", "Arming requires an incomplete V4 authority")?;
    let bytes = encode(envelope)?;
    let parent = dir.parent().ok_or_else(|| path_error("Capture directory needs a parent"))?;
    let store = store::ProjectStore::new(parent)?;
    let _lock = store.lock(id, "document")?;
    require(
        !read_marker(dir)? && !super::windows::read_marker(dir)? && !super::windows_observations::read_marker(dir)?
            && asset(dir, "capture.json")?.is_none() && asset(dir, "project.json")?.is_none(),
        "capture.json",
        "V4 intent requires a fresh session; retain existing authority for recovery",
    )?;
    let mut marker = OpenOptions::new().write(true).create_new(true).open(dir.join(MARKER)).map_err(io_error)?;
    marker.write_all(MARKER_BYTES).and_then(|_| marker.sync_all()).map_err(io_error)?;
    store::sync_directory(dir).and_then(|_| store::sync_directory(parent)).map_err(io_error)?;
    publish_bytes(dir, &bytes)
}

/// The recorder must establish clean owned completion and file identity before passing validated candidates.
pub fn publish_finalized_assets(
    dir: &Path,
    envelope: &CaptureEnvelopeV4,
    evidence: &ffmpeg::ProbedVideoEvidence,
) -> CoreResult<CaptureEnvelopeV4> {
    let id = publication_dir(dir)?;
    envelope.validate(id)?;
    let AudioCaptureStateV4::Finalized { audio, .. } = &envelope.state else {
        return Err(invalid("capture.json", "Finalization requires a finalized V4 candidate"));
    };
    let (video, requested) = envelope.source();
    encode(envelope)?; // Bound the duplicate snapshots before touching the referenced assets.
    let parent = dir.parent().ok_or_else(|| path_error("Capture directory needs a parent"))?;
    let store = store::ProjectStore::new(parent)?;
    let _lock = store.lock(id, "document")?;
    let Some(CaptureAuthority::LinuxAudioV4(previous)) = super::read_authority(dir, id)? else {
        return Err(invalid("capture.json", "Finalization requires a matching incomplete V4 intent"));
    };
    let (previous_video, previous_requested) = previous.source();
    require(
        matches!(previous.state, AudioCaptureStateV4::Incomplete { .. })
            && previous_requested == requested && previous_video.source == video.source,
        "capture.json",
        "Finalized capture authority is immutable and its initial source and requests must match",
    )?;
    require(asset(dir, "project.json")?.is_none(), "project.json", "Unexpected project beside incomplete V4 authority; retain files for recovery")?;
    let (doc, inventory) = super::validate_required_assets(dir, video, evidence)?;
    let mut audio = audio.clone();
    project_optional(dir, &mut audio)?;
    let committed = CaptureEnvelopeV4::new_finalized(video.clone(), requested, audio)?;
    let bytes = encode(&committed)?;
    super::sync_asset_inventory(dir, &doc, inventory, |path, directory| {
        if directory { store::sync_directory(path) } else { File::open(path)?.sync_all() }
    })?;
    publish_bytes(dir, &bytes)?;
    Ok(committed)
}

fn project_optional(dir: &Path, audio: &mut AudioRecords) -> CoreResult<()> {
    if let AudioRecord::Validated { asset: mic } = &audio.microphone {
        let path = asset(dir, MICROPHONE)?;
        let usable = path.as_deref().and_then(|p| wav_file(p, &mic.inventory, true)).is_some_and(|f| f.sync_all().is_ok());
        if !usable {
            audio.microphone = AudioRecord::Unavailable { reason: "Microphone WAV validation or synchronization failed".into() };
        }
    }
    if let AudioRecord::Validated { asset: system } = &audio.system {
        let path = asset(dir, SYSTEM)?;
        let evidence = asset(dir, EVIDENCE)?;
        let usable = path.as_deref().and_then(|p| wav_file(p, &system.inventory, true))
            .zip(evidence.as_deref().and_then(|p| evidence_file(p, system)))
            .is_some_and(|(wav, evidence)| {
                wav.sync_all().is_ok() && evidence.sync_all().is_ok()
                    && store::sync_directory(&dir.join("audio/system")).is_ok()
                    && store::sync_directory(&dir.join("audio")).is_ok()
            });
        if !usable {
            audio.system = AudioRecord::Unavailable { reason: "System WAV or Commit evidence validation or synchronization failed".into() };
        }
    }
    Ok(())
}

pub(super) fn import(dir: &Path, envelope: CaptureEnvelopeV4, ffmpeg_path: &Path) -> CoreResult<ProjectDocument> {
    let doc = envelope.finalized_project()?;
    check_paths(dir, &envelope)?;
    let path = asset(dir, "screen.mp4")?.ok_or_else(|| invalid("video", "Native screen.mp4 is missing"))?;
    let evidence = ffmpeg::probe_video_evidence(ffmpeg_path, &path)?;
    super::validate_required_assets(dir, envelope.source().0, &evidence)?;
    Ok(doc.clone())
}

fn check_paths(dir: &Path, envelope: &CaptureEnvelopeV4) -> CoreResult<()> {
    let AudioCaptureStateV4::Finalized { audio, initial_project, .. } = &envelope.state else {
        return Err(invalid("capture.json", "Audio capture is incomplete; retain files for recovery before opening"));
    };
    asset(dir, &initial_project.video.path)?;
    if let Some(cursor) = &initial_project.cursor {
        asset(dir, &cursor.index_path)?;
    }
    if matches!(audio.microphone, AudioRecord::Validated { .. }) {
        asset(dir, MICROPHONE)?;
    }
    if matches!(audio.system, AudioRecord::Validated { .. }) {
        asset(dir, SYSTEM)?;
        asset(dir, EVIDENCE)?;
    }
    Ok(())
}

// Recognize the discriminator even if the remaining V4 JSON is malformed or oversized.
fn identifies_v4(path: &Path) -> bool {
    struct Version<'a>(&'a mut bool);
    impl<'de> serde::de::Visitor<'de> for Version<'_> {
        type Value = ();

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a capture manifest object")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut fields: A) -> Result<(), A::Error> {
            while let Some(key) = fields.next_key::<String>()? {
                if key == "schemaVersion" {
                    if fields.next_value::<u64>()? == 4 {
                        *self.0 = true;
                        break;
                    }
                } else {
                    fields.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }

    let Ok(file) = File::open(path) else { return false };
    let mut found = false;
    let mut decoder = serde_json::Deserializer::from_reader(BufReader::new(file).take(MAX_JSON_BYTES));
    let _ = serde::de::Deserializer::deserialize_map(&mut decoder, Version(&mut found));
    found
}

pub(crate) fn join_persisted(dir: &Path, doc: &ProjectDocument) -> CoreResult<Option<CaptureEnvelopeV4>> {
    if !read_marker(dir)? {
        // Preserve old persisted-project behavior even when unrelated legacy authority is damaged.
        if let Ok(Some(path)) = asset(dir, "capture.json") {
            require(!identifies_v4(&path), MARKER, "V4 intent marker is missing; retain files for recovery")?;
        }
        return Ok(None);
    }
    let Some(CaptureAuthority::LinuxAudioV4(envelope)) = super::read_authority(dir, &doc.project_id)? else {
        return Err(invalid("capture.json", "V4 intent does not match its capture authority; retain files for recovery"));
    };
    let AudioCaptureStateV4::Finalized { initial_project: initial, audio, .. } = &envelope.state else {
        return Err(invalid("capture.json", "Audio capture is incomplete; retain files for recovery before opening"));
    };
    require(
        doc.schema_version == initial.schema_version && doc.project_id == initial.project_id
            && doc.video == initial.video && doc.cursor == initial.cursor && doc.audio_tracks == initial.audio_tracks
            && doc.camera == initial.camera && doc.clicks == initial.clicks
            && initial.warnings.iter().all(|warning| doc.warnings.contains(warning))
            && doc.warnings.iter().all(|warning| initial.warnings.contains(warning) || match warning {
                ProjectWarning::InvalidMicrophoneAudio => matches!(audio.microphone, AudioRecord::Validated { .. }),
                ProjectWarning::InvalidSystemAudio => matches!(audio.system, AudioRecord::Validated { .. }),
                _ => false,
            }),
        "project.json",
        "Saved project source metadata conflicts with V4 capture authority; retain files for recovery",
    )?;
    check_paths(dir, &envelope)?;
    Ok(Some(envelope))
}

pub(crate) fn project_render_audio(dir: &Path, envelope: &CaptureEnvelopeV4, doc: &mut ProjectDocument) -> CoreResult<()> {
    let AudioCaptureStateV4::Finalized { audio, .. } = &envelope.state else {
        return Err(invalid("capture.json", "Cannot render an incomplete V4 capture"));
    };
    let tracks = doc.audio_tracks.as_mut().ok_or_else(|| invalid("audioTracks", "V4 render requires schema-5 audio tracks"))?;
    if let AudioRecord::Validated { asset: mic } = &audio.microphone {
        if asset(dir, MICROPHONE)?.as_deref().and_then(|p| wav_file(p, &mic.inventory, true)).is_none() {
            tracks.microphone = None;
            add_warning(&mut doc.warnings, ProjectWarning::InvalidMicrophoneAudio);
        }
    }
    if let AudioRecord::Validated { asset: system } = &audio.system {
        let path = asset(dir, SYSTEM)?;
        let evidence = asset(dir, EVIDENCE)?;
        // Withheld audio is never played. Check inventory without reading all samples on every preview.
        if path.as_deref().and_then(|p| wav_file(p, &system.inventory, false)).is_none()
            || evidence.as_deref().and_then(|p| p.metadata().ok()).is_none_or(|m| !(1..=MAX_EVIDENCE_BYTES).contains(&m.len()))
        {
            add_warning(&mut doc.warnings, ProjectWarning::InvalidSystemAudio);
        }
    }
    Ok(())
}

fn add_warning(warnings: &mut Vec<ProjectWarning>, warning: ProjectWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}
