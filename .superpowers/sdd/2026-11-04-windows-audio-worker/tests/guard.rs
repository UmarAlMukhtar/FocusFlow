use focusflow_windows_audio_worker::{format::{Encoding, Format}, protocol::*};
use std::{path::PathBuf, time::{Duration, Instant}};
const NONCE: &str = "00112233-4455-6677-8899-aabbccddeeff";
fn ms(t: Instant, n: u64) -> Instant { t + Duration::from_millis(n) }
fn stamp(qpc: u64) -> ClockStamp { ClockStamp { qpc, frequency: 1000 } }
fn format() -> Format { Format { rate: 48000, channels: 2, bits: 16, valid_bits: 16, align: 4, encoding: Encoding::Pcm } }
fn request() -> TrackRequest { TrackRequest { kind: TrackKind::Microphone, endpoint_id: "opaque-ID".into(), explicitly_enabled: true, output_dir: std::env::temp_dir().join("parent-owned/new-child") } }
fn event(event: Event) -> EventV1 { EventV1 { version: 1, nonce: NONCE.into(), kind: TrackKind::Microphone, event } }
fn owner(t: Instant) -> Owner { Owner::new(ms(t, 8000), NONCE.into(), TrackKind::Microphone).unwrap() }
fn running(t: Instant) -> Owner { let mut o=owner(t);o.assigned(t).unwrap();o.message(t,stamp(0), &event(Event::Ready { format: format() })).unwrap();o }
fn summary() -> TerminalSummary { TerminalSummary { accepted_frames: 0, written_complete_frames: 0, synchronized_frames: None, usable: None, retained_partial: true, cause: "stopped empty".into(), evidence_present: false } }
fn raw(value: &serde_json::Value) -> Vec<u8> { let mut v=serde_json::to_vec(value).unwrap();v.push(b'\n');v }
fn send(o: &mut Owner, t: Instant, qpc: u64, e: Event) -> ProtocolResult<()> { o.message(t,stamp(qpc),&event(e)) }

// Catch bypassed explicit selection, opaque-ID truncation, loose JSON shapes and pre-spawn frame overflow.
#[test]
fn init_requires_explicit_selection_nonce_and_exact_shape() {
    let init=InitV1 { version: 1, nonce: NONCE.into(), request: request() };
    let encoded=encode_init(&init).unwrap();
    assert_eq!(read_init(&encoded[..],NONCE).unwrap().request(), &init.request);
    assert_eq!(decode_init_frame(&encoded, "ffffffff-ffff-ffff-ffff-ffffffffffff").unwrap_err(), ProtocolError::Nonce);
    for (path,value) in [ ("version",serde_json::json!(2)), ("nonce",serde_json::json!("")), ("extra",serde_json::json!(1)) ] {
        let mut v=serde_json::to_value(&init).unwrap();v[path]=value;assert!(decode_init_frame(&raw(&v), NONCE).is_err());
    }
    for (field,value) in [ ("explicitly_enabled",serde_json::json!(false)), ("endpoint_id",serde_json::json!("")), ("endpoint_id",serde_json::json!("bad\u{0000}id")), ("endpoint_id",serde_json::json!("x".repeat(2049))), ("friendly_name",serde_json::json!("default")), ("output_dir",serde_json::json!("relative")) ] {
        let mut v=serde_json::to_value(&init).unwrap();v["request"][field]=value;assert!(decode_init_frame(&raw(&v),NONCE).is_err());
    }
    let mut missing=serde_json::to_value(&init).unwrap();missing["request"].as_object_mut().unwrap().remove("explicitly_enabled");assert!(decode_init_frame(&raw(&missing),NONCE).is_err());
    let mut long=init.clone();long.request.endpoint_id="é".repeat(1024);assert!(encode_init(&long).is_ok());long.request.endpoint_id.push('é');assert_eq!(encode_init(&long).unwrap_err(),ProtocolError::Selection);
    long=init.clone();long.request.endpoint_id="\\".repeat(2048);assert_eq!(encode_init(&long).unwrap_err(),ProtocolError::Oversize);
    let mut bad_path=request();bad_path.output_dir=PathBuf::from("relative");assert_eq!(bad_path.validate(),Err(ProtocolError::OutputDirectory));
    assert!(validate_requests(&[]).is_ok());assert_eq!(validate_requests(&[request(),request()]),Err(ProtocolError::DuplicateKind));
    let mut sys=request();sys.kind=TrackKind::System;assert!(validate_requests(&[request(),sys.clone()]).is_ok());assert!(validate_requests(&[request(),sys.clone(),sys]).is_err());
}

// Catch loose envelopes, duplicates, delayed Ready, oversized total frames and truncated frames.
#[test]
fn events_reject_duplicate_late_wrong_nonce_and_oversize() {
    let ready=event(Event::Ready { format: format() });let encoded=encode_event(&ready).unwrap();
    assert_eq!(read_event(&encoded[..],NONCE,TrackKind::Microphone).unwrap(),ready);
    assert_eq!(decode_event_frame(&encoded,NONCE,TrackKind::System),Err(ProtocolError::Kind));
    assert_eq!(decode_event_frame(&encoded,"ffffffff-ffff-ffff-ffff-ffffffffffff",TrackKind::Microphone),Err(ProtocolError::Nonce));
    let mut v=serde_json::to_value(&ready).unwrap();v["event"]["format"]["extra"]=1.into();assert!(decode_event_frame(&raw(&v),NONCE,TrackKind::Microphone).is_err());
    let mut padded=encoded.clone();padded.pop();padded.resize(4095,b' ');padded.push(b'\n');assert!(decode_event_frame(&padded,NONCE,TrackKind::Microphone).is_ok());assert!(read_event(&padded[..],NONCE,TrackKind::Microphone).is_ok());
    padded.insert(4095,b' ');assert_eq!(decode_event_frame(&padded,NONCE,TrackKind::Microphone),Err(ProtocolError::Oversize));assert_eq!(read_event(&padded[..],NONCE,TrackKind::Microphone),Err(ProtocolError::Oversize));
    assert_eq!(decode_event_frame(&encoded[..encoded.len()-1],NONCE,TrackKind::Microphone),Err(ProtocolError::Truncated));assert_eq!(read_event(&encoded[..encoded.len()-1],NONCE,TrackKind::Microphone),Err(ProtocolError::Truncated));
    let mut multiple=encoded.clone();multiple.extend_from_slice(&encoded);assert!(decode_event_frame(&multiple,NONCE,TrackKind::Microphone).is_err());
    let t=Instant::now();let mut o=running(t);assert_eq!(o.message(t,stamp(0),&ready),Err(ProtocolError::Order));
    let mut o=owner(t);o.assigned(t).unwrap();assert_eq!(o.message(ms(t,8000),stamp(8000),&ready),Err(ProtocolError::Deadline));
    let mut ending=event(Event::Ending { cause:"x".repeat(1025),observed:stamp(0) });assert_eq!(encode_event(&ending),Err(ProtocolError::Reason));ending.event=Event::Ending { cause:"x".repeat(1024),observed:stamp(0) };assert!(encode_event(&ending).is_ok());
}

// Catch fake StopAck requirements, spontaneous-ending timeout renewal and the Stop/Ending race.
#[test]
fn spontaneous_startup_runtime_end_and_stop_race() {
    let t=Instant::now();
    for startup in [true,false] {
        let mut o=if startup { let mut x=owner(t);x.assigned(t).unwrap();x } else { running(t) };
        send(&mut o,ms(t,100),100,Event::Ending { cause:"native failure".into(),observed:stamp(100) }).unwrap();assert_eq!(o.state,State::Stopping);
        o.stop(ms(t,500)).unwrap();assert_eq!(send(&mut o,ms(t,500),500,Event::StopAck { observed:stamp(500) }),Err(ProtocolError::Order));
        assert_eq!(send(&mut o,ms(t,500),500,Event::Ready { format:format() }),Err(ProtocolError::Order));
        send(&mut o,ms(t,600),600,Event::CaptureReleased { at:stamp(600) }).unwrap();assert_eq!(o.state,State::StorageFinalizing);
        send(&mut o,ms(t,601),601,Event::Terminal { outcome:summary() }).unwrap();assert_eq!(o.state,State::AwaitingExit);
    }
    let mut o=running(t);o.stop(ms(t,20)).unwrap();send(&mut o,ms(t,21),21,Event::Ending { cause:"race".into(),observed:stamp(21) }).unwrap();assert_eq!(o.tick(ms(t,3020),false).unwrap(),Tick::Force);
    let mut o=running(t);o.stop(t).unwrap();send(&mut o,t,0,Event::StopAck { observed:stamp(0) }).unwrap();assert_eq!(send(&mut o,t,0,Event::Ending { cause:"duplicate".into(),observed:stamp(0) }),Err(ProtocolError::Order));
    let mut o=owner(t);o.assigned(t).unwrap();send(&mut o,ms(t,7900),7900,Event::Ending { cause:"late startup failure".into(),observed:stamp(7900) }).unwrap();assert_eq!(o.tick(ms(t,7999),false).unwrap(),Tick::None);assert_eq!(o.tick(ms(t,8000),false).unwrap(),Tick::Force);
}

// Catch renewed admission/native/forced deadlines and duplicate containment effects.
#[test]
fn deadlines_are_first_observation_only() {
    let t=Instant::now();let mut o=owner(t);o.assigned(ms(t,7999)).unwrap();assert_eq!(o.tick(ms(t,7999),false).unwrap(),Tick::None);assert_eq!(o.tick(ms(t,8000),false).unwrap(),Tick::Force);assert_eq!(o.tick(ms(t,9999),false).unwrap(),Tick::None);o.stop(ms(t,9999)).unwrap();assert_eq!(o.cancel_storage(ms(t,9999)).unwrap(),Tick::None);assert_eq!(o.tick(ms(t,10000),false).unwrap(),Tick::RetainPending);assert_eq!(o.tick(ms(t,20000),false).unwrap(),Tick::None);assert_eq!(o.state,State::CleanupPending);o.tick(ms(t,20001),true).unwrap();assert_eq!(o.state,State::Exited);
    let mut o=running(t);assert_eq!(o.tick(ms(t,900000),false).unwrap(),Tick::None);o.stop(ms(t,900001)).unwrap();o.stop(ms(t,902999)).unwrap();assert_eq!(o.tick(ms(t,903000),false).unwrap(),Tick::None);assert_eq!(o.tick(ms(t,903001),false).unwrap(),Tick::Force);
    let mut o=running(t);o.stop(ms(t,10)).unwrap();send(&mut o,ms(t,3009),3009,Event::StopAck { observed:stamp(3009) }).unwrap();assert_eq!(o.tick(ms(t,3010),false).unwrap(),Tick::Force);
}

// Catch wrong/future clocks and converting elapsed recovery against every poll instead of once.
#[test]
fn recovery_deadline_uses_checked_shared_qpc() {
    let t=Instant::now();
    for bad in [ ClockStamp { qpc:0,frequency:0 },ClockStamp { qpc:0,frequency:999 },stamp(5001) ] {
        let mut o=running(t);assert_eq!(send(&mut o,ms(t,5000),5000,Event::RecoveryStarted { episode:1,first_failure:bad }),Err(ProtocolError::Clock));assert_eq!(o.state,State::Running);
    }
    let mut o=running(t);assert_eq!(send(&mut o,ms(t,5000),5000,Event::RecoveryStarted { episode:1,first_failure:stamp(0) }),Err(ProtocolError::Deadline));
    let mut o=running(t);send(&mut o,ms(t,2000),2000,Event::RecoveryStarted { episode:1,first_failure:stamp(1000) }).unwrap();assert_eq!(o.state,State::Recovering);assert_eq!(o.tick(ms(t,5999),false).unwrap(),Tick::None);assert_eq!(o.tick(ms(t,6000),false).unwrap(),Tick::Force);
    let mut o=running(t);send(&mut o,ms(t,2000),2000,Event::RecoveryStarted { episode:1,first_failure:stamp(1000) }).unwrap();o.stop(ms(t,5500)).unwrap();send(&mut o,ms(t,5501),5501,Event::StopAck { observed:stamp(5501) }).unwrap();assert_eq!(o.tick(ms(t,6000),false).unwrap(),Tick::Force);
    let mut o=running(t);send(&mut o,t,100,Event::RecoveryStarted { episode:1,first_failure:stamp(100) }).unwrap();assert_eq!(send(&mut o,t,100,Event::Recovered { episode:2,generation:1 }),Err(ProtocolError::Order));send(&mut o,t,100,Event::Recovered { episode:1,generation:1 }).unwrap();assert_eq!(o.state,State::Running);assert_eq!(o.tick(ms(t,10000),false).unwrap(),Tick::None);assert!(send(&mut o,ms(t,10000),10100,Event::RecoveryStarted { episode:1,first_failure:stamp(10100) }).is_err());send(&mut o,ms(t,10000),10100,Event::RecoveryStarted { episode:2,first_failure:stamp(10100) }).unwrap();
}

// Catch treating capture release as process completion, renewed storage budgets and late summaries.
#[test]
fn capture_release_starts_one_storage_deadline() {
    let t=Instant::now();let mut o=running(t);o.stop(t).unwrap();send(&mut o,ms(t,100),100,Event::StopAck { observed:stamp(100) }).unwrap();send(&mut o,ms(t,1000),1000,Event::CaptureReleased { at:stamp(500) }).unwrap();assert_eq!(o.state,State::StorageFinalizing);assert!(o.terminal().is_none());
    o.stop(ms(t,110000)).unwrap();assert_eq!(send(&mut o,ms(t,110000),110000,Event::CaptureReleased { at:stamp(110000) }),Err(ProtocolError::Order));assert_eq!(o.tick(ms(t,120499),false).unwrap(),Tick::None);assert_eq!(send(&mut o,ms(t,120500),120500,Event::Terminal { outcome:summary() }),Err(ProtocolError::Deadline));assert_eq!(o.tick(ms(t,120500),false).unwrap(),Tick::Force);assert_eq!(o.tick(ms(t,122500),false).unwrap(),Tick::RetainPending);
    let mut o=running(t);o.stop(t).unwrap();send(&mut o,t,0,Event::StopAck { observed:stamp(0) }).unwrap();send(&mut o,t,0,Event::CaptureReleased { at:stamp(0) }).unwrap();send(&mut o,ms(t,119999),119999,Event::Terminal { outcome:summary() }).unwrap();assert!(o.terminal().is_some());assert_eq!(o.state,State::AwaitingExit);assert_eq!(o.tick(ms(t,120000),false).unwrap(),Tick::Force);assert!(o.terminal().is_none());
    let mut o=running(t);o.stop(t).unwrap();send(&mut o,t,0,Event::StopAck { observed:stamp(0) }).unwrap();assert_eq!(send(&mut o,ms(t,3000),3000,Event::CaptureReleased { at:stamp(2999) }),Err(ProtocolError::Deadline));
    let mut o=running(t);o.stop(t).unwrap();send(&mut o,t,50,Event::StopAck { observed:stamp(50) }).unwrap();for at in [stamp(49),stamp(101),ClockStamp { qpc:50,frequency:0 },ClockStamp { qpc:50,frequency:999 }] { assert_eq!(send(&mut o,t,100,Event::CaptureReleased { at }),Err(ProtocolError::Clock)); }
}

// Catch Terminal bypassing StopAck/Ending, release ordering, or cancellation containment.
#[test]
fn terminal_before_release_is_rejected() {
    let t=Instant::now();let mut o=running(t);assert_eq!(send(&mut o,t,0,Event::Terminal { outcome:summary() }),Err(ProtocolError::Order));o.stop(t).unwrap();assert_eq!(send(&mut o,t,0,Event::CaptureReleased { at:stamp(0) }),Err(ProtocolError::Order));send(&mut o,t,0,Event::StopAck { observed:stamp(0) }).unwrap();assert_eq!(send(&mut o,t,0,Event::Terminal { outcome:summary() }),Err(ProtocolError::Order));assert_eq!(send(&mut o,t,0,Event::StopAck { observed:stamp(0) }),Err(ProtocolError::Order));send(&mut o,t,0,Event::CaptureReleased { at:stamp(0) }).unwrap();send(&mut o,t,0,Event::Terminal { outcome:summary() }).unwrap();assert_eq!(send(&mut o,t,0,Event::Terminal { outcome:summary() }),Err(ProtocolError::Order));
    assert_eq!(o.protocol_failure(ms(t,1)).unwrap(),Tick::Force);assert!(o.terminal().is_none());assert_eq!(send(&mut o,ms(t,1),1,Event::Terminal { outcome:summary() }),Err(ProtocolError::Order));assert_eq!(o.cancel_storage(ms(t,2000)).unwrap(),Tick::None);assert_eq!(o.tick(ms(t,2001),false).unwrap(),Tick::RetainPending);
    let mut o=running(t);assert_eq!(o.protocol_failure(t).unwrap(),Tick::None);assert_eq!(o.state,State::Stopping);assert_eq!(send(&mut o,t,0,Event::StopAck { observed:stamp(0) }),Err(ProtocolError::Order));o.stop(t).unwrap();send(&mut o,t,0,Event::StopAck { observed:stamp(0) }).unwrap();assert_eq!(send(&mut o,t,0,Event::CaptureReleased { at:stamp(0) }),Err(ProtocolError::Order));assert_eq!(send(&mut o,t,0,Event::Terminal { outcome:summary() }),Err(ProtocolError::Order));
}

// Catch valid trailing frames replacing a failed owner's original containment deadline.
#[test]
fn protocol_failure_cannot_promote_capture_release_to_storage() {
    let t = Instant::now();
    for (stop_first, deadline_ms) in [(false, 3100), (true, 3000)] {
        for spontaneous in [false, true] {
            let mut o = running(t);
            if stop_first { o.stop(t).unwrap(); }
            assert_eq!(o.protocol_failure(ms(t, 100)).unwrap(), Tick::None);
            o.stop(ms(t, 100)).unwrap();
            let ending = if spontaneous {
                Event::Ending { cause: "trailing native failure".into(), observed: stamp(100) }
            } else { Event::StopAck { observed: stamp(100) } };
            send(&mut o, ms(t, 100), 100, ending).unwrap();
            let release = send(&mut o, ms(t, 101), 101, Event::CaptureReleased { at: stamp(101) });
            if release.is_err() {
                assert_eq!(o.protocol_failure(ms(t, 101)).unwrap(), Tick::None);
                o.stop(ms(t, 101)).unwrap();
            }
            o.stop(ms(t, deadline_ms - 1)).unwrap();
            assert_eq!(o.tick(ms(t, deadline_ms - 1), false).unwrap(), Tick::None);
            assert_eq!(o.tick(ms(t, deadline_ms), false).unwrap(), Tick::Force);
            assert_eq!(release, Err(ProtocolError::Order));
            assert!(o.terminal().is_none());
            assert_eq!(o.tick(ms(t, deadline_ms), false).unwrap(), Tick::None);
            assert_eq!(o.protocol_failure(ms(t, deadline_ms + 1999)).unwrap(), Tick::None);
            o.stop(ms(t, deadline_ms + 1999)).unwrap();
            assert_eq!(o.cancel_storage(ms(t, deadline_ms + 1999)).unwrap(), Tick::None);
            assert_eq!(o.tick(ms(t, deadline_ms + 1999), false).unwrap(), Tick::None);
            assert_eq!(o.tick(ms(t, deadline_ms + 2000), false).unwrap(), Tick::RetainPending);
            assert_eq!(o.state, State::CleanupPending);
            assert_eq!(o.tick(ms(t, deadline_ms + 2001), false).unwrap(), Tick::None);
            o.tick(ms(t, deadline_ms + 2002), true).unwrap();
            assert_eq!(o.state, State::Exited);
            assert!(o.terminal().is_none());
        }
    }
}

#[test]
fn valid_producer_failure_retains_prefix_during_storage_grace() {
    let t = Instant::now();
    let mut o = running(t);
    send(&mut o, t, 0, Event::Ending { cause: "native capture failure".into(), observed: stamp(0) }).unwrap();
    send(&mut o, ms(t, 1), 1, Event::CaptureReleased { at: stamp(1) }).unwrap();
    assert_eq!(o.tick(ms(t, 3000), false).unwrap(), Tick::None);
    assert_eq!(o.state, State::StorageFinalizing);
    let mut prefix = summary();
    prefix.accepted_frames = 1;
    prefix.written_complete_frames = 1;
    prefix.synchronized_frames = Some(1);
    prefix.usable = Some(WavInventory { frames: 1, rate: 48000, channels: 2, bits: 16, encoding: Encoding::Pcm, file_bytes: 48 });
    prefix.cause = "native capture failure".into();
    send(&mut o, ms(t, 4000), 4000, Event::Terminal { outcome: prefix.clone() }).unwrap();
    o.tick(ms(t, 4001), true).unwrap();
    assert_eq!(o.terminal(), Some(&prefix));
}

#[test]
fn untrusted_terminal_inventory_cannot_claim_undrained_or_unsynchronized_audio() {
    let mut s=summary();s.accepted_frames=1;s.written_complete_frames=1;s.synchronized_frames=Some(1);s.usable=Some(WavInventory { frames:1,rate:48000,channels:2,bits:16,encoding:Encoding::Pcm,file_bytes:48 });
    assert!(encode_event(&event(Event::Terminal { outcome:s.clone() })).is_ok());
    for n in 0..5 { let mut bad=s.clone();match n { 0=>bad.written_complete_frames=0,1=>bad.synchronized_frames=None,2=>bad.usable.as_mut().unwrap().frames=0,3=>bad.usable.as_mut().unwrap().file_bytes=44,_=>bad.usable.as_mut().unwrap().channels=3 };assert_eq!(encode_event(&event(Event::Terminal { outcome:bad })),Err(ProtocolError::Inventory)); }
    let mut v=serde_json::to_value(event(Event::Terminal { outcome:s })).unwrap();v["event"]["outcome"]["path"]="replace.wav".into();assert!(decode_event_frame(&raw(&v),NONCE,TrackKind::Microphone).is_err());
}

#[test]
fn observed_exit_at_storage_deadline_cannot_qualify_a_prior_terminal() {
    let t=Instant::now();
    for (exit_ms,qualified) in [(119999,true),(120000,false)] {
        let mut o=running(t);o.stop(t).unwrap();send(&mut o,t,0,Event::StopAck { observed:stamp(0) }).unwrap();send(&mut o,t,0,Event::CaptureReleased { at:stamp(0) }).unwrap();send(&mut o,ms(t,100),100,Event::Terminal { outcome:summary() }).unwrap();
        assert_eq!(o.tick(ms(t,exit_ms),true).unwrap(),Tick::None);assert_eq!(o.state,State::Exited);assert_eq!(o.terminal().is_some(),qualified);
    }
}
#[test]
fn optional_inventory_fields_still_require_exact_wire_shape() {
    for field in ["synchronized_frames","usable"] {
        let mut v=serde_json::to_value(event(Event::Terminal { outcome:summary() })).unwrap();v["event"]["outcome"].as_object_mut().unwrap().remove(field);
        assert!(decode_event_frame(&raw(&v),NONCE,TrackKind::Microphone).is_err(),"missing field {field} must fail exact shape");
    }
}
#[test]
fn gated_worker_can_report_truthful_ending_without_ready() {
    let t=Instant::now();let mut o=owner(t);
    send(&mut o,t,0,Event::Ending { cause:"gate closed before native startup".into(),observed:stamp(0) }).unwrap();
    send(&mut o,t,0,Event::CaptureReleased { at:stamp(0) }).unwrap();
    send(&mut o,t,0,Event::Terminal { outcome:summary() }).unwrap();
    assert_eq!(o.state,State::AwaitingExit);
}
#[test]
fn kind_and_encoding_use_only_the_exact_string_wire_shape() {
    let init=InitV1 { version:1,nonce:NONCE.into(),request:request() };
    let mut v=serde_json::to_value(init).unwrap();v["request"]["kind"]=serde_json::json!({"Microphone":null});assert!(decode_init_frame(&raw(&v),NONCE).is_err());
    let mut v=serde_json::to_value(event(Event::Ready { format:format() })).unwrap();v["event"]["format"]["encoding"]=serde_json::json!({"Pcm":null});assert!(decode_event_frame(&raw(&v),NONCE,TrackKind::Microphone).is_err());
}

#[test]
fn private_proof_has_no_runtime_generated_approval() {
    assert!(focusflow_windows_audio_worker::proof::guard::compiled_approval().is_none());
    for mode in ["--private-audio-proof","--private-audio-render"] {
        assert_eq!(focusflow_windows_audio_worker::proof::dispatch(["private.exe",mode].into_iter().map(Into::into)),77);
    }
}

#[test]
fn private_parent_and_renderer_dispatch_reject_forged_environment_approval() {
    for mode in ["--private-audio-proof", "--private-audio-render"] {
        let result=std::process::Command::new(env!("CARGO_BIN_EXE_private_audio_proof"))
            .env_clear().env("GITHUB_ACTIONS","true").env("RUNNER_ENVIRONMENT","github-hosted").env("RUNNER_OS","Windows")
            .env("GITHUB_REPOSITORY","owner/repo").env("GITHUB_REF","refs/heads/proof")
            .env("GITHUB_WORKFLOW_REF","owner/repo/.github/workflows/proof.yml@refs/heads/proof")
            .env("GITHUB_WORKFLOW_SHA","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .env("GITHUB_SHA","bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            .env("GITHUB_RUN_ID","123456").env("GITHUB_RUN_ATTEMPT","1")
            .env("NATIVE_GO","true").env("APPROVED_PROOF",r#"{"nativeGo":true,"approved":true}"#)
            .arg(mode).args(["forged-id","forged-output","forged-nonce"]).output().unwrap();
        assert_eq!(result.status.code(),Some(77));
        assert!(String::from_utf8(result.stderr).unwrap().contains("Native-GO is absent"));
        assert!(result.stdout.is_empty());
    }
}
