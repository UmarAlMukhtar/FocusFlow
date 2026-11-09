mod common;

use common::{fixture_project, TestDir};
use focusflow_core::{
    capture::audio::{decode, publish_finalized_assets, publish_incomplete},
    edits::Edit,
    ffmpeg::{PacketTime, ProbedVideo, ProbedVideoEvidence},
    legacy::LegacyDefaults,
    project::{AudioAsset, CursorMode, EditableState, ProjectWarning, MAX_JSON_BYTES},
    store::ProjectStore,
    ErrorCode,
};
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}};

const MARKER: &str = ".focusflow-audio-v4-intent";
const SYSTEM: &str = "audio/system/system.wav";
const EVIDENCE: &str = "audio/system/system.evidence.partial.json";

fn evidence() -> ProbedVideoEvidence {
    ProbedVideoEvidence {
        video: ProbedVideo { width: 100, height: 50, duration_us: 1_000_000 },
        endpoint: PacketTime { ticks: 30, numerator: 1, denominator: 30 },
        first_packet_duration: PacketTime { ticks: 1, numerator: 1, denominator: 30 },
    }
}

fn origin() -> Value {
    json!({"policy":"discardWarmupUntilCommit","sampleVideoRelation":"unknown",
        "selectionEpoch":"a".repeat(32),"admissionId":"b".repeat(32),
        "parentSendNs":1000,"childCommitBeforeNs":1100,"childCommitAfterNs":1101})
}

fn system_evidence() -> Value {
    let first = json!({"kind":"monitor","generation":0,"packet":0,"sourceStartFrame":0,
        "frameCount":3,"callbackBeforeNs":"1102","callbackAfterNs":"1103"});
    let latest = json!({"kind":"monitor","generation":0,"packet":1,"sourceStartFrame":3,
        "frameCount":5,"callbackBeforeNs":"1200","callbackAfterNs":"1201"});
    json!({
        "timing":"unknown","sampleVideoRelation":"unknown","domain":0,"writtenFrames":8,
        "terminalReason":"stopped","packets":2,"first":first,"latest":latest,"landmarks":[],
        "selection":{"socket_path":"/run/user/1000/pipewire-0","peer_uid":1000,"peer_pid":101,
            "cookie":102,"node_id":5,"node_serial":20,"selection_epoch":"a".repeat(32),
            "ports":[{"id":6,"serial":21,"channel":"FL"},{"id":7,"serial":22,"channel":"FR"}]},
        "origin":{"policy":"discardWarmupUntilCommit","timing":"unknown","sampleVideoRelation":"unknown",
            "admission":{"selection_epoch":"a".repeat(32),"admission_id":"b".repeat(32)},
            "parentSendNs":1000,"childCommitObservationNs":[1100,1101],
            "warmupPackets":1,"warmupFrames":64,"firstRetainedPacket":first}
    })
}

fn video(finalized: bool) -> Value {
    let mut doc = fixture_project();
    doc.schema_version = 3;
    doc.video.cursor = CursorMode::Absent;
    doc.video.duration_us = 1_000_000;
    doc.clips[0].out_us = 1_000_000;
    doc.microphone = Some(AudioAsset { path: "mic.wav".into() });
    doc.warnings = vec![ProjectWarning::LegacyTimingUnknown, ProjectWarning::CursorCaptureIncomplete];
    doc.clicks.push(focusflow_core::project::Click { at_us: 100_000, x: 0.5, y: 0.5 });
    let state = if finalized {
        json!({"status":"finalized","videoTiming":{"endpoint":{"provenance":"savedPacketFirstAndTail",
            "endPts":30,"timeBase":{"numerator":1,"denominator":30},"interval":"halfOpen"},"clock":null},
            "cursorTiming":null,"initialProject":doc})
    } else {
        json!({"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"})
    };
    json!({"schemaVersion":1,"captureId":"fixture",
        "source":{"backend":"linuxX11","kind":"region","initialBounds":{"x":40,"y":60,"width":100,"height":50},"windowId":null},
        "diagnostic":finalized.then(|| json!({"stage":"clock","message":"Clock rejected; cursor absent."})), "state":state})
}

fn envelope(finalized: bool) -> Value {
    let video = video(finalized);
    let mut state = json!({"status":if finalized {"finalized"} else {"incomplete"},
        "videoAuthority":{"type":"linuxV1","manifest":video},"audioRequested":{"microphone":true,"system":true}});
    if finalized {
        state["audio"] = json!({
            "microphone":{"status":"validated","asset":{"path":"mic.wav","producer":"cpalMicrophoneV1","renderPolicy":"legacyZero",
                "timing":{"type":"unknown"},"inventory":{"sampleRate":48000,"channels":1,"encoding":"pcmInteger","bitsPerSample":16,"frameCount":8,"fileBytes":60}}},
            "system":{"status":"validated","asset":{"path":SYSTEM,"producer":"linuxSystemV1","renderPolicy":"withheldUnverified",
                "timing":{"type":"unknown"},"inventory":{"sampleRate":48000,"channels":2,"encoding":"ieeeFloat","bitsPerSample":32,"frameCount":8,"fileBytes":132},
                "evidencePath":EVIDENCE,"origin":origin()}}});
        let mut doc = video["state"]["initialProject"].clone();
        doc["schemaVersion"] = json!(5);
        doc.as_object_mut().unwrap().remove("microphone");
        doc.as_object_mut().unwrap().remove("timing");
        doc["audioTracks"] = json!({"microphone":{"path":"mic.wav","sampleRate":48000,"channels":1,"frameCount":8,
            "timing":{"type":"unknown"}},"system":null});
        doc["warnings"] = json!(["cursorCaptureIncomplete","microphoneTimingUnknown","systemTimingUnknown"]);
        state["initialProject"] = doc;
    }
    json!({"schemaVersion":4,"captureId":"fixture","state":state})
}

fn parsed(value: &Value) -> focusflow_core::capture::audio::CaptureEnvelopeV4 {
    decode(&serde_json::to_vec(value).unwrap(), "fixture").unwrap()
}

struct Session { _temp: TestDir, dir: PathBuf, store: ProjectStore }
impl Session {
    fn new() -> Self {
        let temp = TestDir::new();
        let dir = temp.path().join("fixture");
        fs::create_dir(&dir).unwrap();
        let store = ProjectStore::new(temp.path()).unwrap();
        Self { _temp: temp, dir, store }
    }
    fn arm(&self) { publish_incomplete(&self.dir, &parsed(&envelope(false))).unwrap(); }
    fn assets(&self) {
        fs::write(self.dir.join("screen.mp4"), b"fixture video: exact packet evidence supplied").unwrap();
        fs::create_dir_all(self.dir.join("audio/system")).unwrap();
        let mut mic = hound::WavWriter::create(self.dir.join("mic.wav"), hound::WavSpec {
            channels:1, sample_rate:48000, bits_per_sample:16, sample_format:hound::SampleFormat::Int
        }).unwrap();
        for value in 0..8i16 { mic.write_sample(value).unwrap(); }
        mic.finalize().unwrap();
        let mut system = hound::WavWriter::create(self.dir.join(SYSTEM), hound::WavSpec {
            channels:2, sample_rate:48000, bits_per_sample:32, sample_format:hound::SampleFormat::Float
        }).unwrap();
        for _ in 0..16 { system.write_sample(0.2f32).unwrap(); }
        system.finalize().unwrap();
        self.write(EVIDENCE, &system_evidence());
    }
    fn write(&self, name: &str, value: &Value) { fs::write(self.dir.join(name), serde_json::to_vec(value).unwrap()).unwrap(); }
    fn finish(&self) -> focusflow_core::capture::audio::CaptureEnvelopeV4 {
        publish_finalized_assets(&self.dir, &parsed(&envelope(true)), &evidence()).unwrap()
    }
    fn capture_bytes(&self) -> Vec<u8> { fs::read(self.dir.join("capture.json")).unwrap() }
}

#[test]
fn strict_decode_checks_complete_canonical_snapshot_and_nested_v1() {
    let good = envelope(true);
    let decoded = parsed(&good);
    assert_eq!(serde_json::to_value(decoded).unwrap(), good);
    for (path, value) in [
        ("/schemaVersion",json!(5)),
        ("/captureId",json!("other")),
        ("/state/videoAuthority/type",json!("windowsV2")),
        ("/state/videoAuthority/manifest/captureId",json!("other")),
        ("/state/videoAuthority/manifest/schemaVersion",json!(4)),
        ("/state/videoAuthority/manifest/state", video(false)["state"].clone()),
        ("/state/audioRequested/system",json!(false)),
        ("/state/audio/system/asset/path",json!("../system.wav")),
        ("/state/audio/system/asset/producer",json!("cpalMicrophoneV1")),
        ("/state/audio/system/asset/producer",json!({"linuxSystemV1":null})),
        ("/state/audio/system/asset/renderPolicy",json!("provisionalZero")),
        ("/state/audio/system/asset/timing",json!({"type":"unknown","offsetUs":0})),
        ("/state/audio/system/asset/timing",json!({"type":"estimated","offsetUs":0,"uncertaintyUs":0})),
        ("/state/audio/system/asset/inventory/fileBytes",json!(131)),
        ("/state/audio/system/asset/origin/admissionId",json!("other")),
        ("/state/audio/system/asset/origin/childCommitBeforeNs",json!(999)),
        ("/state/audio/system/asset/evidencePath",json!("other.json")),
        ("/state/initialProject/revision",json!(1)),
        ("/state/initialProject/video/durationUs",json!(2_000_000)),
        ("/state/initialProject/warnings",json!(["cursorCaptureIncomplete","microphoneTimingUnknown"])),
        ("/state/initialProject/audioTracks/system",good["state"]["initialProject"]["audioTracks"]["microphone"].clone()),
        ("/state/initialProject/warnings",json!([{"cursorCaptureIncomplete":null},"microphoneTimingUnknown","systemTimingUnknown"])),
    ] {
        let mut mutated = good.clone();
        *mutated.pointer_mut(path).unwrap() = value;
        assert!(decode(&serde_json::to_vec(&mutated).unwrap(),"fixture").is_err(), "accepted {path}");
    }
    for path in ["/state/audio", "/state/audio/system", "/state/videoAuthority/manifest", "/state/initialProject/audioTracks/microphone"] {
        let mut mutated = good.clone();
        let (parent,key) = path.rsplit_once('/').unwrap();
        mutated.pointer_mut(parent).unwrap().as_object_mut().unwrap().remove(key);
        assert!(decode(&serde_json::to_vec(&mutated).unwrap(),"fixture").is_err(),"accepted missing {path}");
    }
    let raw = serde_json::to_string(&good).unwrap().replace("\"fileBytes\":132", "\"fileBytes\":132,\"fileBytes\":132");
    assert!(decode(raw.as_bytes(),"fixture").is_err());
    let mut positional = good;
    positional["state"]["audioRequested"] = json!([true,true]);
    assert!(decode(&serde_json::to_vec(&positional).unwrap(),"fixture").is_err());
    assert!(decode(&vec![b' ';MAX_JSON_BYTES as usize+1],"fixture").is_err());
}

#[test]
fn publication_is_monotonic_and_reuses_checked_video_authority() {
    let s = Session::new();
    s.assets();
    assert!(publish_finalized_assets(&s.dir,&parsed(&envelope(true)),&evidence()).is_err());
    s.arm();
    let incomplete = s.capture_bytes();
    assert!(publish_incomplete(&s.dir,&parsed(&envelope(false))).is_err());
    let mut bad = evidence(); bad.endpoint.ticks = 31;
    assert!(publish_finalized_assets(&s.dir,&parsed(&envelope(true)),&bad).is_err());
    let mut mismatched = envelope(true);
    mismatched["state"]["videoAuthority"]["manifest"]["source"]["initialBounds"]["x"] = json!(41);
    assert!(publish_finalized_assets(&s.dir,&parsed(&mismatched),&evidence()).is_err());
    assert_eq!(s.capture_bytes(),incomplete);
    let committed = s.finish();
    assert_eq!(serde_json::to_value(committed).unwrap(),envelope(true));
    let final_bytes = s.capture_bytes();
    assert!(publish_finalized_assets(&s.dir,&parsed(&envelope(true)),&evidence()).is_err());
    assert_eq!(s.capture_bytes(),final_bytes);
    assert!(!s.dir.join("project.json").exists());
    assert!(s.dir.join(MARKER).is_file());
    assert!(s.store.list_projects(None,10).unwrap().projects[0].needs_import);
}

#[test]
fn invalid_optional_wav_or_origin_excludes_only_the_failed_record() {
    for damage in ["mic","system","origin","nonfinite-system","missing-evidence"] {
        let s = Session::new();
        s.arm();
        s.assets();
        match damage {
            "mic" => fs::write(s.dir.join("mic.wav"), b"truncated").unwrap(),
            "system" => fs::write(s.dir.join(SYSTEM), b"truncated").unwrap(),
            "origin" => {
                let mut mismatched = system_evidence();
                mismatched["origin"]["admission"]["admission_id"] = json!("c".repeat(32));
                s.write(EVIDENCE,&mismatched);
            },
            "nonfinite-system" => {
                let mut wav = fs::read(s.dir.join(SYSTEM)).unwrap();
                wav[68..72].copy_from_slice(&f32::NAN.to_le_bytes());
                fs::write(s.dir.join(SYSTEM),wav).unwrap();
            },
            _ => fs::remove_file(s.dir.join(EVIDENCE)).unwrap(),
        }
        let protected = [s.dir.join("mic.wav"),s.dir.join(SYSTEM)].map(|p|fs::read(p).unwrap());
        let committed = s.finish();
        let wire = serde_json::to_value(&committed).unwrap();
        let doc = committed.finalized_project().unwrap();
        let tracks = doc.audio_tracks.as_ref().unwrap();
        assert!(tracks.system.is_none());
        assert_eq!(tracks.microphone.is_none(),damage=="mic");
        let (invalid, healthy, warning) = if damage=="mic" {
            ("microphone","system",ProjectWarning::InvalidMicrophoneAudio)
        } else {
            ("system","microphone",ProjectWarning::InvalidSystemAudio)
        };
        assert_eq!(wire["state"]["audio"][invalid]["status"],"unavailable");
        assert_eq!(wire["state"]["audio"][healthy]["status"],"validated");
        assert!(doc.warnings.contains(&warning));
        assert_eq!(wire["state"]["videoAuthority"]["manifest"],video(true));
        assert_eq!([s.dir.join("mic.wav"),s.dir.join(SYSTEM)].map(|p|fs::read(p).unwrap()),protected);
    }
}

#[cfg(unix)]
fn probe(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let tool = dir.join("packet-fixture");
    fs::write(&tool, r#"#!/bin/sh
printf '#format: frame checksums\n#version: 2\n#hash: adler32\n#extradata 0, 44, 11420d96\n#software: Lavf62.12.103\n#tb 0: 1/30\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 100x50\n#sar 0: 0/1\n#stream#, dts, pts, duration, size, hash\n'
printf 'Duration: 00:00:01.00, start: 0\nStream #0:0: Video: h264, yuv420p, 100x50\n' >&2
case " $* " in
  *' -sseof -5 '*) printf '0, 28, 29, 1, 1234, ab12cd34\n';;
  *) printf '0, -2, 0, 1, 1234, ab12cd34\n';;
esac
"#).unwrap();
    fs::set_permissions(&tool,fs::Permissions::from_mode(0o700)).unwrap();
    tool
}

fn defaults() -> LegacyDefaults {
    LegacyDefaults {scale:2.0,intro_us:0,hold_us:0,outro_us:0,pan_transition_us:0,settle_us:0}
}

#[cfg(unix)]
#[test]
fn import_reopen_edit_restore_and_late_audio_failure_preserve_authority() {
    let s = Session::new();
    s.arm();
    s.assets();
    let committed = s.finish();
    let capture = s.capture_bytes();
    let imported = s.store.open_or_import("fixture",&probe(&s.dir),defaults()).unwrap();
    assert_eq!(&imported,committed.finalized_project().unwrap());
    assert_eq!(s.store.snapshot("fixture",0).unwrap().document,imported);
    let state = EditableState::from_document(&imported);
    let edited = s.store.apply_edits("fixture",0,&[Edit::TrimClip {clip_id:"clip-0".into(),in_us:100_000,out_us:900_000}]).unwrap();
    assert_eq!(edited.revision,1);
    assert_eq!(s.store.get("fixture").unwrap(),edited);
    let restored = s.store.restore_edits("fixture",1,&state).unwrap();
    assert_eq!(restored.revision,2);
    let project = fs::read(s.dir.join("project.json")).unwrap();
    fs::write(s.dir.join("mic.wav"),b"corrupt").unwrap();
    fs::remove_file(s.dir.join(SYSTEM)).unwrap();
    let snapshot = s.store.snapshot("fixture",2).unwrap().document;
    assert!(snapshot.audio_tracks.as_ref().unwrap().microphone.is_none());
    assert!(snapshot.audio_tracks.as_ref().unwrap().system.is_none());
    for warning in [ProjectWarning::SystemTimingUnknown,ProjectWarning::InvalidMicrophoneAudio,ProjectWarning::InvalidSystemAudio] {
        assert!(snapshot.warnings.contains(&warning));
    }
    assert_eq!(s.store.get("fixture").unwrap(),restored);
    assert_eq!(fs::read(s.dir.join("project.json")).unwrap(),project);
    assert_eq!(s.capture_bytes(),capture);
}

#[test]
fn persisted_project_cannot_bypass_intent_or_immutable_sources() {
    let s = Session::new();
    let valid = envelope(true)["state"]["initialProject"].clone();
    s.arm();
    s.write("project.json",&valid);
    assert!(s.store.get("fixture").is_err());
    assert!(s.store.open_or_import("fixture",Path::new("unreachable"),defaults()).is_err());
    assert!(s.store.list_projects(None,10).unwrap().projects.is_empty());
    fs::remove_file(s.dir.join("capture.json")).unwrap();
    assert!(s.store.get("fixture").is_err());
    s.write("capture.json",&envelope(true));
    assert!(s.store.get("fixture").is_ok());
    for (path,value) in [
        ("/video/path",json!("other.mp4")),
        ("/audioTracks/microphone/frameCount",json!(9)),
        ("/audioTracks/microphone",Value::Null),
        ("/warnings",json!(["cursorCaptureIncomplete","microphoneTimingUnknown"])),
        ("/camera/panTransitionUs",json!(0)),
        ("/clicks",json!([])),
    ] {
        let mut bad = valid.clone();
        *bad.pointer_mut(path).unwrap() = value;
        s.write("project.json",&bad);
        assert!(s.store.get("fixture").is_err(),"accepted {path}");
        assert!(s.store.apply_edits("fixture",0,&[Edit::TrimClip {clip_id:"clip-0".into(),in_us:0,out_us:900_000}]).is_err());
    }
    s.write("project.json",&valid);
    fs::remove_file(s.dir.join(MARKER)).unwrap();
    assert!(s.store.get("fixture").is_err(),"valid V4 capture without its marker");
    for bytes in [br#"{"schemaVersion":4,"state":"#.as_slice(), br#"{"schemaVersion":1,"schemaVersion":4}"#] {
        fs::write(s.dir.join("capture.json"),bytes).unwrap();
        assert!(s.store.get("fixture").is_err(),"identifiable V4 capture cannot bypass its marker");
    }
    fs::remove_file(s.dir.join("capture.json")).unwrap();
    assert!(s.store.get("fixture").is_ok(),"generic v5 is not implicitly V4");
    fs::write(s.dir.join("capture.json"),br#"{"schemaVersion":1,"state":"#).unwrap();
    assert!(s.store.get("fixture").is_ok(),"unrelated old malformed capture does not alter saved-project behavior");
}

#[cfg(unix)]
#[test]
fn optional_aliases_remain_hard_errors_before_and_after_publication() {
    use std::os::unix::fs::symlink;
    for name in ["mic.wav",SYSTEM,EVIDENCE] {
        let s = Session::new();
        s.arm();
        s.assets();
        let bytes = s.capture_bytes();
        let path = s.dir.join(name);
        let held = s.dir.join("held");
        fs::rename(&path,&held).unwrap();
        symlink(&held,&path).unwrap();
        assert_eq!(publish_finalized_assets(&s.dir,&parsed(&envelope(true)),&evidence()).unwrap_err().code,ErrorCode::InvalidPath);
        assert_eq!(s.capture_bytes(),bytes);
        fs::remove_file(&path).unwrap();
        fs::rename(&held,&path).unwrap();
        s.finish();
        s.write("project.json",&envelope(true)["state"]["initialProject"]);
        fs::remove_file(&path).unwrap();
        symlink(s.dir.join("missing"),&path).unwrap();
        assert_eq!(s.store.get("fixture").unwrap_err().code,ErrorCode::InvalidPath);
    }
}

#[test]
fn document_lock_and_conflicting_intents_prevent_replacement() {
    let s = Session::new();
    let lock = fs::OpenOptions::new().read(true).write(true).create_new(true)
        .open(s._temp.path().join(".locks/fixture.document")).unwrap();
    lock.try_lock().unwrap();
    assert_eq!(publish_incomplete(&s.dir,&parsed(&envelope(false))).unwrap_err().code,ErrorCode::Busy);
    assert!(!s.dir.join(MARKER).exists());
    drop(lock);
    s.arm();
    s.assets();
    let lock = fs::OpenOptions::new().read(true).write(true).open(s._temp.path().join(".locks/fixture.document")).unwrap();
    lock.try_lock().unwrap();
    let before = s.capture_bytes();
    assert_eq!(publish_finalized_assets(&s.dir,&parsed(&envelope(true)),&evidence()).unwrap_err().code,ErrorCode::Busy);
    assert_eq!(s.capture_bytes(),before);
    drop(lock);
    fs::remove_file(s.dir.join("capture.json")).unwrap();
    let linux = focusflow_core::capture::decode(&serde_json::to_vec(&video(false)).unwrap(),"fixture").unwrap();
    assert!(focusflow_core::capture::publish(&s.dir,&linux).is_err());
    let windows = focusflow_core::capture::decode_windows(include_bytes!("fixtures/windows-incomplete.json"),"fixture").unwrap();
    assert!(focusflow_core::capture::publish_windows_incomplete(&s.dir,&windows).is_err());
    assert!(!s.dir.join("capture.json").exists());
    assert!(!s.dir.join("capture.windows-v2").exists());
    s.write("capture.json",&envelope(true));
    fs::write(s.dir.join("capture.windows-v3"),b"focusflow-windows-capture-v3\n").unwrap();
    assert!(s.store.open_or_import("fixture",Path::new("unreachable"),defaults()).is_err());
    assert!(!s.dir.join("project.json").exists());
}

#[test]
fn disabled_track_is_not_a_failed_track_and_legacy_audio_warning_is_converted() {
    let mut v4 = envelope(true);
    v4["state"]["audioRequested"]["system"] = json!(false);
    v4["state"]["audio"]["system"] = json!({"status":"disabled"});
    let inner = &mut v4["state"]["videoAuthority"]["manifest"]["state"]["initialProject"];
    inner["microphone"] = Value::Null;
    inner["warnings"].as_array_mut().unwrap().push(json!("invalidOptionalAudio"));
    v4["state"]["audio"]["microphone"] = json!({"status":"unavailable","reason":"CPAL WAV invalid"});
    let outer = &mut v4["state"]["initialProject"];
    outer["audioTracks"]["microphone"] = Value::Null;
    outer["warnings"] = json!(["cursorCaptureIncomplete","invalidMicrophoneAudio"]);
    let doc = parsed(&v4);
    assert_eq!(serde_json::to_value(&doc).unwrap(),v4);
    for invalid in [json!({"status":"disabled","reason":"unexpected"}), json!({"status":"unavailable","reason":"unexpected"})] {
        let mut altered = v4.clone();
        altered["state"]["audio"]["system"] = invalid;
        assert!(decode(&serde_json::to_vec(&altered).unwrap(),"fixture").is_err());
    }
}

#[test]
fn malformed_compact_origin_is_unavailable_without_losing_video_mic_or_recovery_files() {
    let cases = [
        ("/selection", None),
        ("/selection/selection_epoch", Some(json!("c".repeat(32)))),
        ("/selection/ports/1/id", Some(json!(6))),
        ("/selection/peer_pid", Some(json!(-1))),
        ("/first/kind", None),
        ("/first/kind", Some(json!("gapDiscard"))),
        ("/first/generation", Some(json!(1))),
        ("/first/packet", Some(json!(1))),
        ("/first/sourceStartFrame", Some(json!(1))),
        ("/first/callbackBeforeNs", None),
        ("/first/callbackAfterNs", Some(json!(1103))),
        ("/first/callbackAfterNs", Some(json!("1101"))),
        ("/first/callbackBeforeNs", Some(json!("1100"))),
        ("/first/callbackAfterNs", Some(json!("86400000001101"))),
        ("/first/callbackAfterNs", Some(json!("18446744073709551616"))),
        ("/first/callbackBeforeNs", Some(json!("01102"))),
        ("/latest", None),
        ("/latest/generation", Some(json!(1))),
        ("/latest/packet", Some(json!(0))),
        ("/latest/sourceStartFrame", Some(json!(4))),
        ("/latest", Some(json!({"kind":"monitor","generation":0,"packet":1,"sourceStartFrame":4,
            "frameCount":4,"callbackBeforeNs":"1200","callbackAfterNs":"1201"}))),
        ("/latest/frameCount", Some(json!(4))),
        ("/latest/callbackBeforeNs", Some(json!("1102"))),
        ("/packets", Some(json!(0))),
        ("/packets", Some(json!(9))),
        ("/terminalReason", None),
        ("/terminalReason", Some(json!("failed"))),
        ("/origin/childCommitObservationNs", Some(json!([1100,1102]))),
        ("/writtenFrames", Some(json!(7))),
    ];
    let mut wrongly_validated = Vec::new();
    for (path, replacement) in cases {
        let session = Session::new();
        session.arm();
        session.assets();
        let mut compact = system_evidence();
        match &replacement {
            Some(value) => *compact.pointer_mut(path).unwrap() = value.clone(),
            None => {
                let (parent, key) = path.rsplit_once('/').unwrap();
                compact.pointer_mut(parent).unwrap().as_object_mut().unwrap().remove(key);
            }
        }
        // Keep the two copies equal so first-row tests exercise origin facts.
        compact["origin"]["firstRetainedPacket"] = compact["first"].clone();
        session.write(EVIDENCE, &compact);
        let files = ["screen.mp4", "mic.wav", SYSTEM, EVIDENCE];
        let before = files.map(|name| fs::read(session.dir.join(name)).unwrap());
        let committed = session.finish();
        let wire = serde_json::to_value(&committed).unwrap();
        let document = committed.finalized_project().unwrap();
        assert_eq!(wire["state"]["videoAuthority"]["manifest"], video(true));
        assert_eq!(wire["state"]["audio"]["microphone"]["status"], "validated");
        assert!(document.audio_tracks.as_ref().unwrap().microphone.is_some());
        assert!(document.audio_tracks.as_ref().unwrap().system.is_none());
        assert_eq!(files.map(|name| fs::read(session.dir.join(name)).unwrap()), before);
        if wire["state"]["audio"]["system"]["status"] != "unavailable"
            || !document.warnings.contains(&ProjectWarning::InvalidSystemAudio)
        {
            wrongly_validated.push((path, replacement));
        }
    }
    assert!(wrongly_validated.is_empty(), "accepted invalid producer evidence {wrongly_validated:?}");
}

#[test]
fn compact_origin_supports_one_packet_and_a_span_with_interior_packets() {
    for packets in [1, 3] {
        let session = Session::new();
        session.arm();
        session.assets();
        let mut compact = system_evidence();
        if packets == 1 {
            compact["first"]["frameCount"] = json!(8);
            compact["origin"]["firstRetainedPacket"] = compact["first"].clone();
            compact["latest"] = compact["first"].clone();
        } else {
            compact["latest"]["packet"] = json!(2);
            compact["latest"]["sourceStartFrame"] = json!(6);
            compact["latest"]["frameCount"] = json!(2);
        }
        compact["packets"] = json!(packets);
        compact["terminalReason"] = json!("normal");
        session.write(EVIDENCE, &compact);
        let committed = session.finish();
        assert_eq!(serde_json::to_value(committed).unwrap(), envelope(true));
    }
}
