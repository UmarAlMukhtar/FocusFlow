mod common;
use common::{fixture_project, TestDir};
use focusflow_core::{edits::Edit, legacy::LegacyDefaults, project::*, store::ProjectStore, ErrorCode};
use std::{fs, path::{Path, PathBuf}, process::Command, time::{Duration, Instant}};

fn setup() -> (TestDir, PathBuf, ProjectStore) {
    let temp = TestDir::new();
    let project = temp.path().join("fixture");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("screen.mp4"), b"synthetic required video").unwrap();
    fs::write(project.join("project.json"), serde_json::to_vec(&fixture_project()).unwrap()).unwrap();
    let store = ProjectStore::new(temp.path()).unwrap();
    (temp, project, store)
}
fn trim() -> Vec<Edit> { vec![Edit::TrimClip { clip_id: "clip-0".into(), in_us: 1_000_000, out_us: 4_000_000 }] }
fn defaults() -> LegacyDefaults { LegacyDefaults { scale: 2.0, intro_us: 200_000, hold_us: 300_000, outro_us: 200_000, pan_transition_us: 200_000, settle_us: 100_000 } }

// Reimport must never overwrite an edited document or require FFmpeg for an existing one.
#[test]
fn repeat_import_preserves_edited_document() {
    let (_temp, project, store) = setup();
    let result = store.apply_edits("fixture", 0, &trim());
    assert!(result.is_ok(), "{result:?}");
    let edited = result.unwrap();
    assert_eq!(edited.revision, 1);
    assert_eq!(edited.clips[0].in_us, 1_000_000);
    fs::write(project.join("timeline.json"), b"malformed legacy data").unwrap();
    fs::write(project.join(".project-abandoned.tmp"), b"invalid temporary bytes").unwrap();
    assert_eq!(store.open_or_import("fixture", Path::new("absent-ffmpeg"), defaults()).unwrap(), edited);
    assert_eq!(serde_json::from_slice::<ProjectDocument>(&fs::read(project.join("project.json")).unwrap()).unwrap(), edited);
}

// A stale export must not silently capture newer edits; the old snapshot must remain unchanged.
#[test]
fn stale_snapshot_is_rejected() {
    let (_temp, _project, store) = setup();
    let first = store.snapshot("fixture", 0);
    assert!(first.is_ok(), "{first:?}");
    let snapshot = first.unwrap();
    store.apply_edits("fixture", 0, &trim()).unwrap();
    assert_eq!(store.snapshot("fixture", 0).unwrap_err().code, ErrorCode::RevisionConflict);
    assert_eq!(snapshot.document.revision, 0);
    assert_eq!(snapshot.document.clips[0].in_us, 0);
    assert_eq!(store.snapshot("fixture", 1).unwrap().document.revision, 1);
}

// Candidate validation failure must retain the complete previous bytes and revision.
#[test]
fn failed_save_retains_previous_json() {
    let (_temp, project, store) = setup();
    let before = fs::read(project.join("project.json")).unwrap();
    let invalid = [Edit::TrimClip { clip_id: "clip-0".into(), in_us: 4, out_us: 4 }];
    assert_eq!(store.apply_edits("fixture", 0, &invalid).unwrap_err().code, ErrorCode::InvalidDocument);
    assert_eq!(fs::read(project.join("project.json")).unwrap(), before);
    assert_eq!(store.get("fixture").unwrap().revision, 0);
}

// Separate OS processes must contend on the same persistent document lock, not in-memory state.
#[test]
fn two_processes_cannot_win_same_revision() {
    let (temp, project, _store) = setup();
    let mut children = Vec::new();
    for n in 0..2 {
        children.push(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "common::child_writer", "--ignored", "--nocapture"])
            .env("FOCUSFLOW_WRITER_ROOT", temp.path())
            .env("FOCUSFLOW_WRITER_INDEX", n.to_string()).spawn().unwrap());
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(temp.path().join("ready-0").exists() && temp.path().join("ready-1").exists()) {
        assert!(Instant::now() < deadline, "child writer did not reach barrier");
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(temp.path().join("start"), b"go").unwrap();
    for mut child in children { assert!(child.wait().unwrap().success()); }
    let results: Vec<serde_json::Value> = (0..2).map(|n| serde_json::from_slice(&fs::read(temp.path().join(format!("result-{n}"))).unwrap()).unwrap()).collect();
    assert_eq!(results.iter().filter(|value| value["revision"] == 1).count(), 1, "{results:?}");
    assert_eq!(results.iter().filter(|value| value["code"] == "busy" || value["code"] == "revisionConflict").count(), 1, "{results:?}");
    let winner: ProjectDocument = serde_json::from_slice(&fs::read(project.join("project.json")).unwrap()).unwrap();
    assert_eq!(winner.revision, 1);
    assert_eq!(winner.clips[0].in_us, 1_000_000);
    assert!(temp.path().join(".locks/fixture.document").is_file());
}


#[test]
fn imports_legacy_once_without_changing_source_files() {
    let temp = TestDir::new();
    let project = temp.path().join("legacy");
    fs::create_dir(&project).unwrap();
    common::write_legacy(&project);
    let status = Command::new("ffmpeg").args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black:s=100x50:r=10:d=5", "-c:v", "libx264", "-pix_fmt", "yuv420p"]).arg(project.join("screen.mp4")).status().unwrap();
    assert!(status.success(), "real FFmpeg fixture generation failed");
    let originals: Vec<_> = ["screen.mp4", "metadata.json", "timeline.json", "clicks.json", "drags.json"].iter().map(|name| (*name, fs::read(project.join(name)).unwrap())).collect();
    let store = ProjectStore::new(temp.path()).unwrap();
    let result = store.open_or_import("legacy", Path::new("ffmpeg"), defaults());
    assert!(result.is_ok(), "{result:?}");
    let imported = result.unwrap();
    assert_eq!(imported.revision, 0);
    assert_eq!(imported.project_id, "legacy");
    assert_eq!(store.get("legacy").unwrap(), imported);
    for (name, bytes) in originals { assert_eq!(fs::read(project.join(name)).unwrap(), bytes, "{name}"); }
}

#[test]
fn existing_invalid_or_future_document_never_reimports() {
    let (_temp, project, store) = setup();
    for (bytes, code) in [
        (br#"{"schemaVersion":5,"futureField":true}"#.as_slice(), ErrorCode::UnsupportedSchema),
        (b"invalid JSON".as_slice(), ErrorCode::InvalidDocument),
    ] {
        fs::write(project.join("project.json"), bytes).unwrap();
        assert_eq!(store.open_or_import("fixture", Path::new("absent-ffmpeg"), defaults()).unwrap_err().code, code);
        assert_eq!(fs::read(project.join("project.json")).unwrap(), bytes);
    }
}

#[test]
fn rejects_wrong_document_id_oversized_reads_and_revision_overflow() {
    let (_temp, project, store) = setup();
    let mut doc = fixture_project();
    doc.project_id = "other".into();
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(store.get("fixture").unwrap_err().code, ErrorCode::InvalidDocument);
    doc.project_id = "fixture".into();
    doc.revision = MAX_REVISION;
    let before = serde_json::to_vec(&doc).unwrap();
    fs::write(project.join("project.json"), &before).unwrap();
    assert_eq!(store.apply_edits("fixture", MAX_REVISION, &trim()).unwrap_err().code, ErrorCode::InvalidDocument);
    assert_eq!(fs::read(project.join("project.json")).unwrap(), before);
    fs::File::create(project.join("project.json")).unwrap().set_len(MAX_JSON_BYTES + 1).unwrap();
    assert_eq!(store.get("fixture").unwrap_err().code, ErrorCode::InvalidDocument);
}

#[test]
fn stale_edit_and_invalid_batch_leave_bytes_unchanged() {
    let (_temp, project, store) = setup();
    let before = fs::read(project.join("project.json")).unwrap();
    assert_eq!(store.apply_edits("fixture", 1, &trim()).unwrap_err().code, ErrorCode::RevisionConflict);
    assert_eq!(store.apply_edits("fixture", 0, &[]).unwrap_err().code, ErrorCode::InvalidDocument);
    assert_eq!(fs::read(project.join("project.json")).unwrap(), before);
}

#[test]
fn snapshot_requires_video_but_downgrades_missing_or_corrupt_optional_audio() {
    let (_temp, project, store) = setup();
    let mut doc = fixture_project();
    doc.microphone = Some(AudioAsset { path: "mic.wav".into() });
    let before = serde_json::to_vec(&doc).unwrap();
    fs::write(project.join("project.json"), &before).unwrap();
    for corrupt in [false, true] {
        if corrupt { fs::write(project.join("mic.wav"), b"corrupt audio").unwrap(); }
        let snapshot = store.snapshot("fixture", 0).unwrap();
        assert!(snapshot.document.microphone.is_none());
        assert!(snapshot.document.warnings.contains(&ProjectWarning::InvalidOptionalAudio));
        assert_eq!(snapshot.document.revision, 0);
        assert_eq!(fs::read(project.join("project.json")).unwrap(), before);
    }
    fs::remove_file(project.join("screen.mp4")).unwrap();
    assert_eq!(store.snapshot("fixture", 0).unwrap_err().code, ErrorCode::NotFound);
}

#[test]
fn snapshot_retains_valid_microphone() {
    let (_temp, project, store) = setup();
    let mut writer = hound::WavWriter::create(project.join("mic.wav"), hound::WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }).unwrap();
    for sample in [1_i16, 2, 3] { writer.write_sample(sample).unwrap(); }
    writer.finalize().unwrap();
    let mut doc = fixture_project();
    doc.microphone = Some(AudioAsset { path: "mic.wav".into() });
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(store.snapshot("fixture", 0).unwrap().document.microphone, doc.microphone);
}

#[test]
fn document_lock_is_persistent_and_blocks_edits_and_delete() {
    let (temp, project, store) = setup();
    store.get("fixture").unwrap();
    let path = temp.path().join(".locks/fixture.document");
    let document_lock = fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    document_lock.try_lock().unwrap();
    assert_eq!(store.get("fixture").unwrap_err().code, ErrorCode::Busy);
    assert_eq!(store.apply_edits("fixture", 0, &trim()).unwrap_err().code, ErrorCode::Busy);
    assert_eq!(store.delete_project("fixture").unwrap_err().code, ErrorCode::Busy);
    document_lock.unlock().unwrap();
    drop(document_lock);
    store.delete_project("fixture").unwrap();
    assert!(!project.exists());
    assert!(path.is_file());
    // The real-job render/delete guard is exercised by jobs::tests::delete_during_render_is_busy.

}

#[cfg(any(unix, windows))]
fn symlink(target: &Path, link: &Path, directory: bool) {
    #[cfg(unix)]
    { let _ = directory; std::os::unix::fs::symlink(target, link).unwrap(); }
    #[cfg(windows)]
    if directory { std::os::windows::fs::symlink_dir(target, link).expect("Directory symlink coverage requires privileges"); }
    else { std::os::windows::fs::symlink_file(target, link).expect("File symlink coverage requires privileges"); }
}

#[cfg(any(unix, windows))]
#[test]
fn lock_directory_and_files_cannot_escape_via_symlinks() {
    let temp = TestDir::new();
    let outside = TestDir::new();
    symlink(outside.path(), &temp.path().join(".locks"), true);
    assert_eq!(ProjectStore::new(temp.path()).err().unwrap().code, ErrorCode::InvalidPath);
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);

    for suffix in ["document", "render"] {
        let (temp, project, store) = setup();
        let outside_file = outside.path().join(suffix);
        fs::write(&outside_file, b"untouched outside").unwrap();
        symlink(&outside_file, &temp.path().join(format!(".locks/fixture.{suffix}")), false);
        let error = if suffix == "document" { store.get("fixture").unwrap_err() } else { store.delete_project("fixture").unwrap_err() };
        assert_eq!(error.code, ErrorCode::InvalidPath);
        assert_eq!(fs::read(outside_file).unwrap(), b"untouched outside");
        assert!(project.exists());
    }
}

#[cfg(any(unix, windows))]
#[test]
fn snapshot_rejects_escaping_or_dangling_audio_symlinks() {
    let (_temp, project, store) = setup();
    let outside = TestDir::new();
    fs::write(outside.path().join("mic.wav"), b"outside corrupt audio").unwrap();
    let mut doc = fixture_project();
    doc.microphone = Some(AudioAsset { path: "mic.wav".into() });
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    symlink(&outside.path().join("mic.wav"), &project.join("mic.wav"), false);
    assert_eq!(store.snapshot("fixture", 0).unwrap_err().code, ErrorCode::InvalidPath);
    fs::remove_file(outside.path().join("mic.wav")).unwrap();
    assert_eq!(store.snapshot("fixture", 0).unwrap_err().code, ErrorCode::InvalidPath);
    fs::remove_file(project.join("mic.wav")).unwrap();
    symlink(outside.path(), &project.join("audio"), true);
    doc.microphone = Some(AudioAsset { path: "audio/missing.wav".into() });
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(store.snapshot("fixture", 0).unwrap_err().code, ErrorCode::InvalidPath);
}


#[cfg(any(unix, windows))]
#[test]
fn missing_audio_under_contained_directory_alias_falls_back() {
    let (_temp, project, store) = setup();
    fs::create_dir(project.join("media")).unwrap();
    symlink(&project.join("media"), &project.join("audio"), true);
    let mut doc = fixture_project();
    doc.microphone = Some(AudioAsset { path: "audio/missing.wav".into() });
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    let result = store.snapshot("fixture", 0);
    assert!(result.is_ok(), "{result:?}");
    let snapshot = result.unwrap();
    assert!(snapshot.document.microphone.is_none());
    assert!(snapshot.document.warnings.contains(&ProjectWarning::InvalidOptionalAudio));
}


#[test]
fn duplicate_project_fields_are_rejected_without_rewrite() {
    let (_temp, project, store) = setup();
    let bytes = serde_json::to_string(&fixture_project()).unwrap()
        .replacen("{", "{\"projectId\":\"other\",", 1);
    fs::write(project.join("project.json"), &bytes).unwrap();
    let result = store.get("fixture");
    assert!(result.is_err(), "Duplicate document keys must not silently take the last value");
    assert_eq!(result.unwrap_err().code, ErrorCode::InvalidDocument);
    assert_eq!(fs::read(project.join("project.json")).unwrap(), bytes.as_bytes());
}

#[test]
fn zoom_mode_read_preserves_old_bytes_then_apply_or_restore_upgrades_once() {
    for version in 1..=3 {
        for restore in [false, true] {
            let (_temp, project, store) = setup();
            let mut wire = common::project_wire(version);
            wire["revision"] = serde_json::json!(7);
            let bytes = serde_json::to_vec_pretty(&wire).unwrap();
            fs::write(project.join("project.json"), &bytes).unwrap();
            let read = store.get("fixture").unwrap();
            assert_eq!((read.schema_version, read.revision), (version, 7));
            assert_eq!(fs::read(project.join("project.json")).unwrap(), bytes);
            let state = EditableState::from_document(&read);
            let updated = if restore {store.restore_edits("fixture", 7, &state)} else {store.apply_edits("fixture", 7, &trim())}.unwrap();
            assert_eq!((updated.schema_version, updated.revision), (4, 8));
            assert_eq!(serde_json::to_value(&updated).unwrap()["zooms"][0]["mode"], "fixed");
            assert_eq!(store.get("fixture").unwrap(), updated);
            let again = store.restore_edits("fixture", 8, &state).unwrap();
            assert_eq!((again.schema_version, again.revision), (4, 9));
        }
    }
}

#[test]
fn zoom_mode_follow_restore_conflict_and_invalid_edits_are_atomic() {
    let (_temp, project, store) = setup();
    let mut wire = common::project_wire(4);
    wire["video"]["cursor"] = serde_json::json!("separate");
    wire["cursor"] = serde_json::json!({"indexPath":"cursor/index.json"});
    wire["zooms"][0]["mode"] = serde_json::json!("follow");
    let document: ProjectDocument = serde_json::from_value(wire).unwrap();
    fs::write(project.join("project.json"), serde_json::to_vec(&document).unwrap()).unwrap();
    fs::create_dir(project.join("cursor")).unwrap();
    fs::write(project.join("cursor/index.json"),serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5_000_000,"samplesPath":"cursor/samples.ndjson","shapes":[]}).to_string()).unwrap();
    fs::write(project.join("cursor/samples.ndjson"),serde_json::json!({"atUs":0,"x":0.5,"y":0.5,"visible":true,"shapeId":null,"discontinuity":false}).to_string()).unwrap();
    let state = EditableState::from_document(&document);
    let removed = store.apply_edits("fixture", 0, &[Edit::RemoveZoom {zoom_id:"zoom-0".into()}]).unwrap();
    assert!(removed.zooms.is_empty());
    let restored = store.restore_edits("fixture", 1, &state).unwrap();
    assert_eq!((restored.schema_version, restored.revision), (4, 2));
    assert_eq!(serde_json::to_value(&restored).unwrap()["zooms"][0]["mode"], "follow");
    let before = fs::read(project.join("project.json")).unwrap();
    assert_eq!(store.restore_edits("fixture", 1, &state).unwrap_err().code, ErrorCode::RevisionConflict);
    assert_eq!(store.apply_edits("fixture", 1, &trim()).unwrap_err().code, ErrorCode::RevisionConflict);
    let mut overlap = restored.zooms[0].clone(); overlap.id = "overlap".into();
    assert_eq!(store.apply_edits("fixture", 2, &[Edit::PutZoom {zoom:overlap}]).unwrap_err().code, ErrorCode::InvalidDocument);
    let mut invalid = state; invalid.zooms[0].scale = 9.0;
    assert_eq!(store.restore_edits("fixture", 2, &invalid).unwrap_err().code, ErrorCode::InvalidDocument);
    assert_eq!(fs::read(project.join("project.json")).unwrap(), before);
    assert_eq!(store.get("fixture").unwrap().revision, 2);
}

#[test]
fn zoom_mode_invalid_follow_provenance_does_not_write_or_upgrade() {
    for cursor in [CursorMode::Embedded, CursorMode::Absent] {
        let (_temp, project, store) = setup();
        let mut doc = fixture_project(); doc.schema_version = 3; doc.video.cursor = cursor;
        let bytes = serde_json::to_vec(&doc).unwrap();
        fs::write(project.join("project.json"), &bytes).unwrap();
        let mut wire = common::fixed_zoom_wire(); wire["mode"] = serde_json::json!("follow");
        let zoom: ZoomElement = serde_json::from_value(wire).unwrap();
        assert_eq!(store.apply_edits("fixture", 0, &[Edit::PutZoom {zoom:zoom.clone()}]).unwrap_err().code, ErrorCode::InvalidDocument);
        let mut state = EditableState::from_document(&doc); state.zooms = vec![zoom];
        assert_eq!(store.restore_edits("fixture", 0, &state).unwrap_err().code, ErrorCode::InvalidDocument);
        assert_eq!(fs::read(project.join("project.json")).unwrap(), bytes);
        assert_eq!((store.get("fixture").unwrap().schema_version, store.get("fixture").unwrap().revision), (3, 0));
    }
}
