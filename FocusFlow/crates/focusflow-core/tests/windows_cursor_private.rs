//! Constructed private qualification inputs, not imported capture authority or native evidence.
#[path="../../../src-tauri/src/recorder/windows_capture/process/qualification/cursor_math.rs"]
mod cursor_math;
mod common;
#[test]
fn private_proof_layout_cannot_import_capture_or_publish_embedded_authority(){
 use std::{fs,path::Path};use focusflow_core::{store::ProjectStore,legacy::LegacyDefaults};
 let temp=common::TestDir::new();let dir=temp.path().join("private-proof");fs::create_dir(&dir).unwrap();fs::write(dir.join("proof-without-cursor.mp4"),b"private native pixels").unwrap();fs::write(dir.join("proof-summary.json"),b"{\"privateProof\":true,\"neverImport\":true}").unwrap();
 let store=ProjectStore::new(temp.path()).unwrap();assert!(store.list_projects(None,100).unwrap().projects.is_empty());
 let defaults=LegacyDefaults{scale:1.0,intro_us:0,hold_us:0,outro_us:0,pan_transition_us:100000,settle_us:100000};assert!(store.open_or_import("private-proof",Path::new("absent-ffmpeg"),defaults).is_err());
 for name in ["capture.json","capture.windows-v2","project.json","screen.mp4","session.json"]{assert!(!dir.join(name).exists());}
 assert_eq!(fs::read(dir.join("proof-without-cursor.mp4")).unwrap(),b"private native pixels");
}
