use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "focusflow-core-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Could not create test directory: {error}"),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("Could not remove test directory");
    }
}

#[allow(dead_code)]
pub fn fixture_project() -> focusflow_core::project::ProjectDocument {
    use focusflow_core::project::*;
    ProjectDocument {
        frame_style: Default::default(), schema_version: 1, project_id: "fixture".into(), revision: 0,
        cursor: None, cursor_settings: Default::default(),
        video: VideoAsset { cursor: CursorMode::Embedded, path: "screen.mp4".into(), width: 100, height: 50, duration_us: 5_000_000 },
        microphone: None,
        timing: Timing { provenance: TimingProvenance::LegacyUnknown, audio_offset_us: None },
        camera: CameraDefaults { pan_transition_us: 200_000, settle_us: 100_000 },
        clips: vec![Clip { id: "clip-0".into(), in_us: 0, out_us: 5_000_000 }],
        zooms: vec![], clicks: vec![],
        warnings: vec![ProjectWarning::LegacyTimingUnknown, ProjectWarning::EmbeddedCursor],
    }
}

#[allow(dead_code)]
pub fn write_legacy(dir: &Path) {
    for (name, value) in [
        ("metadata.json", serde_json::json!({"version":1,"source":{"type":"region","x":1920,"y":0,"width":100,"height":50},"captureBounds":{"x":1920,"y":0,"width":100,"height":50},"captureBackend":"ffmpeg","hasMicAudio":false})),
        ("session.json", serde_json::json!({"session_id":"old","created_at":"2026-10-20","duration_seconds":0.0,"recording_source":"region","exported":false})),
        ("timeline.json", serde_json::json!([{"start":1.0,"end":2.0,"x":50,"y":25,"scale":3.0}])),
        ("clicks.json", serde_json::json!([{"timestamp":1.0,"x":50,"y":25,"button":"left"}])),
        ("drags.json", serde_json::json!([{"start":1.0,"end":2.0,"button":"left","points":[{"timestamp":1.5,"x":50,"y":25}]}])),
    ] { std::fs::write(dir.join(name), serde_json::to_vec(&value).unwrap()).unwrap(); }
    std::fs::write(dir.join("screen.mp4"), b"synthetic media placeholder; probe supplied separately").unwrap();
}

// Ignored entry point reused by the store integration executable for real process contention.
#[test]
#[ignore]
fn child_writer() {
    let root = std::path::PathBuf::from(std::env::var_os("FOCUSFLOW_WRITER_ROOT").expect("writer root"));
    let index = std::env::var("FOCUSFLOW_WRITER_INDEX").expect("writer index");
    std::fs::write(root.join(format!("ready-{index}")), b"ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("start").exists() {
        assert!(std::time::Instant::now() < deadline, "parent did not release writer barrier");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = focusflow_core::store::ProjectStore::new(&root).unwrap().apply_edits("fixture", 0, &[
        focusflow_core::edits::Edit::TrimClip { clip_id: "clip-0".into(), in_us: 1_000_000, out_us: 4_000_000 }
    ]);
    let value = match result {
        Ok(document) => serde_json::json!({"revision": document.revision}),
        Err(error) => serde_json::to_value(error).unwrap(),
    };
    std::fs::write(root.join(format!("result-{index}")), serde_json::to_vec(&value).unwrap()).unwrap();
}

#[allow(dead_code)]
pub fn fixed_zoom_wire() -> serde_json::Value {
    serde_json::json!({"id":"zoom-0","clipId":"clip-0","startUs":1_000_000,"endUs":3_000_000,
        "x":0.25,"y":0.75,"scale":2.0,"introUs":200_000,"holdUs":300_000,"outroUs":400_000})
}

#[allow(dead_code)]
pub fn project_wire(version: u32) -> serde_json::Value {
    let mut doc = fixture_project();
    doc.schema_version = version;
    let mut wire = serde_json::to_value(doc).unwrap();
    wire["zooms"] = serde_json::json!([fixed_zoom_wire()]);
    if version >= 4 { wire["zooms"][0]["mode"] = serde_json::json!("fixed"); }
    wire
}
