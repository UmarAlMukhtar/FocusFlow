use focusflow_windows_audio_worker::{
    clock::*, format::*, packet::*, protocol::*, wav::*
};
use std::sync::mpsc::sync_channel;
use std::time::{Duration, Instant};
const NONCE: &str = "00112233-4455-6677-8899-aabbccddeeff";
fn at(base: Instant, ms: u64) -> Instant { base + Duration::from_millis(ms) }
fn owner(base: Instant) -> Owner { Owner::new(at(base, 8000), NONCE.into(), TrackKind::Microphone).unwrap() }
fn evt(event: Event) -> EventV1 { EventV1 { version: 1, nonce: NONCE.into(), kind: TrackKind::Microphone, event } }
fn observed(qpc: u64) -> ClockStamp { ClockStamp { qpc, frequency: 1000 } }

fn fmt(bits: u16) -> Format {
    Format {
        rate: 48_000, channels: 2, bits, valid_bits: bits, align: bits/4, encoding: Encoding::Pcm
    }
}
#[test] fn rejects_bad_mix_before_readiness() {
    assert_eq!(fmt(16).validate().unwrap(), fmt(16));
    for f in [Format {
        channels: 3, ..fmt(16)
    }, Format {
        rate: 7999, ..fmt(16)
    }, Format {
        rate: 192001, ..fmt(16)
    }, Format {
        align: 1, ..fmt(16)
    }, Format {
        valid_bits: 15, ..fmt(16)
    }] {
        assert!(f.validate().is_err());
    }
}
#[test] fn decodes_pcm_extremes_and_rejects_nonfinite_or_unaligned() {
    assert_eq!(fmt(16).decode(&[0, 128, 255, 127]).unwrap(), vec![-1.0, 32767.0/32768.0]);
    assert_eq!(fmt(24).decode(&[0, 0, 128, 255, 255, 127]).unwrap(), vec![-1.0, 8388607.0/8388608.0]);
    assert_eq!(fmt(32).decode(&[0, 0, 0, 128, 0, 0, 0, 0]).unwrap(), vec![-1.0, 0.0]);
    let f = Format {
        encoding: Encoding::Float, ..fmt(32)
    };
    assert!(f.decode(&[0, 0, 128, 127, 0, 0, 0, 0]).is_err());
    assert!(fmt(16).decode(&[0]).is_err());
}
#[test] fn empty_never_reads_outputs_and_silent_ignores_data() {
    let(tx, rx) = sync_channel(2);
    let mut t = Track::new(fmt(16), tx).unwrap();
    t.admit(0, u32::MAX, u64::MAX, u64::MAX, None).unwrap();
    assert_eq!(t.accepted_frames, 0);
    assert!(rx.try_recv().is_err());
    t.admit(2, SILENT, 700, 800, None).unwrap();
    let p = rx.try_recv().unwrap();
    assert_eq!(p.bytes, vec![0;
    8]);
    assert_eq!((p.wav_first, p.device_first, p.qpc100ns), (0, 700, 800));
}
#[test] fn overflow_preserves_prefix_and_ends_without_deleting_packets() {
    let(tx, rx) = sync_channel(1);
    let mut t = Track::new(fmt(16), tx).unwrap();
    t.admit(2, 0, 90, 100, Some(&[1;
    8])).unwrap();
    assert!(t.admit(2, 0, 92, 200, Some(&[2;
    8])).is_err());
    assert_eq!(t.accepted_frames, 2);
    assert!(t.ended.is_some());
    assert_eq!(rx.recv().unwrap().bytes, vec![1;
    8]);
    assert!(t.admit(1, SILENT, 94, 300, None).is_err());
}
#[test] fn unknown_backwards_oversize_and_missing_data_terminate_track() {
    for (frames, flags, device, qpc, data) in [(1, 8, 100, 200, Some(&[0;
    4][..])), (1, 0, 99, 200, Some(&[0;
    4][..])), (1, 0, 101, 99, Some(&[0;
    4][..])), (u32::MAX, SILENT, 101, 200, None), (1, 0, 101, 200, None)] {
        let(tx, _rx) = sync_channel(2);
        let mut t = Track::new(fmt(16), tx).unwrap();
        t.admit(1, SILENT, 100, 100, None).unwrap();
        assert!(t.admit(frames, flags, device, qpc, data).is_err());
        assert_eq!(t.accepted_frames, 1);
        assert!(t.ended.is_some());
    }
}
#[test] fn device_jumps_flags_and_recovery_split_domains_without_synthesized_samples() {
    let(tx, rx) = sync_channel(8);
    let mut t = Track::new(fmt(16), tx).unwrap();
    for (dev, qpc, flag) in [(100, 200, 0), (101, 400, 0), (500, 600, 0), (501, 800, TIMESTAMP_ERROR), (502, 1000, DISCONTINUITY)] {
        t.admit(1, flag|SILENT, dev, qpc, None).unwrap();
    }
    t.recover(fmt(16)).unwrap();
    t.admit(1, SILENT, 1, 1, None).unwrap();
    let p: Vec<_>=rx.try_iter().collect();
    assert_eq!(p.iter().map(|p|p.segment_start).collect::<Vec<_>>(), vec![true, false, true, true, true, true]);
    assert_eq!((p[5].generation, p[5].wav_first, p[5].device_first), (1, 5, 1));
    assert_eq!(t.accepted_frames, 6);
    assert!(t.recover(fmt(24)).is_err());
    assert!(t.ended.is_some());
}
#[test] fn packet_clock_is_100ns_and_raw_cutoff_uses_its_frequency_once() {
    assert_eq!(offset_us(995, 1000), Rational {
        numerator: -5, denominator: 10
    });
    assert_eq!(raw_cutoff_100ns(9, 3).unwrap(), Rational {
        numerator: 90_000_000, denominator: 3
    });
    assert!(raw_cutoff_100ns(1, 0).is_err());
    assert_eq!(interpolate(150, (100, 1000), (200, 3000)).unwrap(), Rational {
        numerator: 200_000, denominator: 100
    });
    assert_eq!(drift_ppm((0, 0), (48000, 10_001_000), 48000).unwrap(), Rational {
        numerator: 48_000_000_000_000, denominator: 480_000_000_000
    });
    assert!(interpolate(201, (100, 1000), (200, 3000)).is_err());
}
#[test] fn ownership_deadlines_keep_cleanup_pending_until_observed_exit() {
    let base = Instant::now(); let mut o = owner(base);
    assert!(o.message(base, observed(0), &evt(Event::Ready { format: fmt(16) })).is_err());
    o.assigned(base).unwrap(); o.tick(at(base, 7999), false).unwrap();
    assert_eq!(o.state, State::Starting); o.tick(at(base, 8000), false).unwrap();
    assert_eq!(o.state, State::ForceExit); o.tick(at(base, 10000), false).unwrap();
    assert_eq!(o.state, State::CleanupPending); o.tick(at(base, 10001), true).unwrap();
    assert_eq!(o.state, State::Exited);
    let mut o = owner(base); o.assigned(base).unwrap();
    o.message(at(base, 1), observed(1), &evt(Event::Ready { format: fmt(16) })).unwrap();
    o.stop(at(base, 20)).unwrap(); o.tick(at(base, 3019), false).unwrap();
    assert_eq!(o.state, State::Stopping); o.tick(at(base, 3020), false).unwrap();
    assert_eq!(o.state, State::ForceExit); o.tick(at(base, 5020), true).unwrap();
    assert_eq!(o.state, State::Exited);
}
#[test] fn recovery_never_substitutes_id_and_has_total_budget() {
    let mut r = Recovery::new("opaque-A", 100).unwrap();
    assert!(r.attempt("same friendly label", 101).is_err());
    assert_eq!(r.attempt("opaque-A", 101).unwrap(), 1);
    assert_eq!(r.attempt("opaque-A", 102).unwrap(), 2);
    assert!(r.attempt("opaque-A", 103).is_err());
    let mut r = Recovery::new("opaque-A", 100).unwrap();
    assert!(r.attempt("opaque-A", 5100).is_err());
}
#[test] fn control_rejects_oversize_truncation_and_unknown_fields() {
    let ready = evt(Event::Ready { format: fmt(16) });
    let bytes = encode_event(&ready).unwrap();
    assert_eq!(read_event(&bytes[..], NONCE, TrackKind::Microphone).unwrap(), ready);
    assert!(read_event(&vec![b'a'; 4097][..], NONCE, TrackKind::Microphone).is_err());
    assert!(read_event(&bytes[..bytes.len()-1], NONCE, TrackKind::Microphone).is_err());
    let mut value = serde_json::to_value(&ready).unwrap(); value["extra"] = 1.into();
    let mut bytes = serde_json::to_vec(&value).unwrap(); bytes.push(b'\n');
    assert!(read_event(&bytes[..], NONCE, TrackKind::Microphone).is_err());
}
#[test] fn exclusive_partial_validates_every_sample_and_preserves_existing_file() {
    let dir = std::env::temp_dir().join(format!("ff-audio-prep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("owned.partial.wav");
    let existing = dir.join("existing.wav");
    std::fs::write(&existing, b"keep").unwrap();
    assert!(PartialWav::create(&existing, fmt(16)).is_err());
    assert_eq!(std::fs::read(&existing).unwrap(), b"keep");
    let mut w = PartialWav::create(&path, fmt(16)).unwrap();
    w.append(&Packet {
        generation: 0, wav_first: 0, device_first: 20, frames: 1, qpc100ns: 30, flags: SILENT, segment_start: true, bytes: vec![0;
        4]
    }).unwrap();
    let p = w.finish().unwrap();
    assert!(focusflow_core::audio::is_usable_wav(&p));
    std::fs::remove_dir_all(dir).unwrap();
}
#[test] fn overlap_and_invalid_recovery_are_terminal_without_prefix_change() {
    let(tx, _rx) = sync_channel(4);
    let mut t = Track::new(fmt(16), tx).unwrap();
    t.admit(2, SILENT, 100, 100, None).unwrap();
    assert!(t.admit(1, SILENT, 101, 200, None).is_err());
    assert_eq!(t.accepted_frames, 2);
    let(tx, _rx) = sync_channel(4);
    let mut t = Track::new(fmt(16), tx).unwrap();
    assert!(t.recover(Format {
        channels: 0, ..fmt(16)
    }).is_err());
    assert!(t.ended.is_some());
}
#[test] fn timestamp_error_never_becomes_anchor_and_disconnected_writer_ends_prefix() {
    let p = Packet {
        generation: 0, wav_first: 0, device_first: 20, frames: 1, qpc100ns: 30, flags: TIMESTAMP_ERROR, segment_start: true, bytes: vec![0;
        4]
    };
    assert!(!p.trusted_anchor());
    let(tx, rx) = sync_channel(1);
    drop(rx);
    let mut t = Track::new(fmt(16), tx).unwrap();
    assert!(t.admit(1, SILENT, 0, 0, None).is_err());
    assert_eq!(t.accepted_frames, 0);
    assert!(t.ended.is_some());
}
#[test] fn startup_deadline_cannot_be_extended_by_assignment_and_stop_cannot_restart() {
    let base = Instant::now(); let mut o = owner(base);
    o.assigned(at(base, 7999)).unwrap();
    assert!(o.message(at(base, 8000), observed(8000), &evt(Event::Ready { format: fmt(16) })).is_err());
    o.tick(at(base, 8000), false).unwrap(); assert_eq!(o.state, State::ForceExit);
    let mut o = owner(base); o.assigned(at(base, 1)).unwrap();
    o.message(at(base, 2), observed(2), &evt(Event::Ready { format: fmt(16) })).unwrap();
    o.stop(at(base, 3)).unwrap(); o.stop(at(base, 3000)).unwrap();
    o.tick(at(base, 3003), false).unwrap(); assert_eq!(o.state, State::ForceExit);
}
#[test] fn empty_wav_and_corrupt_payload_are_never_usable() {
    let dir = std::env::temp_dir().join(format!("ff-audio-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("empty.partial.wav");
    let w = PartialWav::create(&path, fmt(16)).unwrap();
    assert!(w.finish().is_err());
    assert!(!focusflow_core::audio::is_usable_wav(&path));
    let path2 = dir.join("good.partial.wav");
    let mut w = PartialWav::create(&path2, Format {
        encoding: Encoding::Float, ..fmt(32)
    }).unwrap();
    let p = Packet {
        generation: 0, wav_first: 0, device_first: 0, frames: 1, qpc100ns: 0, flags: 0, segment_start: true, bytes: vec![0, 0, 128, 127, 0, 0, 0, 0]
    };
    assert!(w.append(&p).is_err());
    assert!(w.finish().is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test] fn terminal_protocol_requires_ready_then_stop_ack_and_disallows_duplicates() {
    let base = Instant::now(); let mut o = owner(base); o.assigned(base).unwrap();
    assert!(o.message(at(base, 1), observed(1), &evt(Event::StopAck { observed: observed(1) })).is_err());
    o.message(at(base, 1), observed(1), &evt(Event::Ready { format: fmt(16) })).unwrap();
    assert!(o.message(at(base, 2), observed(2), &evt(Event::Ready { format: fmt(16) })).is_err());
    o.stop(at(base, 3)).unwrap();
    let terminal = evt(Event::Terminal { outcome: TerminalSummary { accepted_frames: 12,
        written_complete_frames: 12, synchronized_frames: None, usable: None,
        retained_partial: true, cause: "accepted prefix: queue full".into(), evidence_present: false } });
    assert!(o.message(at(base, 4), observed(4), &terminal).is_err());
    o.message(at(base, 4), observed(4), &evt(Event::StopAck { observed: observed(4) })).unwrap();
    assert!(o.message(at(base, 4), observed(4), &evt(Event::StopAck { observed: observed(4) })).is_err());
    assert!(o.message(at(base, 5), observed(5), &terminal).is_err());
    o.message(at(base, 5), observed(5), &evt(Event::CaptureReleased { at: observed(5) })).unwrap();
    o.message(at(base, 6), observed(6), &terminal).unwrap();
    assert!(o.message(at(base, 6), observed(6), &terminal).is_err());
}
#[test] fn wav_preserves_pcm24_storage_and_exact_integer_samples() {
    let dir = std::env::temp_dir().join(format!("ff-audio-pcm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pcm.partial.wav");
    let mut w = PartialWav::create(&path, fmt(24)).unwrap();
    let p = Packet {
        generation: 0, wav_first: 0, device_first: 20, frames: 1, qpc100ns: 30, flags: 0, segment_start: true, bytes: vec![1, 0, 128, 255, 255, 127]
    };
    w.append(&p).unwrap();
    let path = w.finish().unwrap();
    let mut r = hound::WavReader::open(path).unwrap();
    assert_eq!((r.spec().bits_per_sample, r.spec().sample_format), (24, hound::SampleFormat::Int));
    assert_eq!(r.samples::<i32>().map(Result::unwrap).collect::<Vec<_>>(), vec![-8388607, 8388607]);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn only_standard_mono_stereo_layouts_reach_wav_readiness() {
    assert!(fmt(16).validate_mask(3).is_ok());
    assert!(fmt(16).validate_mask(0).is_ok());
    assert!(fmt(16).validate_mask(5).is_err());
    let mono = Format { channels: 1, align: 2, ..fmt(16) };
    assert!(mono.validate_mask(4).is_ok());
    assert!(mono.validate_mask(1).is_err());
}

#[test]
fn retained_live_pending_owner_blocks_replacement_until_its_exact_exit_is_observed() {
    use focusflow_windows_audio_worker::pending::{ObservedExit, PendingRegistry, RetainedChild};
    use std::{cell::Cell, rc::Rc};

    struct ExactResource { id: u64, drops: Rc<Cell<u32>> }
    impl Drop for ExactResource {
        fn drop(&mut self) { self.drops.set(self.drops.get() + 1); }
    }

    let drops = Rc::new(Cell::new(0));
    let mut registry = PendingRegistry::new();
    let mut retained = RetainedChild::new(ExactResource { id: 17, drops: drops.clone() });
    assert!(registry.replacement_allowed());
    retained.retain_pending(&mut registry);
    retained.retain_pending(&mut registry);
    assert_eq!(registry.len(), 1);
    assert!(!registry.replacement_allowed());
    assert!(retained.cleanup_pending());
    assert_eq!(drops.get(), 0);

    let calls = Cell::new(0);
    let mut observed = None;
    {
        let mut poll = |owned: &mut ExactResource| {
            calls.set(calls.get() + 1);
            observed.filter(|(id, _)| *id == owned.id)
                .map(|(_, code)| ObservedExit { code: Some(code) })
        };
        assert_eq!(retained.poll_pending(&mut registry, &mut poll), None);
        assert_eq!(calls.get(), 1);
        assert!(!registry.replacement_allowed());
        assert!(retained.cleanup_pending());
    }

    observed = Some((18, 90));
    registry.poll(|owned| observed.filter(|(id, _)| *id == owned.id)
        .map(|(_, code)| ObservedExit { code: Some(code) }));
    assert!(!registry.replacement_allowed());
    assert_eq!(drops.get(), 0);

    observed = Some((17, 23));
    registry.poll(|owned| observed.filter(|(id, _)| *id == owned.id)
        .map(|(_, code)| ObservedExit { code: Some(code) }));
    assert!(registry.replacement_allowed());
    assert_eq!(drops.get(), 1);
    assert!(!retained.cleanup_pending());
    let exit = retained.poll_pending(&mut registry, |_| {
        panic!("The exact resource was already observed and released");
    });
    assert_eq!(exit, Some(ObservedExit { code: Some(23) }));
    drop(retained);
    assert_eq!(drops.get(), 1);

    let mut dropped_owner = RetainedChild::new(ExactResource { id: 29, drops: drops.clone() });
    dropped_owner.retain_pending(&mut registry);
    drop(dropped_owner);
    assert_eq!(registry.len(), 1);
    assert!(!registry.replacement_allowed());
    assert_eq!(drops.get(), 1);
    registry.poll(|owned| {
        assert_eq!(owned.id, 29);
        Some(ObservedExit { code: None })
    });
    assert!(registry.replacement_allowed());
    drop(registry);
    assert_eq!(drops.get(), 2);
}
