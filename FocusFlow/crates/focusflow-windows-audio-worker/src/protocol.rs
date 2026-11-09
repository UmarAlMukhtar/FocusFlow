use crate::{format::{Encoding, Format}, Result};
use serde::{Deserialize, Serialize};
use std::{fmt, io::Read, path::PathBuf, time::{Duration, Instant}};

pub const FRAME_BYTES: usize = 4096;
pub const STARTUP_MS: u64 = 8000;
pub const STOP_MS: u64 = 3000;
pub const STORAGE_MS: u64 = 120_000;
pub const FORCE_MS: u64 = 2000;
pub const RECOVERY_MS: u64 = 5000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    Oversize, Truncated, Io(String), Shape(String), Version, Nonce, Kind,
    Selection, DuplicateKind, OutputDirectory, Reason, Format, Inventory,
    Clock, Deadline, Order,
}
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversize => f.write_str("Worker frame exceeds 4096 bytes including newline"),
            Self::Truncated => f.write_str("Worker frame ended before newline"),
            Self::Io(s) | Self::Shape(s) => write!(f, "Invalid worker frame: {s}"),
            Self::Selection => f.write_str("Enable the track and select its exact endpoint ID (1..2048 UTF-8 bytes)"),
            _ => write!(f, "Invalid worker protocol: {self:?}"),
        }
    }
}
impl std::error::Error for ProtocolError {}
pub type ProtocolResult<T> = std::result::Result<T, ProtocolError>;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum TrackKind { Microphone, System }
impl<'de> Deserialize<'de> for TrackKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match String::deserialize(deserializer)?.as_str() {
            "Microphone" => Ok(Self::Microphone),
            "System" => Ok(Self::System),
            other => Err(serde::de::Error::unknown_variant(other, &["Microphone", "System"])),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrackRequest {
    pub kind: TrackKind,
    pub endpoint_id: String,
    pub explicitly_enabled: bool,
    pub output_dir: PathBuf,
}
impl TrackRequest {
    pub fn validate(&self) -> ProtocolResult<()> {
        if !self.explicitly_enabled || self.endpoint_id.is_empty()
            || self.endpoint_id.len() > 2048 || self.endpoint_id.contains('\0') {
            return Err(ProtocolError::Selection);
        }
        if !self.output_dir.is_absolute()
            || self.output_dir.to_str().is_none_or(|s| s.contains('\0')) {
            return Err(ProtocolError::OutputDirectory);
        }
        Ok(())
    }
}

pub fn validate_requests(requests: &[TrackRequest]) -> ProtocolResult<()> {
    if requests.len() > 2 { return Err(ProtocolError::DuplicateKind); }
    for (i, request) in requests.iter().enumerate() {
        request.validate()?;
        if requests[..i].iter().any(|other| other.kind == request.kind) {
            return Err(ProtocolError::DuplicateKind);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InitV1 { pub version: u32, pub nonce: String, pub request: TrackRequest }

/// Pure pipe validation only; native Job/control/owned-directory checks still precede WorkerPermit.
#[derive(Debug, Clone)]
pub struct ValidatedInit { init: InitV1 }
impl ValidatedInit {
    pub fn request(&self) -> &TrackRequest { &self.init.request }
    pub fn nonce(&self) -> &str { &self.init.nonce }
}
fn validate_nonce(nonce: &str) -> ProtocolResult<()> {
    if nonce.len() != 36 || !nonce.bytes().enumerate().all(|(i, b)| {
        if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() }
    }) { return Err(ProtocolError::Nonce); }
    Ok(())
}
fn validate_init(init: InitV1, expected_nonce: &str) -> ProtocolResult<ValidatedInit> {
    if init.version != 1 { return Err(ProtocolError::Version); }
    validate_nonce(&init.nonce)?;
    if init.nonce != expected_nonce { return Err(ProtocolError::Nonce); }
    init.request.validate()?;
    Ok(ValidatedInit { init })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClockStamp { pub qpc: u64, pub frequency: u64 }
impl ClockStamp {
    fn elapsed(self, observed_after_read: Self) -> ProtocolResult<Duration> {
        if self.frequency == 0 || self.frequency != observed_after_read.frequency
            || self.qpc > observed_after_read.qpc { return Err(ProtocolError::Clock); }
        let delta = observed_after_read.qpc - self.qpc;
        let nanos = u128::from(delta) * 1_000_000_000 / u128::from(self.frequency);
        let secs = u64::try_from(nanos / 1_000_000_000).map_err(|_| ProtocolError::Clock)?;
        Ok(Duration::new(secs, (nanos % 1_000_000_000) as u32))
    }
}

/// Wire inventory is untrusted until writer/parent reconciliation after exact observed exit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WavInventory {
    pub frames: u64, pub rate: u32, pub channels: u16, pub bits: u16,
    pub encoding: Encoding, pub file_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalSummary {
    pub accepted_frames: u64,
    pub written_complete_frames: u64,
    #[serde(deserialize_with = "required_option")]
    pub synchronized_frames: Option<u64>,
    #[serde(deserialize_with = "required_option")]
    pub usable: Option<WavInventory>,
    pub retained_partial: bool,
    pub cause: String,
    pub evidence_present: bool,
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(deserializer: D) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
impl TerminalSummary {
    fn validate(&self) -> ProtocolResult<()> {
        validate_reason(&self.cause)?;
        if self.written_complete_frames > self.accepted_frames
            || self.synchronized_frames.is_some_and(|n| n > self.written_complete_frames) {
            return Err(ProtocolError::Inventory);
        }
        if let Some(w) = &self.usable {
            let format = Format { rate: w.rate, channels: w.channels, bits: w.bits,
                valid_bits: w.bits, align: w.channels.checked_mul(w.bits / 8).ok_or(ProtocolError::Inventory)?,
                encoding: w.encoding };
            format.validate().map_err(|_| ProtocolError::Inventory)?;
            if w.frames == 0 || w.frames != self.accepted_frames
                || w.frames != self.written_complete_frames
                || self.synchronized_frames != Some(w.frames) || w.file_bytes <= 44 {
                return Err(ProtocolError::Inventory);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Event {
    Ready { format: Format },
    RecoveryStarted { episode: u32, first_failure: ClockStamp },
    Recovered { episode: u32, generation: u32 },
    StopAck { observed: ClockStamp },
    Ending { cause: String, observed: ClockStamp },
    CaptureReleased { at: ClockStamp },
    Terminal { outcome: TerminalSummary },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventV1 { pub version: u32, pub nonce: String, pub kind: TrackKind, pub event: Event }
fn validate_reason(reason: &str) -> ProtocolResult<()> {
    if reason.is_empty() || reason.len() > 1024 || reason.contains('\0') {
        return Err(ProtocolError::Reason);
    }
    Ok(())
}
impl EventV1 {
    fn validate(&self, nonce: &str, kind: TrackKind) -> ProtocolResult<()> {
        if self.version != 1 { return Err(ProtocolError::Version); }
        validate_nonce(&self.nonce)?;
        if self.nonce != nonce { return Err(ProtocolError::Nonce); }
        if self.kind != kind { return Err(ProtocolError::Kind); }
        match &self.event {
            Event::Ready { format } => { format.validate().map_err(|_| ProtocolError::Format)?; }
            Event::Ending { cause, .. } => validate_reason(cause)?,
            Event::Terminal { outcome } => outcome.validate()?,
            _ => {}
        }
        Ok(())
    }
}

fn decode<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> ProtocolResult<T> {
    if frame.len() > FRAME_BYTES { return Err(ProtocolError::Oversize); }
    if frame.last() != Some(&b'\n') { return Err(ProtocolError::Truncated); }
    if frame[..frame.len()-1].contains(&b'\n') { return Err(ProtocolError::Shape("Multiple frames".into())); }
    serde_json::from_slice(&frame[..frame.len()-1]).map_err(|e| ProtocolError::Shape(e.to_string()))
}
fn encode(value: &impl Serialize) -> ProtocolResult<Vec<u8>> {
    let mut frame = serde_json::to_vec(value).map_err(|e| ProtocolError::Shape(e.to_string()))?;
    frame.push(b'\n');
    if frame.len() > FRAME_BYTES { return Err(ProtocolError::Oversize); }
    Ok(frame)
}
fn read_frame(mut reader: impl Read) -> ProtocolResult<Vec<u8>> {
    let mut frame = Vec::with_capacity(FRAME_BYTES);
    for _ in 0..FRAME_BYTES {
        let mut byte = [0];
        match reader.read_exact(&mut byte) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(ProtocolError::Truncated),
            Err(e) => return Err(ProtocolError::Io(e.to_string())),
        }
        frame.push(byte[0]);
        if byte[0] == b'\n' { return Ok(frame); }
    }
    Err(ProtocolError::Oversize)
}
pub fn encode_init(init: &InitV1) -> ProtocolResult<Vec<u8>> {
    validate_init(init.clone(), &init.nonce)?;
    encode(init)
}
pub fn decode_init_frame(frame: &[u8], expected_nonce: &str) -> ProtocolResult<ValidatedInit> {
    validate_init(decode(frame)?, expected_nonce)
}
pub fn read_init(reader: impl Read, expected_nonce: &str) -> ProtocolResult<ValidatedInit> {
    decode_init_frame(&read_frame(reader)?, expected_nonce)
}
pub fn encode_event(event: &EventV1) -> ProtocolResult<Vec<u8>> {
    event.validate(&event.nonce, event.kind)?;
    encode(event)
}
pub fn decode_event_frame(frame: &[u8], nonce: &str, kind: TrackKind) -> ProtocolResult<EventV1> {
    let event: EventV1 = decode(frame)?;
    event.validate(nonce, kind)?;
    Ok(event)
}
pub fn read_event(reader: impl Read, nonce: &str, kind: TrackKind) -> ProtocolResult<EventV1> {
    decode_event_frame(&read_frame(reader)?, nonce, kind)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State { Gated, Starting, Running, Recovering, Stopping, StorageFinalizing, AwaitingExit, ForceExit, CleanupPending, Exited }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick { None, Force, RetainPending }

pub struct Owner {
    pub state: State,
    nonce: String,
    kind: TrackKind,
    startup_deadline: Instant,
    native_deadline: Option<Instant>,
    recovery_deadline: Option<Instant>,
    storage_deadline: Option<Instant>,
    force_deadline: Option<Instant>,
    stop_signaled: bool,
    ending: Option<ClockStamp>,
    episode: u32,
    generation: u32,
    terminal: Option<TerminalSummary>,
    failed: bool,
}
impl Owner {
    pub fn new(startup_deadline: Instant, nonce: String, kind: TrackKind) -> ProtocolResult<Self> {
        validate_nonce(&nonce)?;
        Ok(Self { state: State::Gated, nonce, kind, startup_deadline,
            native_deadline: None, recovery_deadline: None, storage_deadline: None,
            force_deadline: None, stop_signaled: false, ending: None, episode: 0,
            generation: 0, terminal: None, failed: false })
    }
    pub fn assigned(&mut self, now: Instant) -> ProtocolResult<()> {
        if self.state != State::Gated { return Err(ProtocolError::Order); }
        self.check_deadline(now)?;
        self.state = State::Starting;
        Ok(())
    }
    fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Gated | State::Starting => Some(self.startup_deadline),
            State::Recovering => self.recovery_deadline,
            State::Stopping => self.native_deadline,
            State::StorageFinalizing | State::AwaitingExit => self.storage_deadline,
            State::ForceExit => self.force_deadline,
            _ => None,
        }
    }
    fn check_deadline(&self, now: Instant) -> ProtocolResult<()> {
        if self.deadline().is_some_and(|deadline| now >= deadline) { return Err(ProtocolError::Deadline); }
        Ok(())
    }
    fn begin_stop(&mut self, now: Instant) -> ProtocolResult<()> {
        let mut deadline = now.checked_add(Duration::from_millis(STOP_MS)).ok_or(ProtocolError::Clock)?;
        if matches!(self.state, State::Gated | State::Starting) { deadline = deadline.min(self.startup_deadline); }
        if let Some(recovery) = self.recovery_deadline { deadline = deadline.min(recovery); }
        self.native_deadline.get_or_insert(deadline);
        self.state = State::Stopping;
        Ok(())
    }
    /// Call only after attempting the direct event signal, even when that attempt fails.
    pub fn stop(&mut self, now: Instant) -> ProtocolResult<()> {
        self.stop_signaled = true;
        match self.state {
            State::Gated | State::Starting | State::Running | State::Recovering => self.begin_stop(now),
            _ => Ok(()),
        }
    }
    /// Latches unavailable audio. The caller signals Stop; storage/EOF cancellation forces immediately.
    pub fn protocol_failure(&mut self, now: Instant) -> ProtocolResult<Tick> {
        self.failed = true;
        match self.state {
            State::StorageFinalizing | State::AwaitingExit => self.cancel_storage(now),
            State::Gated | State::Starting | State::Running | State::Recovering => {
                self.begin_stop(now)?; Ok(Tick::None)
            }
            _ => Ok(Tick::None),
        }
    }
    pub fn cancel_storage(&mut self, now: Instant) -> ProtocolResult<Tick> {
        self.failed = true;
        self.force(now)
    }
    fn force(&mut self, now: Instant) -> ProtocolResult<Tick> {
        if matches!(self.state, State::Exited | State::CleanupPending | State::ForceExit) { return Ok(Tick::None); }
        self.force_deadline = Some(now.checked_add(Duration::from_millis(FORCE_MS)).ok_or(ProtocolError::Clock)?);
        self.state = State::ForceExit;
        Ok(Tick::Force)
    }
    /// Nonblocking, one-shot containment effects; exact process observation is supplied by the owner.
    pub fn tick(&mut self, now: Instant, exited: bool) -> ProtocolResult<Tick> {
        if exited {
            if self.deadline().is_some_and(|deadline| now >= deadline) { self.failed = true; }
            self.state = State::Exited;
            return Ok(Tick::None);
        }
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            if self.state == State::ForceExit {
                self.state = State::CleanupPending;
                return Ok(Tick::RetainPending);
            }
            self.failed = true;
            return self.force(now);
        }
        Ok(Tick::None)
    }
    /// observed_after_read must be a fresh shared-QPC read after this complete frame was read.
    pub fn message(&mut self, now: Instant, observed_after_read: ClockStamp, message: &EventV1) -> ProtocolResult<()> {
        message.validate(&self.nonce, self.kind)?;
        self.check_deadline(now)?;
        match &message.event {
            Event::Ready { .. } if self.state == State::Starting => { self.state = State::Running; }
            Event::RecoveryStarted { episode, first_failure } if self.state == State::Running
                && self.episode.checked_add(1) == Some(*episode) => {
                let remaining = Duration::from_millis(RECOVERY_MS).checked_sub(first_failure.elapsed(observed_after_read)?)
                    .filter(|d| !d.is_zero()).ok_or(ProtocolError::Deadline)?;
                self.recovery_deadline = Some(now.checked_add(remaining).ok_or(ProtocolError::Clock)?);
                self.episode = *episode;
                self.state = State::Recovering;
            }
            Event::Recovered { episode, generation } if self.state == State::Recovering
                && *episode == self.episode && self.generation.checked_add(1) == Some(*generation) => {
                self.generation = *generation;
                self.recovery_deadline = None;
                self.state = State::Running;
            }
            Event::StopAck { observed } if self.state == State::Stopping && self.stop_signaled && self.ending.is_none() => {
                observed.elapsed(observed_after_read)?;
                self.ending = Some(*observed);
            }
            Event::Ending { observed, .. } if matches!(self.state, State::Gated | State::Starting | State::Running | State::Recovering | State::Stopping)
                && self.ending.is_none() => {
                observed.elapsed(observed_after_read)?;
                self.begin_stop(now)?;
                self.ending = Some(*observed);
            }
            Event::CaptureReleased { at } if self.state == State::Stopping && self.ending.is_some() && !self.failed => {
                let ending = self.ending.ok_or(ProtocolError::Order)?;
                if at.frequency != ending.frequency || at.qpc < ending.qpc { return Err(ProtocolError::Clock); }
                let remaining = Duration::from_millis(STORAGE_MS).checked_sub(at.elapsed(observed_after_read)?)
                    .filter(|d| !d.is_zero()).ok_or(ProtocolError::Deadline)?;
                self.storage_deadline = Some(now.checked_add(remaining).ok_or(ProtocolError::Clock)?);
                self.state = State::StorageFinalizing;
            }
            Event::Terminal { outcome } if self.state == State::StorageFinalizing && !self.failed => {
                self.terminal = Some(outcome.clone());
                self.state = State::AwaitingExit;
            }
            _ => return Err(ProtocolError::Order),
        }
        Ok(())
    }
    /// A summary alone never qualifies audio: parent must reconcile files and observe clean exact exit.
    pub fn terminal(&self) -> Option<&TerminalSummary> {
        if self.failed { None } else { self.terminal.as_ref() }
    }
}

pub struct Recovery { id: String, start: u64, attempts: u32 }
impl Recovery {
    pub fn new(id: &str, now: u64) -> Result<Self> {
        if id.is_empty() || id.contains('\0') { return Err("Exact endpoint ID required".into()); }
        Ok(Self { id: id.into(), start: now, attempts: 0 })
    }
    pub fn attempt(&mut self, id: &str, now: u64) -> Result<u32> {
        if id != self.id || now < self.start || now - self.start >= 5000 || self.attempts >= 2 {
            return Err("Same-endpoint recovery budget unavailable".into());
        }
        self.attempts += 1;
        Ok(self.attempts)
    }
}
