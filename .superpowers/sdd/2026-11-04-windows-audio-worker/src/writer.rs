//! Private durable-prefix storage. Raw observations confer no project timing authority.
use crate::{format::Format, packet::{Packet, starts_domain, MAX_DOMAINS, QUEUE_PACKETS,
    SILENT, TIMESTAMP_ERROR, DISCONTINUITY}, wav::{PartialWav, create_new}, Result};
pub use crate::protocol::WavInventory;
use serde::Serialize;
use std::{fs::File, io::{self, Seek, Write}, path::{Path, PathBuf}, sync::mpsc::{self,
    Receiver, SyncSender, TryRecvError}, thread::{self, JoinHandle}};

pub trait WavSink {
    fn append(&mut self, packet: &Packet) -> Result<()>;
    fn finish(self) -> Result<WavInventory>;
}
impl<W: Write + Seek> WavSink for PartialWav<W> {
    fn append(&mut self, packet: &Packet) -> Result<()> { PartialWav::append(self, packet) }
    fn finish(self) -> Result<WavInventory> { self.finish_inventory() }
}

#[derive(Debug)]
pub struct WriterOutcome {
    pub accepted_frames: Option<u64>,
    pub written_complete_frames: u64,
    pub synchronized_frames: Option<u64>,
    pub usable: Option<WavInventory>,
    pub retained_partial: bool,
    pub evidence_present: bool,
    pub reason: String,
    validated: Option<WavInventory>,
    cancelled: bool,
}
fn bounded_reason(reason: &str) -> String {
    let mut end = reason.len().min(768);
    while !reason.is_char_boundary(end) { end -= 1; }
    reason[..end].to_string()
}
impl WriterOutcome {
    fn partial(retained_partial: bool, reason: &str) -> Self {
        Self { accepted_frames: None, written_complete_frames: 0, synchronized_frames: None,
            usable: None, retained_partial, evidence_present: false, reason: bounded_reason(reason),
            validated: None, cancelled: false }
    }
    fn notice(&mut self, reason: &str) {
        self.reason = bounded_reason(&format!("{}; {}", self.reason, bounded_reason(reason)));
    }
    /// Admission count comes from Track after its sole sender is closed and drain has completed.
    pub fn reconcile(mut self, accepted_frames: u64, producer_reason: Option<&str>) -> Self {
        self.accepted_frames = Some(accepted_frames);
        self.usable = None;
        if self.written_complete_frames == accepted_frames && !self.cancelled {
            if let Some(v) = &self.validated {
                if v.frames == accepted_frames { self.usable = Some(v.clone()); }
            }
        } else { self.notice("Accepted/written count mismatch or storage cancelled; partial retained"); }
        if let Some(reason) = producer_reason { self.notice(reason); }
        self
    }
    /// Outcome boundary only; the parent must force/observe the actual child on storage timeout.
    pub fn cancel_storage(&mut self) {
        self.cancelled = true; self.validated = None; self.usable = None;
        self.notice("Storage cancelled; partial retained and unusable");
    }
}

#[derive(Clone, Serialize)]
struct RawPacket {
    generation: u32, wav_first: u64, device_first: u64, frames: u32,
    qpc100ns: u64, flags: u32, segment_start: bool,
}
impl From<&Packet> for RawPacket {
    fn from(p: &Packet) -> Self { Self { generation: p.generation, wav_first: p.wav_first,
        device_first: p.device_first, frames: p.frames, qpc100ns: p.qpc100ns, flags: p.flags,
        segment_start: p.segment_start } }
}
#[derive(Serialize)]
struct Domain {
    first: RawPacket, latest: RawPacket, landmarks: Vec<RawPacket>, packets: u64,
}
#[derive(Serialize)]
pub struct EvidenceBuilder {
    #[serde(skip)] path: PathBuf,
    #[serde(skip)] format: Format,
    domains: Vec<Domain>,
    packets: u64, silent_packets: u64, timestamp_errors: u64, discontinuities: u64,
    raw_tail: Option<RawPacket>,
    exclusive_end: &'static str,
    first_sample: &'static str,
    physical_uncertainty: &'static str,
}
pub const MAX_EVIDENCE_BYTES: usize = 2 * 1024 * 1024;
impl EvidenceBuilder {
    pub fn new(path: &Path, format: Format) -> Result<Self> {
        Ok(Self { path: path.into(), format: format.validate()?, domains: Vec::new(), packets: 0,
            silent_packets: 0, timestamp_errors: 0, discontinuities: 0, raw_tail: None,
            exclusive_end: "Unmeasured", first_sample: "Unmeasured", physical_uncertainty: "Unmeasured" })
    }
    pub fn check_packet(&self, packet: &Packet) -> Result<()> {
        let previous = self.raw_tail.as_ref().map(|p| p.generation);
        if starts_domain(previous, packet.generation, packet.segment_start) && self.domains.len() >= MAX_DOMAINS {
            return Err("Writer timing domain budget exceeded; partial retained".into());
        }
        Ok(())
    }
    pub fn record(&mut self, packet: &Packet) -> Result<()> {
        self.check_packet(packet)?;
        let raw = RawPacket::from(packet);
        if starts_domain(self.raw_tail.as_ref().map(|p|p.generation), packet.generation, packet.segment_start) {
            self.domains.push(Domain { first: raw.clone(), latest: raw.clone(), landmarks: Vec::new(), packets: 1 });
        } else if let Some(domain) = self.domains.last_mut() {
            // Sample the previous latest on powers of two; retain at most eight interior landmarks.
            if domain.packets > 1 && domain.packets.is_power_of_two() {
                if domain.landmarks.len() == 8 { domain.landmarks.remove(0); }
                domain.landmarks.push(domain.latest.clone());
            }
            domain.latest = raw.clone(); domain.packets += 1;
        }
        self.packets += 1;
        self.silent_packets += u64::from(packet.flags & SILENT != 0);
        self.timestamp_errors += u64::from(packet.flags & TIMESTAMP_ERROR != 0);
        self.discontinuities += u64::from(packet.flags & DISCONTINUITY != 0);
        self.raw_tail = Some(raw);
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() > MAX_EVIDENCE_BYTES - self.0.len() { return Err(io::Error::other("Raw evidence exceeds 2 MiB")); }
                self.0.extend_from_slice(bytes); Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> { Ok(()) }
        }
        let mut bytes = Bounded(Vec::with_capacity(MAX_EVIDENCE_BYTES));
        serde_json::to_writer(&mut bytes, &self).map_err(|e| e.to_string())?;
        let mut file = create_new(&self.path)?;
        file.write_all(&bytes.0).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())
    }
}

/// One loop for both the file sink and operation-failure tests. Never accumulates PCM.
pub fn run_writer(rx: Receiver<Packet>, mut sink: impl WavSink, mut evidence: EvidenceBuilder) -> WriterOutcome {
    let mut out = WriterOutcome::partial(true, "Writer drained");
    let mut healthy = true;
    for packet in rx.iter() {
        let append = packet.validate(evidence.format, out.written_complete_frames)
            .and_then(|_| evidence.check_packet(&packet)).and_then(|_| sink.append(&packet));
        if let Err(error) = append {
            out.reason = bounded_reason(&error); healthy = false; break;
        }
        out.written_complete_frames += u64::from(packet.frames);
        if let Err(error) = evidence.record(&packet) {
            out.reason = bounded_reason(&error); healthy = false; break;
        }
    }
    // Close the receiver immediately on any failed append, before potentially slow storage work.
    drop(rx);
    if healthy {
        match sink.finish() {
            Ok(inventory) if inventory.frames == out.written_complete_frames => {
                out.synchronized_frames = Some(out.written_complete_frames); out.validated = Some(inventory);
            }
            Ok(_) => out.notice("Finalized WAV writer inventory/count mismatch; partial retained"),
            Err(error) => out.notice(&error),
        }
    } else { drop(sink); } // Hound Drop's best-effort header update is never trusted as salvage.
    match evidence.finish() {
        Ok(()) => out.evidence_present = true,
        Err(error) => out.notice(&format!("Raw evidence unavailable; timing unknown: {error}")),
    }
    out
}

/// Own through capture release and the bounded storage phase; no successful detached completion.
pub struct WriterHandle {
    sender: Option<SyncSender<Packet>>,
    created: Receiver<Result<()>>,
    completion: Receiver<WriterOutcome>,
    join: Option<JoinHandle<()>>,
    creation_ok: bool,
    completed: bool,
}
impl WriterHandle {
    pub fn poll_created(&mut self) -> Result<bool> {
        if self.creation_ok { return Ok(true); }
        match self.created.try_recv() {
            Ok(Ok(())) => { self.creation_ok = true; Ok(true) },
            Ok(Err(error)) => Err(error),
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err("WAV creation acknowledgement disconnected".into()),
        }
    }
    pub fn take_sender(&mut self) -> Result<SyncSender<Packet>> {
        if !self.creation_ok { return Err("WAV creation must be acknowledged before producer Start".into()); }
        self.sender.take().ok_or_else(|| "Writer sender already moved/closed".into())
    }
    pub fn close_sender(&mut self) { self.sender.take(); }
    /// Returns a result once, only after observing and joining the real writer thread.
    pub fn poll_completion(&mut self) -> Result<Option<WriterOutcome>> {
        if self.completed || self.join.as_ref().is_some_and(|j| !j.is_finished()) { return Ok(None); }
        self.completed = true;
        let joined = self.join.take().ok_or("Writer join handle missing")?.join();
        if joined.is_err() { return Ok(Some(WriterOutcome::partial(true, "Writer panicked; partial unqualified"))); }
        match self.completion.try_recv() {
            Ok(outcome) => Ok(Some(outcome)),
            Err(_) => Ok(Some(WriterOutcome::partial(true, "Writer completion disconnected; partial unqualified"))),
        }
    }
}
impl Drop for WriterHandle {
    fn drop(&mut self) {
        self.sender.take();
        // Keep this owner during polling. Dropping it waits; an OS storage hang needs the parent's child deadline.
        if let Some(join) = self.join.take() { let _ = join.join(); }
    }
}
pub fn spawn_writer(path: &Path, format: Format) -> Result<WriterHandle> {
    spawn_writer_with_finalize(path, &path.with_extension("evidence.json"), format, Box::new(DefaultFinalize))
}
struct DefaultFinalize;
impl crate::wav::FinalizeOps for DefaultFinalize {}
struct FinalizeSink { sink: PartialWav<File>, ops: Box<dyn crate::wav::FinalizeOps + Send> }
impl crate::wav::FinalizeOps for Box<dyn crate::wav::FinalizeOps + Send> {
    fn before_append(&mut self) { (**self).before_append() }
    fn sync(&mut self, file: &File) -> Result<()> { (**self).sync(file) }
    fn full_validation(&mut self, path: &Path) -> bool { (**self).full_validation(path) }
}
impl WavSink for FinalizeSink {
    fn append(&mut self, packet: &Packet) -> Result<()> {
        self.ops.before_append();
        self.sink.append(packet)
    }
    fn finish(mut self) -> Result<WavInventory> { self.sink.finish_inventory_with(&mut self.ops) }
}
/// Shared worker needs fixed evidence basename and tests inject real finalization syscall outcomes.
/// Header/sample writing, channel creation, acknowledgement, loop and join remain identical.
pub fn spawn_writer_with_finalize(path: &Path, evidence_path: &Path, format: Format,
    ops: Box<dyn crate::wav::FinalizeOps + Send>) -> Result<WriterHandle> {
    format.validate()?;
    let path = path.to_path_buf();
    let evidence_path = evidence_path.to_path_buf();
    let (sender, rx) = mpsc::sync_channel(QUEUE_PACKETS);
    let (created_tx, created) = mpsc::sync_channel(1);
    let (completion_tx, completion) = mpsc::sync_channel(1);
    let join = thread::Builder::new().name("audio-wav-writer".into()).spawn(move || {
        let file = match create_new(&path) {
            Ok(file) => file,
            Err(error) => {
                let _ = created_tx.send(Err(bounded_reason(&error)));
                let _ = completion_tx.send(WriterOutcome::partial(false, &error)); return;
            }
        };
        let sink = match PartialWav::<File>::from_created_file(&path, format, file) {
            Ok(sink) => sink,
            Err(error) => {
                let _ = created_tx.send(Err(bounded_reason(&error)));
                let _ = completion_tx.send(WriterOutcome::partial(true, &error)); return;
            }
        };
        if created_tx.send(Ok(())).is_err() { return; }
        // Construction cannot fail after the validated format; still retain an unqualified partial on error.
        let evidence = match EvidenceBuilder::new(&evidence_path, format) {
            Ok(evidence) => evidence,
            Err(error) => { let _ = completion_tx.send(WriterOutcome::partial(true, &error)); return; }
        };
        let _ = completion_tx.send(run_writer(rx, FinalizeSink { sink, ops }, evidence));
    }).map_err(|e| e.to_string())?;
    Ok(WriterHandle { sender: Some(sender), created, completion, join: Some(join), creation_ok: false, completed: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    struct PanicSink;
    impl WavSink for PanicSink {
        fn append(&mut self, _: &Packet) -> Result<()> { panic!("injected sink panic") }
        fn finish(self) -> Result<WavInventory> { panic!("injected finalize panic") }
    }
    #[test]
    fn joined_panic_and_missing_completion_are_unqualified_once() {
        for panic in [true, false] {
            let (sender, rx) = mpsc::sync_channel(8);
            let (created_tx, created) = mpsc::sync_channel(1); drop(created_tx);
            let (completion_tx, completion) = mpsc::sync_channel(1);
            let join = thread::spawn(move || {
                let _owned_completion = completion_tx;
                if panic {
                    let f = Format { rate:48000, channels:2, bits:16, valid_bits:16, align:4, encoding:crate::format::Encoding::Pcm };
                    // Finalization panics inside the very same loop used by file-backed production.
                    let _ = run_writer(rx, PanicSink, EvidenceBuilder::new(Path::new("never-created.raw.json"), f).unwrap());
                }
            });
            let mut handle = WriterHandle { sender:Some(sender), created, completion, join:Some(join), creation_ok:false, completed:false };
            handle.close_sender(); let deadline=Instant::now()+Duration::from_secs(5);
            let out=loop { if let Some(out)=handle.poll_completion().unwrap() {break out;}
                assert!(Instant::now()<deadline); thread::yield_now(); };
            assert!(out.usable.is_none()); assert!(out.synchronized_frames.is_none()); assert!(out.retained_partial);
            assert!(handle.join.is_none()); assert!(handle.poll_completion().unwrap().is_none());
        }
    }
}
