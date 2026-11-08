use crate::{format::Format, Result};
use std::sync::mpsc::SyncSender;

pub const MAX_DOMAINS: usize = 1024;
pub const QUEUE_PACKETS: usize = 8;

pub fn frame_cap(format: Format) -> Result<u64> {
    format.validate()?;
    Ok(u64::from(format.rate).checked_mul(86_400).ok_or("Source lifetime overflow")?
        .min((u64::from(u32::MAX) - 4096) / u64::from(format.align)))
}

/// Both queue admission and raw evidence count exactly these boundaries.
pub fn starts_domain(previous_generation: Option<u32>, generation: u32, segment_start: bool) -> bool {
    segment_start || previous_generation != Some(generation)
}

pub const DISCONTINUITY: u32 = 1;
pub const SILENT: u32 = 2;
pub const TIMESTAMP_ERROR: u32 = 4;

#[derive(Debug, Clone)]
pub struct Packet {
    pub generation: u32,
    pub wav_first: u64,
    pub device_first: u64,
    pub frames: u32,
    pub qpc100ns: u64,
    pub flags: u32,
    pub segment_start: bool,
    pub bytes: Vec<u8>,
}

impl Packet {
    pub fn validate(&self, format: Format, written: u64) -> Result<()> {
        if self.frames == 0 || self.flags & !7 != 0 || self.wav_first != written
            || self.bytes.len() != format.byte_len(self.frames)? {
            return Err("WAV accepted-prefix packet mismatch".into());
        }
        let end = written.checked_add(u64::from(self.frames)).ok_or("WAV frame overflow")?;
        if end > frame_cap(format)? { return Err("WAV whole-packet source budget exceeded".into()); }
        self.device_first.checked_add(u64::from(self.frames)).ok_or("Device frame overflow")?;
        format.decode(&self.bytes)?;
        Ok(())
    }

    pub fn trusted_anchor(&self) -> bool { self.flags & TIMESTAMP_ERROR == 0 }
}

/// The last accepted packet is raw tail; no exclusive-end timestamp is inferred.
pub struct Track {
    pub accepted_frames: u64,
    pub ended: Option<String>,
    pub generation: u32,
    pub format: Format,
    sender: SyncSender<Packet>,
    last: Option<(u64, u32, u64, bool)>,
    domains: usize,
    domain_generation: Option<u32>,
    reserved_generation: bool,
}

impl Track {
    pub fn new(format: Format, sender: SyncSender<Packet>) -> Result<Self> {
        Ok(Self {
            accepted_frames: 0, ended: None, generation: 0,
            format: format.validate()?, sender, last: None, domains: 0, domain_generation: None, reserved_generation: false,
        })
    }

    pub fn admit(&mut self, frames: u32, flags: u32, device: u64,
        qpc: u64, data: Option<&[u8]>) -> Result<()> {
        if let Some(e) = &self.ended { return Err(e.clone()); }
        if frames == 0 { return Ok(()); }
        let result: Result<()> = (|| {
            if flags & !7 != 0 { return Err("Unknown native packet flags".into()); }
            let n = self.format.byte_len(frames)?;
            device.checked_add(u64::from(frames)).ok_or("Device frame index overflow")?;
            let next = self.accepted_frames.checked_add(u64::from(frames))
                .ok_or("WAV frame index overflow")?;
            if next > frame_cap(self.format)? { return Err("Audio source frame/RIFF budget reached; prefix retained".into()); }
            if self.last.is_some_and(|(d, n, t, _)| {
                d.checked_add(u64::from(n)).is_none_or(|end| device < end) || qpc < t
            }) {
                return Err("Backwards native packet evidence".into());
            }
            let segment_start = self.last.is_none_or(|(d, n, t, error)| {
                d.checked_add(u64::from(n)) != Some(device) || qpc <= t || error
            }) || flags & (DISCONTINUITY | TIMESTAMP_ERROR) != 0;
            let new_domain = starts_domain(self.domain_generation, self.generation, segment_start);
            if new_domain && !self.reserved_generation && self.domains >= MAX_DOMAINS {
                return Err("Audio timing domain budget reached; prefix retained".into());
            }
            let bytes = if flags & SILENT != 0 {
                vec![0; n]
            } else {
                let b = data.ok_or("Non-silent packet has no data")?;
                if b.len() != n { return Err("Native packet byte length mismatch".into()); }
                self.format.decode(b)?;
                b.to_vec()
            };
            let p = Packet {
                generation: self.generation, wav_first: self.accepted_frames,
                device_first: device, frames, qpc100ns: qpc, flags, segment_start, bytes,
            };
            self.sender.try_send(p)
                .map_err(|_| "Audio writer queue unavailable; accepted prefix retained")?;
            self.accepted_frames = next;
            if new_domain {
                if self.reserved_generation { self.reserved_generation = false; }
                else { self.domains += 1; }
            }
            self.domain_generation = Some(self.generation);
            self.last = Some((device, frames, qpc, flags & TIMESTAMP_ERROR != 0));
            Ok(())
        })();
        if let Err(e) = &result { self.ended = Some(e.clone()); }
        result
    }

    pub fn check_recovery(&self) -> Result<()> {
        if self.ended.is_some() { return Err("Track already terminal".into()); }
        if self.domains >= MAX_DOMAINS || self.generation as usize >= MAX_DOMAINS - 1 {
            return Err("Audio timing domain/generation recovery budget reached; prefix retained".into());
        }
        Ok(())
    }

    pub fn recover(&mut self, format: Format) -> Result<()> {
        if let Err(error) = self.check_recovery() { self.ended = Some(error.clone()); return Err(error); }
        if let Err(e) = format.validate() {
            self.ended = Some(e.clone());
            return Err(e);
        }
        if format != self.format {
            let error = "Recovered endpoint changed mix format".to_string();
            self.ended = Some(error.clone());
            return Err(error);
        }
        self.generation = self.generation.checked_add(1).ok_or("Generation overflow")?;
        self.last = None;
        // Empty recovered generations still consume the finite domain/event budget.
        self.domains += 1;
        self.reserved_generation = true;
        Ok(())
    }
}
