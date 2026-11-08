use super::*;
use std::{cell::RefCell,sync::atomic::{AtomicU64,Ordering}};
static NEXT:AtomicU64=AtomicU64::new(0);
struct Fixture {root:PathBuf,dir:PathBuf}
impl Fixture {
    fn new()->Self {let root=std::env::temp_dir().join(format!("focusflow-windows-publish-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));let dir=root.join("fixture");fs::create_dir_all(&dir).unwrap();Self {root:root.canonicalize().unwrap(),dir:dir.canonicalize().unwrap()}}
}
impl Drop for Fixture {fn drop(&mut self){fs::remove_dir_all(&self.root).unwrap();}}
fn start()->WindowsCaptureManifestV2 {decode_windows(include_bytes!("../../../tests/fixtures/windows-incomplete.json"),"fixture").unwrap()}
fn final_manifest()->WindowsCaptureManifestV2 {decode_windows(include_bytes!("../../../tests/fixtures/windows-finalized.json"),"fixture").unwrap()}
fn grid()->ffmpeg::SavedCfrGrid {ffmpeg::SavedCfrGrid {completed_frames:30,video:ffmpeg::ProbedVideoEvidence {video:ffmpeg::ProbedVideo {width:642,height:360,duration_us:1_000_000},endpoint:ffmpeg::PacketTime {ticks:30000,numerator:1,denominator:30000},first_packet_duration:ffmpeg::PacketTime {ticks:1000,numerator:1,denominator:30000}}}}
fn fail()->std::io::Result<()> {Err(std::io::Error::other("injected publication failure"))}
fn no_project(f:&Fixture) {assert!(!f.dir.join("project.json").exists());}
#[test]
fn lock_and_invalid_candidate_fail_before_persistent_marker_creation() {
    let f=Fixture::new();let store=store::ProjectStore::new(&f.root).unwrap();let lock=store.lock("fixture","document").unwrap();
    assert_eq!(publish_windows_incomplete(&f.dir,&start()).unwrap_err().code,ErrorCode::Busy);assert!(!f.dir.join(MARKER_NAME).exists());drop(lock);
    let mut invalid=start();invalid.source.window_id="0".into();assert!(publish_windows_incomplete(&f.dir,&invalid).is_err());assert!(!f.dir.join(MARKER_NAME).exists());
    assert!(read_authority(&f.dir,"fixture").unwrap().is_none());no_project(&f);
}
#[test]
fn marker_write_and_sync_failures_leave_recovery_without_authority_or_video() {
    for prefix in [0,7,MARKER_BYTES.len()] {
        let f=Fixture::new();let result=publish_windows_with(&f.dir,&start(),None,|file|{file.write_all(&MARKER_BYTES[..prefix])?;if prefix==MARKER_BYTES.len(){file.flush()?;}fail()},|_|panic!("capture write after marker failure"),|_|panic!("rename after marker failure"),|_,_|panic!("directory sync after marker failure"));
        assert!(result.is_err());assert_eq!(fs::read(f.dir.join(MARKER_NAME)).unwrap(),&MARKER_BYTES[..prefix]);assert!(!f.dir.join("capture.json").exists());assert!(!f.dir.join("screen.mp4").exists());assert!(read_authority(&f.dir,"fixture").is_err());no_project(&f);
        if prefix<MARKER_BYTES.len(){assert!(publish_windows_incomplete(&f.dir,&start()).is_err());}else{publish_windows_incomplete(&f.dir,&start()).unwrap();}
    }
    let f=Fixture::new();assert!(publish_windows_with(&f.dir,&start(),None,write_marker,|_|panic!("capture write after marker directory sync failure"),|_|Ok(()),|_,_|fail()).is_err());assert_eq!(fs::read(f.dir.join(MARKER_NAME)).unwrap(),MARKER_BYTES);assert!(!f.dir.join("capture.json").exists());assert!(read_authority(&f.dir,"fixture").is_err());
}
#[test]
fn create_new_conflicts_are_never_overwritten_or_followed() {
    for content in [b"".as_slice(),b"partial".as_slice()] {let f=Fixture::new();fs::write(f.dir.join(MARKER_NAME),content).unwrap();assert!(publish_windows_incomplete(&f.dir,&start()).is_err());assert_eq!(fs::read(f.dir.join(MARKER_NAME)).unwrap(),content);assert!(!f.dir.join("capture.json").exists());}
    let f=Fixture::new();fs::create_dir(f.dir.join(MARKER_NAME)).unwrap();assert!(publish_windows_incomplete(&f.dir,&start()).is_err());assert!(f.dir.join(MARKER_NAME).is_dir());
    // Exercise the actual create_new guarantee without introducing a public race hook.
    let f=Fixture::new();let path=f.dir.join(MARKER_NAME);let mut first=fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();first.write_all(b"partial").unwrap();assert_eq!(fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap_err().kind(),std::io::ErrorKind::AlreadyExists);assert_eq!(fs::read(path).unwrap(),b"partial");
}
#[test]
fn incomplete_order_is_marker_flush_sync_then_directory_then_json_then_parent() {
    let f=Fixture::new();let calls=RefCell::new(vec![]);
    publish_windows_with(&f.dir,&start(),None,|file|{assert!(!f.dir.join("capture.json").exists());calls.borrow_mut().push("marker".to_owned());write_marker(file)},|path|{assert_eq!(fs::read(f.dir.join(MARKER_NAME))?,MARKER_BYTES);assert_eq!(fs::metadata(path)?.len(),0);calls.borrow_mut().push("write".into());Ok(())},|_|{calls.borrow_mut().push("rename".into());Ok(())},|path,directory|{calls.borrow_mut().push(if path==f.dir {"session"}else{"parent"}.into());sync_asset(path,directory)}).unwrap();
    assert_eq!(*calls.borrow(),["marker","session","write","rename","session","parent"]);no_project(&f);
}
#[test]
fn replacement_failure_retains_marker_and_previous_incomplete() {
    for existing in [false,true] {for stage in ["write","rename"] {
        let f=Fixture::new();if existing {publish_windows_incomplete(&f.dir,&start()).unwrap();}
        let before=fs::read(f.dir.join("capture.json")).ok();fs::write(f.dir.join(".capture.json-unrelated.tmp"),b"other owner").unwrap();
        let error=publish_windows_with(&f.dir,&start(),None,write_marker,|p|{if stage=="write" {fs::write(p,b"partial")?;fail()}else{Ok(())}},|_|if stage=="rename"{fail()}else{Ok(())},sync_asset);
        assert!(error.is_err());assert_eq!(fs::read(f.dir.join("capture.json")).ok(),before);assert_eq!(fs::read(f.dir.join(MARKER_NAME)).unwrap(),MARKER_BYTES);assert_eq!(fs::read(f.dir.join(".capture.json-unrelated.tmp")).unwrap(),b"other owner");assert_eq!(fs::read_dir(&f.dir).unwrap().count(),if existing {3}else{2});no_project(&f);
    }}
}
#[test]
fn post_rename_sync_failure_reports_committed_but_unconfirmed_and_requires_reload() {
    for parent_failure in [false,true] {
        let f=Fixture::new();let renamed=RefCell::new(false);
        let error=publish_windows_with(&f.dir,&start(),None,write_marker,|_|Ok(()),|_|{*renamed.borrow_mut()=true;Ok(())},|path,directory|{if *renamed.borrow()&&(!parent_failure||path==f.root) {fail()}else{sync_asset(path,directory)}}).unwrap_err();
        assert!(error.message.contains("durability is unconfirmed")&&error.message.contains("Reload"));assert!(matches!(read_authority(&f.dir,"fixture").unwrap(),Some(CaptureAuthority::WindowsV2(_))));no_project(&f);
    }
}
fn mic(dir:&Path) {let mut w=hound::WavWriter::create(dir.join("mic.wav"),hound::WavSpec {channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();w.write_sample(100i16).unwrap();w.finalize().unwrap();}
#[test]
fn finalization_syncs_only_retained_media_before_atomic_authority_and_is_immutable() {
    let f=Fixture::new();publish_windows_incomplete(&f.dir,&start()).unwrap();fs::write(f.dir.join("screen.mp4"),b"immutable").unwrap();mic(&f.dir);
    let mut candidate=final_manifest();if let WindowsCaptureStateV2::Finalized {initial_project,..}=&mut candidate.state {initial_project.microphone=Some(project::AudioAsset {path:"mic.wav".into()});}
    let calls=RefCell::new(vec![]);
    publish_windows_with(&f.dir,&candidate,Some(&grid()),|_|panic!("new marker after incomplete"),|_|{calls.borrow_mut().push("write".to_owned());Ok(())},|_|{calls.borrow_mut().push("rename".to_owned());Ok(())},|path,directory|{calls.borrow_mut().push(path.strip_prefix(&f.dir).unwrap().to_string_lossy().into_owned());sync_asset(path,directory)}).unwrap();
    assert_eq!(*calls.borrow(),[MARKER_NAME,"","screen.mp4","mic.wav","","write","rename",""]);assert!(publish_windows_finalized_assets(&f.dir,&candidate,&grid()).is_err());assert_eq!(fs::read(f.dir.join("screen.mp4")).unwrap(),b"immutable");no_project(&f);
}
#[test]
fn every_finalization_failure_preserves_incomplete_until_rename() {
    for stage in ["marker-sync","pre-directory","video","audio","assets-directory","write","rename","post-directory"] {
        let f=Fixture::new();publish_windows_incomplete(&f.dir,&start()).unwrap();let before=fs::read(f.dir.join("capture.json")).unwrap();fs::write(f.dir.join("screen.mp4"),b"immutable").unwrap();mic(&f.dir);let audio=fs::read(f.dir.join("mic.wav")).unwrap();
        let mut candidate=final_manifest();if let WindowsCaptureStateV2::Finalized {initial_project,..}=&mut candidate.state {initial_project.microphone=Some(project::AudioAsset {path:"mic.wav".into()});}
        let mut dirs=0;
        let error=publish_windows_with(&f.dir,&candidate,Some(&grid()),write_marker,|_|if stage=="write"{fail()}else{Ok(())},|_|if stage=="rename"{fail()}else{Ok(())},|path,directory|{
            if directory {dirs+=1;}
            if (stage=="marker-sync"&&path.ends_with(MARKER_NAME))||(stage=="video"&&path.ends_with("screen.mp4"))||(stage=="audio"&&path.ends_with("mic.wav"))||(directory&&[("pre-directory",1),("assets-directory",2),("post-directory",3)].contains(&(stage,dirs))) {fail()}else{sync_asset(path,directory)}
        }).unwrap_err();
        if stage=="post-directory" {assert!(error.message.contains("durability is unconfirmed"));assert_ne!(fs::read(f.dir.join("capture.json")).unwrap(),before);assert!(publish_windows_finalized_assets(&f.dir,&candidate,&grid()).is_err());}else{assert_eq!(fs::read(f.dir.join("capture.json")).unwrap(),before);}
        assert_eq!(fs::read(f.dir.join("screen.mp4")).unwrap(),b"immutable");assert_eq!(fs::read(f.dir.join("mic.wav")).unwrap(),audio);no_project(&f);
    }
}

#[test]
fn existing_linux_authority_is_rejected_before_marker_creation_and_stays_readable() {
    let f=Fixture::new();let linux:CaptureManifestV1=serde_json::from_value(serde_json::json!({"schemaVersion":1,"captureId":"fixture","source":{"backend":"linuxX11","kind":"region","initialBounds":{"x":0,"y":0,"width":100,"height":50},"windowId":null},"diagnostic":null,"state":{"status":"incomplete","videoPath":"screen.mp4","videoCursor":"separate","rawSamplesPath":"cursor/raw.ndjson"}})).unwrap();
    publish(&f.dir,&linux).unwrap();let before=fs::read(f.dir.join("capture.json")).unwrap();
    assert!(publish_windows_incomplete(&f.dir,&start()).is_err());
    assert!(publish_windows_finalized_assets(&f.dir,&final_manifest(),&grid()).is_err());
    assert!(!f.dir.join(MARKER_NAME).exists());assert_eq!(fs::read(f.dir.join("capture.json")).unwrap(),before);
    let Some(CaptureAuthority::LinuxV1(actual))=read_authority(&f.dir,"fixture").unwrap() else {panic!("Windows attempt hid existing Linux authority")};assert_eq!(actual,linux);no_project(&f);
}

#[test]
fn sync_existing_assets_preserves_bytes_and_never_creates_missing_files() {
    let f=Fixture::new();
    fs::write(f.dir.join(MARKER_NAME),MARKER_BYTES).unwrap();
    fs::write(f.dir.join("screen.mp4"),(0..65536).map(|n|(n%251) as u8).collect::<Vec<_>>()).unwrap();
    mic(&f.dir);
    for name in [MARKER_NAME,"screen.mp4","mic.wav"] {
        let path=f.dir.join(name);let before=fs::read(&path).unwrap();
        sync_asset(&path,false).unwrap();
        assert_eq!(fs::read(&path).unwrap(),before,"sync changed {name}");
    }
    let missing=f.dir.join("missing.mp4");
    assert_eq!(sync_asset(&missing,false).unwrap_err().kind(),std::io::ErrorKind::NotFound);
    assert!(!missing.exists());no_project(&f);
}
