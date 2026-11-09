use focusflow_windows_audio_worker::{format::{Encoding, Format}, packet::{Track, SILENT, DISCONTINUITY}};
use std::sync::mpsc::sync_channel;
fn format() -> Format { Format { rate: 48_000, channels: 2, bits: 16, valid_bits: 16, align: 4, encoding: Encoding::Pcm } }

#[test]
fn whole_packet_riff_and_24h_caps_precede_queue_admission() {
    for f in [format(), Format { rate: 8000, channels: 1, align: 2, ..format() }] {
        let (tx, rx) = sync_channel(8);
        let mut t = Track::new(f, tx).unwrap();
        let cap = (u64::from(f.rate) * 86400).min((u64::from(u32::MAX) - 4096) / u64::from(f.align));
        t.accepted_frames = cap - 1;
        assert!(t.admit(2, SILENT, 0, 0, None).is_err());
        assert_eq!(t.accepted_frames, cap - 1);
        assert!(rx.try_recv().is_err());
        assert!(t.ended.is_some());
        let (tx, rx) = sync_channel(8);
        let mut exact = Track::new(f, tx).unwrap(); exact.accepted_frames = cap - 1;
        exact.admit(1, SILENT, 0, 0, None).unwrap(); assert_eq!(exact.accepted_frames, cap);
        assert_eq!(rx.recv().unwrap().frames, 1);
        assert!(exact.admit(1, SILENT, 1, 1, None).is_err());
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn domain_1025_is_rejected_before_queue_admission() {
    let (tx, rx) = sync_channel(8);
    let mut t = Track::new(format(), tx).unwrap();
    for i in 0..1024 { t.admit(1, SILENT | DISCONTINUITY, i, i, None).unwrap(); rx.recv().unwrap(); }
    assert!(t.admit(1, SILENT | DISCONTINUITY, 1024, 1024, None).is_err());
    assert_eq!(t.accepted_frames, 1024);
    assert!(rx.try_recv().is_err());
}

use focusflow_windows_audio_worker::{packet::{Packet, TIMESTAMP_ERROR}, protocol::WavInventory,
    wav::{PartialWav, FinalizeOps}, writer::{run_writer, spawn_writer, EvidenceBuilder, WavSink}};
use std::{fs::{self, File}, io::{self, Write, Seek, SeekFrom}, path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp { fn new() -> Self {
    let p = std::env::temp_dir().join(format!("ff-task2-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::SeqCst)));
    fs::create_dir(&p).unwrap(); Self(p)
} fn wav(&self) -> PathBuf { self.0.join("owned.partial.wav") }
fn evidence(&self) -> PathBuf { self.0.join("raw.json") } }
impl Drop for Temp { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
fn packet(first: u64, frames: u32, f: Format) -> Packet { Packet { generation: 0, wav_first: first,
    device_first: first, frames, qpc100ns: first + 10, flags: SILENT, segment_start: first == 0,
    bytes: vec![0; frames as usize * usize::from(f.align)] } }
fn drain(t: &Temp, packets: Vec<Packet>, accepted: u64) -> focusflow_windows_audio_worker::writer::WriterOutcome {
    let (tx, rx) = sync_channel(8); for p in packets { tx.send(p).unwrap(); } drop(tx);
    run_writer(rx, PartialWav::create(&t.wav(), format()).unwrap(),
        EvidenceBuilder::new(&t.evidence(), format()).unwrap()).reconcile(accepted, None)
}
#[test]
fn created_ack_precedes_start_and_completion_is_joined_once() {
    let t = Temp::new(); let mut h = spawn_writer(&t.wav(), format()).unwrap();
    assert!(h.take_sender().is_err());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !h.poll_created().unwrap() { assert!(Instant::now() < deadline); std::thread::yield_now(); }
    assert!(t.wav().is_file());
    let mut track = Track::new(format(), h.take_sender().unwrap()).unwrap();
    assert!(h.take_sender().is_err());
    track.admit(2, SILENT, 9, 10, None).unwrap(); let accepted = track.accepted_frames; drop(track);
    let out = loop { if let Some(o) = h.poll_completion().unwrap() { break o; }
        assert!(Instant::now() < deadline); std::thread::yield_now(); };
    assert!(out.usable.is_none());
    assert_eq!(out.reconcile(accepted, None).usable.unwrap().frames, 2);
    assert!(h.poll_completion().unwrap().is_none());
}
#[test]
fn create_failure_preserves_existing_path_and_completes_without_owned_partial() {
    let t = Temp::new(); fs::write(t.wav(), b"supplied old fixture").unwrap();
    let mut h = spawn_writer(&t.wav(), format()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop { match h.poll_created() { Err(_) => break, Ok(false) => (), Ok(true) => panic!("existing file accepted") }
        assert!(Instant::now() < deadline); std::thread::yield_now(); }
    let out = loop { if let Some(o) = h.poll_completion().unwrap() { break o; }
        assert!(Instant::now() < deadline); std::thread::yield_now(); };
    assert!(!out.retained_partial); assert!(out.usable.is_none());
    assert_eq!(fs::read(t.wav()).unwrap(), b"supplied old fixture");
}
#[test]
fn drain_matches_accepted_complete_frames() {
    let t = Temp::new(); let (tx, rx) = sync_channel(8); let mut tr = Track::new(format(), tx).unwrap();
    for i in 0..4 { tr.admit(3, SILENT, i * 3, i + 10, None).unwrap(); }
    let accepted = tr.accepted_frames; drop(tr);
    let o = run_writer(rx, PartialWav::create(&t.wav(), format()).unwrap(), EvidenceBuilder::new(&t.evidence(), format()).unwrap());
    assert_eq!(o.synchronized_frames, Some(12)); assert!(o.usable.is_none());
    let o = o.reconcile(accepted, None); assert_eq!(o.accepted_frames, Some(12));
    assert_eq!(o.usable.unwrap().frames, 12); assert_eq!(o.written_complete_frames, 12); assert!(o.evidence_present);
}
#[test]
fn producer_failure_can_publish_valid_drained_prefix() {
    let t = Temp::new(); let o = drain(&t, vec![packet(0, 2, format())], 2)
        .reconcile(2, Some("producer device invalidated"));
    assert_eq!(o.usable.unwrap().frames, 2); assert!(o.reason.contains("device invalidated"));
}
#[test]
fn accepted_more_than_written_cannot_publish_usable() {
    let t = Temp::new(); let o = drain(&t, vec![packet(0, 2, format())], 3);
    assert!(o.usable.is_none()); assert_eq!(o.written_complete_frames, 2);
    assert_eq!(o.synchronized_frames, Some(2)); assert!(t.wav().is_file());
}
#[test]
fn zero_frame_vs_silent_wav() {
    let empty = Temp::new(); let e = drain(&empty, vec![], 0);
    assert!(e.usable.is_none()); assert_eq!(e.written_complete_frames, 0); assert!(empty.wav().is_file());
    let silent = Temp::new(); let s = drain(&silent, vec![packet(0, 1, format())], 1);
    assert_eq!(s.usable.unwrap().frames, 1);
    assert_eq!(hound::WavReader::open(silent.wav()).unwrap().samples::<i16>().map(Result::unwrap).collect::<Vec<_>>(), [0, 0]);
    let json: serde_json::Value = serde_json::from_slice(&fs::read(silent.evidence()).unwrap()).unwrap();
    assert_eq!(json["silent_packets"], 1); assert_eq!(json["raw_tail"]["flags"], SILENT);
    assert_eq!(json["exclusive_end"], "Unmeasured"); assert_eq!(json["first_sample"], "Unmeasured");
}
#[test]
fn evidence_failure_keeps_valid_wav_timing_unknown() {
    let t = Temp::new(); fs::write(t.evidence(), b"old raw evidence").unwrap();
    let o = drain(&t, vec![packet(0, 2, format())], 2);
    assert_eq!(o.usable.unwrap().frames, 2); assert!(!o.evidence_present);
    assert!(o.reason.contains("evidence")); assert_eq!(fs::read(t.evidence()).unwrap(), b"old raw evidence");
}
#[test]
fn cancel_storage_retains_partial_without_usable() {
    let t = Temp::new(); let mut o = drain(&t, vec![packet(0, 2, format())], 2);
    assert!(o.usable.is_some()); o.cancel_storage();
    assert!(o.usable.is_none()); assert!(o.retained_partial); assert!(t.wav().is_file());
    assert!(o.reconcile(2, None).usable.is_none());
}
struct FaultIo { file: File, armed: Arc<AtomicBool>, remaining: usize, fail_seek: bool }
impl Write for FaultIo {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.armed.load(Ordering::SeqCst) && !self.fail_seek {
            if self.remaining == 0 { return Err(io::Error::other("injected sample write")); }
            let n = b.len().min(self.remaining); let n = self.file.write(&b[..n])?; self.remaining -= n; Ok(n)
        } else { self.file.write(b) }
    }
    fn flush(&mut self) -> io::Result<()> { self.file.flush() }
}
impl Seek for FaultIo { fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
    if self.fail_seek && self.armed.load(Ordering::SeqCst) { Err(io::Error::other("injected finalize seek")) }
    else { self.file.seek(p) }
} }
struct Ops(u8);
impl FinalizeOps for Ops {
    fn sync(&mut self, file: &File) -> focusflow_windows_audio_worker::Result<()> {
        if self.0 == 1 { return Err("injected sync failure".into()); }
        file.sync_all().map_err(|e| e.to_string())?;
        if self.0 == 3 { file.set_len(file.metadata().unwrap().len() - 1).unwrap(); }
        Ok(())
    }
    fn full_validation(&mut self, path: &Path) -> bool {
        self.0 != 2 && focusflow_core::audio::is_usable_wav(path)
    }
}
struct FinishSink { wav: PartialWav<FaultIo>, ops: Ops }
impl WavSink for FinishSink {
    fn append(&mut self, p: &Packet) -> focusflow_windows_audio_worker::Result<()> { self.wav.append(p) }
    fn finish(mut self) -> focusflow_windows_audio_worker::Result<WavInventory> { self.wav.finish_inventory_with(&mut self.ops) }
}
#[test]
fn mid_channel_write_poison_retains_partial() {
    let t = Temp::new(); let armed = Arc::new(AtomicBool::new(false));
    let mut w = PartialWav::create_with(&t.wav(), format(), |file| FaultIo { file, armed: armed.clone(), remaining: 2, fail_seek: false }).unwrap();
    w.append(&packet(0, 1, format())).unwrap(); armed.store(true, Ordering::SeqCst);
    assert!(w.append(&packet(1, 1, format())).is_err()); assert_eq!(w.written_complete_frames(), 1);
    assert!(w.append(&packet(1, 1, format())).is_err()); assert!(w.finish_inventory().is_err());
    assert!(fs::metadata(t.wav()).unwrap().len() > 44);
    let t2 = Temp::new(); let a = Arc::new(AtomicBool::new(false));
    let w = PartialWav::create_with(&t2.wav(), format(), |file| FaultIo { file, armed: a.clone(), remaining: 2, fail_seek: false }).unwrap();
    a.store(true, Ordering::SeqCst); let (tx, rx) = sync_channel(8);
    tx.send(packet(0, 1, format())).unwrap(); tx.send(packet(1, 1, format())).unwrap(); drop(tx);
    let o = run_writer(rx, w, EvidenceBuilder::new(&t2.evidence(), format()).unwrap()).reconcile(2, None);
    assert_eq!(o.written_complete_frames, 0); assert!(o.usable.is_none()); assert!(o.synchronized_frames.is_none()); assert!(t2.wav().is_file());
}
#[test]
fn finalize_sync_or_full_validation_failure_is_unusable() {
    for stage in 0..4 {
        let t = Temp::new(); let armed = Arc::new(AtomicBool::new(false));
        let w = PartialWav::create_with(&t.wav(), format(), |file| FaultIo { file, armed: armed.clone(), remaining: usize::MAX, fail_seek: stage == 0 }).unwrap();
        let mut sink = FinishSink { wav: w, ops: Ops(stage) };
        sink.append(&packet(0, 2, format())).unwrap(); armed.store(true, Ordering::SeqCst);
        // Direct finalization and the production loop both retain the failed real file.
        assert!(sink.finish().is_err()); assert!(t.wav().is_file());
        let t2 = Temp::new(); let armed2 = Arc::new(AtomicBool::new(false));
        let w2 = PartialWav::create_with(&t2.wav(), format(), |file| FaultIo { file, armed: armed2.clone(), remaining: usize::MAX, fail_seek: stage == 0 }).unwrap();
        armed2.store(true, Ordering::SeqCst);
        let (tx, rx) = sync_channel(8); tx.send(packet(0, 2, format())).unwrap(); drop(tx);
        let o = run_writer(rx, FinishSink { wav: w2, ops: Ops(stage) }, EvidenceBuilder::new(&t2.evidence(), format()).unwrap()).reconcile(2, None);
        assert!(o.usable.is_none()); assert!(o.synchronized_frames.is_none()); assert_eq!(o.written_complete_frames, 2); assert!(t2.wav().is_file());
    }
}
#[test]
fn pcm_and_finite_float_mono_stereo_preserve_exact_samples_and_inventory() {
    for channels in [1, 2] { for (encoding, bits) in [(Encoding::Pcm, 16), (Encoding::Pcm, 24), (Encoding::Pcm, 32), (Encoding::Float, 32)] {
        let t = Temp::new(); let f = Format { channels, bits, valid_bits: bits, align: channels * (bits / 8), encoding, ..format() };
        let bytes = match (encoding, bits) { (Encoding::Pcm, 16) => vec![0,128,255,127], (Encoding::Pcm, 24) => vec![0,0,128,255,255,127], (Encoding::Pcm,32) => vec![0,0,0,128,255,255,255,127], _ => [-1.0f32, 0.25].into_iter().flat_map(f32::to_le_bytes).collect() };
        let p = Packet { frames: 2 / u32::from(channels), bytes: bytes.clone(), flags: 0, ..packet(0, 1, f) };
        let (tx, rx) = sync_channel(8); tx.send(p).unwrap(); drop(tx);
        let o = run_writer(rx, PartialWav::create(&t.wav(), f).unwrap(), EvidenceBuilder::new(&t.evidence(), f).unwrap()).reconcile(2 / u64::from(channels), None);
        let i = o.usable.unwrap(); assert_eq!((i.channels, i.bits, i.encoding, i.rate), (channels, bits, encoding, 48000));
        assert_eq!(i.file_bytes, fs::metadata(t.wav()).unwrap().len());
        let mut reader = hound::WavReader::open(t.wav()).unwrap();
        if encoding == Encoding::Float { assert_eq!(reader.samples::<f32>().map(Result::unwrap).collect::<Vec<_>>(), [-1.0,0.25]); }
        else { let want = match bits {16 => [-32768,32767], 24 => [-8388608,8388607], _ => [i32::MIN,i32::MAX]};
            assert_eq!(reader.samples::<i32>().map(Result::unwrap).collect::<Vec<_>>(), want); }
    } }
}
#[test]
fn malformed_packets_never_append_and_keep_existing_prefix() {
    for case in 0..6 { let t = Temp::new(); let mut p = packet(1, 1, format());
        match case { 0 => p.wav_first = 7, 1 => {p.bytes.pop();}, 2 => p.frames = 2, 3 => p.frames = 0,
            4 => p.flags = 8, _ => p.bytes = vec![0; 4 * 1024 * 1024 + 4] }
        let o = drain(&t, vec![packet(0, 1, format()), p], 2);
        assert!(o.usable.is_none()); assert_eq!(o.written_complete_frames, 1); assert!(t.wav().is_file());
    }
}
#[test]
fn raw_evidence_is_bounded_under_recovery_and_discontinuity() {
    let t = Temp::new(); let mut e = EvidenceBuilder::new(&t.evidence(), format()).unwrap();
    for domain in 0..1024u32 { for j in 0..1100u64 {
        let mut p = packet(u64::from(domain)*1100+j, 1, format()); p.generation = domain;
        p.device_first = u64::MAX - 1100 + j; p.qpc100ns = u64::MAX - 1100 + j;
        p.flags = SILENT | if j == 0 {DISCONTINUITY | TIMESTAMP_ERROR} else {0}; p.segment_start = j == 0;
        e.check_packet(&p).unwrap(); e.record(&p).unwrap();
    } }
    let p = Packet {generation:1024, segment_start:true, ..packet(1126400,1,format())};
    assert!(e.check_packet(&p).is_err());
    e.finish().unwrap(); let bytes = fs::read(t.evidence()).unwrap(); assert!(bytes.len() <= 2 * 1024 * 1024);
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["domains"].as_array().unwrap().len(), 1024); assert_eq!(v["packets"],1126400);
    assert_eq!(v["timestamp_errors"],1024); assert_eq!(v["discontinuities"],1024);
    for (i,d) in v["domains"].as_array().unwrap().iter().enumerate() {
        assert_eq!(d["first"]["wav_first"], (i as u64)*1100); assert_eq!(d["latest"]["wav_first"], (i as u64)*1100+1099);
        assert!(d["landmarks"].as_array().unwrap().len() == 8);
    }
    assert_eq!(v["raw_tail"]["wav_first"],1126399); assert_eq!(v["exclusive_end"],"Unmeasured");
}
#[cfg(unix)]
#[test]
fn symlink_file_and_parent_are_rejected_without_touching_targets() {
    let t = Temp::new(); let target = t.0.join("old.wav"); fs::write(&target,b"untouched").unwrap();
    std::os::unix::fs::symlink(&target,t.wav()).unwrap(); assert!(PartialWav::create(&t.wav(),format()).is_err());
    assert_eq!(fs::read(&target).unwrap(),b"untouched");
    let real = t.0.join("real"); fs::create_dir(&real).unwrap(); let link=t.0.join("link"); std::os::unix::fs::symlink(&real,&link).unwrap();
    assert!(PartialWav::create(&link.join("new.wav"),format()).is_err()); assert!(!real.join("new.wav").exists());
}

struct HeaderCorruption(u8);
impl FinalizeOps for HeaderCorruption {
    fn sync(&mut self, file: &File) -> focusflow_windows_audio_worker::Result<()> {
        let mut f = file.try_clone().unwrap();
        let (offset, bytes) = match self.0 {
            0 => (24, 44100u32.to_le_bytes().to_vec()), // Valid WAV, wrong sample rate.
            1 => (40, 4u32.to_le_bytes().to_vec()), // Declares one of the two stereo frames.
            2 => (40, 6u32.to_le_bytes().to_vec()), // Three complete samples, incomplete channel frame.
            _ => (44, 3u32.to_le_bytes().to_vec()), // PCM32 extensible subformat changed to float32.
        };
        f.seek(SeekFrom::Start(offset)).unwrap(); f.write_all(&bytes).unwrap();
        if self.0 == 0 { f.seek(SeekFrom::Start(28)).unwrap(); f.write_all(&176400u32.to_le_bytes()).unwrap(); }
        f.sync_all().map_err(|e| e.to_string())
    }
}
#[test]
fn full_inventory_rejects_wrong_counts_partial_channel_rate_and_encoding() {
    for mode in 0..4 {
        let t = Temp::new(); let f = if mode == 3 {Format {bits:32, valid_bits:32, align:8, ..format()}} else {format()};
        let mut wav = PartialWav::create(&t.wav(), f).unwrap(); wav.append(&packet(0,2,f)).unwrap();
        let err = wav.finish_inventory_with(&mut HeaderCorruption(mode)).unwrap_err();
        assert!(t.wav().is_file());
        if mode == 2 { assert!(err.contains("complete sample validation")); }
        else {
            assert!(err.contains("inventory mismatch"), "{mode}: {err}");
            assert!(focusflow_core::audio::is_usable_wav(&t.wav()), "fixture must pass generic full validator to reach exact inventory check");
        }
    }
}
#[test]
fn nonfinite_float_packet_never_reaches_wav_sample_writes() {
    let t = Temp::new(); let f = Format {encoding:Encoding::Float, bits:32, valid_bits:32, align:8, ..format()};
    let mut p = packet(1,1,f); p.bytes = [f32::NAN,0.0].into_iter().flat_map(f32::to_le_bytes).collect();
    let (tx,rx)=sync_channel(8); tx.send(packet(0,1,f)).unwrap(); tx.send(p).unwrap(); drop(tx);
    let out=run_writer(rx,PartialWav::create(&t.wav(),f).unwrap(),EvidenceBuilder::new(&t.evidence(),f).unwrap()).reconcile(2,None);
    assert!(out.usable.is_none()); assert_eq!(out.written_complete_frames,1); assert!(out.synchronized_frames.is_none());
    assert!(t.wav().is_file());
}
#[test]
fn queue_full_ends_producer_but_all_eight_accepted_packets_drain_to_valid_prefix() {
    let t=Temp::new(); let (tx,rx)=sync_channel(8); let mut track=Track::new(format(),tx).unwrap();
    for i in 0..8 {track.admit(1,SILENT,i,i+10,None).unwrap();}
    assert!(track.admit(1,SILENT,8,18,None).is_err()); assert_eq!(track.accepted_frames,8);
    let reason=track.ended.clone(); drop(track);
    let out=run_writer(rx,PartialWav::create(&t.wav(),format()).unwrap(),EvidenceBuilder::new(&t.evidence(),format()).unwrap()).reconcile(8,reason.as_deref());
    assert_eq!(out.usable.unwrap().frames,8); assert_eq!(out.written_complete_frames,8);
    assert!(out.reason.contains("queue unavailable"));
}
