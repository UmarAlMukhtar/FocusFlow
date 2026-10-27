use super::*;
use super::super::*;
use std::{path::PathBuf, process::{Command, Stdio}, sync::{Arc, Mutex, atomic::AtomicBool}, time::Duration};

struct Directory(PathBuf);
impl Directory {
    fn new(label:&str)->Self {
        static NEXT:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
        let path=std::env::temp_dir().join(format!("focusflow-lifecycle-{label}-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();Self(path.canonicalize().unwrap())
    }
}
impl Drop for Directory {fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
fn source()->cursor::Source {cursor::Source {bounds:CaptureBounds{x:0,y:0,width:320,height:240},window:None}}
fn without_sampler(dir:&Path)->Session {
    let source=source();
    Session {sampler:None,outcome:Some(cursor::Outcome {raw_path:dir.join("cursor/raw.ndjson"),committed_bytes:0,records:0,shapes:vec![],cutoff_ns:None,trustworthy_prefix:false,degraded:true,reason:Some("Controlled missing worker result")}),source,
        manifest:incomplete(dir.file_name().unwrap().to_str().unwrap(),SourceKind::Screen,source),defaults:crate::export::ExportSettings::default().legacy_defaults().unwrap()}
}
fn ffmpeg()->PathBuf {PathBuf::from(std::env::var_os("FOCUSFLOW_FFMPEG").unwrap_or_else(||"/usr/bin/ffmpeg".into()))}
fn synthetic_video(dir:&Path) {
    let status=Command::new(ffmpeg()).args(["-v","error","-f","lavfi","-i","color=c=0x228844:s=320x240:r=30","-t","1","-c:v","libx264","-pix_fmt","yuv420p"])
        .arg(dir.join("screen.mp4")).status().unwrap();assert!(status.success());
}
fn empty_tracker()->ClickTracker {
    ClickTracker {stop_requested:Arc::new(AtomicBool::new(true)),join_handle:None,clicks:Arc::new(Mutex::new(vec![])),drags:Arc::new(Mutex::new(vec![]))}
}
fn active_without_sampler(dir:&Path, nonzero:bool)->ActiveRecording {
    let child=Command::new(if nonzero {"/usr/bin/false"}else{"/usr/bin/true"}).stdin(Stdio::piped()).spawn().unwrap();
    let encoder_pid=child.id();
    ActiveRecording {native_capture:Some(without_sampler(dir)),notice:None,capture:ActiveCapture::Ffmpeg{child,encoder_pid},
        video_clock:video_clock::Clock::start(video_clock::Capability::probe(Path::new("/usr/bin/false")),Instant::now()),audio_recorder:None,mic_device_name:None,
        click_tracker:empty_tracker(),output_path:dir.join("screen.mp4"),started_at:Instant::now(),monitor:MonitorInfo{name:None,x:0,y:0,width:320,height:240,scale_factor:1.0},
        capture_bounds:source().bounds,source:RecordingSource::Screen,ffmpeg_path:ffmpeg(),log:Arc::new(Mutex::new(ProcessLog::default()))}
}
#[test]
fn admission_failure_leaves_embedded_arguments_and_never_attempts_authority() {
    let dir=Directory::new("admission");let state=RecorderState::new();
    let mut reservation=StartReservation::acquire(&state).unwrap();reservation.directory=Some(dir.0.clone());
    let mut session=without_sampler(&dir.0);
    let clock=video_clock::Clock::start(video_clock::Capability::probe(Path::new("/usr/bin/false")),Instant::now());
    let mut args=vec!["-draw_mouse".into(),"1".into(),"screen.mp4".into()];
    let error=session.arm(&dir.0,Instant::now(),&clock,&mut reservation,&mut args).unwrap_err();
    assert_eq!(error.code,"cursor_clock_unavailable");assert_eq!(args[1],"1");assert!(!dir.0.join("capture.json").exists());
    assert!(reservation.directory.is_some());drop(reservation);assert!(!dir.0.exists());
}
#[test]
fn stop_retains_valid_video_after_nonzero_source_end_and_missing_worker_result() {
    let dir=Directory::new("source-end");synthetic_video(&dir.0);
    let active=active_without_sampler(&dir.0,true);let pid=active.capture.pid().unwrap();
    capture::publish(&dir.0,&active.native_capture.as_ref().unwrap().manifest).unwrap();
    let state=RecorderState::new();state.lock_runtime().unwrap().active=Some(active);
    let result=tauri::async_runtime::block_on(stop_active_recording(&state)).unwrap();
    assert_eq!(result.phase,RecordingPhase::Completed);assert!(!Path::new(&format!("/proc/{pid}")).exists());
    let manifest:CaptureManifestV1=serde_json::from_slice(&fs::read(dir.0.join("capture.json")).unwrap()).unwrap();
    let CaptureStateV1::Finalized {video_timing,cursor_timing,initial_project:doc}=manifest.state else {panic!("completion without finalized authority")};
    assert_eq!(doc.video.cursor,CursorMode::Absent);assert!(!doc.cursor_settings.enabled);assert!(doc.warnings.contains(&ProjectWarning::CursorCaptureIncomplete));
    assert!(video_timing.clock.is_none()&&cursor_timing.is_none()&&doc.microphone.is_none());
    assert!(!state.cleanup_runtime().finalizing);assert!(!dir.0.join("project.json").exists());
}
#[test]
fn stop_required_media_failure_retains_marker_and_notifies_finalization_waiters() {
    for corrupt in [false,true] {
        let dir=Directory::new("bad-media");if corrupt {fs::write(dir.0.join("screen.mp4"),b"broken media").unwrap();}
        let active=active_without_sampler(&dir.0,false);let pid=active.capture.pid().unwrap();
        capture::publish(&dir.0,&active.native_capture.as_ref().unwrap().manifest).unwrap();let before=fs::read(dir.0.join("capture.json")).unwrap();
        let state=RecorderState::new();let (send,mut receive)=tauri::async_runtime::channel(1);
        {let mut runtime=state.lock_runtime().unwrap();runtime.active=Some(active);runtime.lifecycle_waiters.push(send);}
        assert!(tauri::async_runtime::block_on(stop_active_recording(&state)).is_err());
        assert_eq!(fs::read(dir.0.join("capture.json")).unwrap(),before);assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert!(receive.try_recv().is_ok());let runtime=state.cleanup_runtime();
        assert!(!runtime.finalizing&&runtime.finalizing_output_path.is_none()&&runtime.last_completed.is_none()&&runtime.active.is_none());
    }
}
#[test]
fn unconfirmed_guardian_stop_cancels_statistics_and_releases_waiters_without_finalizing_capture() {
    let dir=Directory::new("held-guardian");synthetic_video(&dir.0);let release=dir.0.join("release-guardian");
    let mut child=Command::new("/usr/bin/python3").args(["-c", "import os,sys,time;print('R',end='',flush=True);end=time.monotonic()+18\nwhile not os.path.exists(sys.argv[1]) and time.monotonic()<end: time.sleep(.02)"])
        .arg(&release).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let pid=child.id();let mut stdout=child.stdout.take().unwrap();let mut ready=[0];stdout.read_exact(&mut ready).unwrap();assert_eq!(ready,[b'R']);
    let mut active=active_without_sampler(&dir.0,false);
    let previous=std::mem::replace(&mut active.capture,ActiveCapture::Ffmpeg{child,encoder_pid:pid});
    tauri::async_runtime::block_on(stop_capture_backend(previous,&active.log)).unwrap();
    let capability=video_clock::Capability::probe(&ffmpeg());assert!(capability.supported());
    active.video_clock=video_clock::Clock::start(capability,Instant::now());active.video_clock.read(stdout);
    capture::publish(&dir.0,&active.native_capture.as_ref().unwrap().manifest).unwrap();let marker=fs::read(dir.0.join("capture.json")).unwrap();
    let state=RecorderState::new();let (send,mut receive)=tauri::async_runtime::channel(1);
    {let mut runtime=state.lock_runtime().unwrap();runtime.active=Some(active);runtime.lifecycle_waiters.push(send);}
    let started=Instant::now();let result=tauri::async_runtime::block_on(stop_active_recording(&state));let elapsed=started.elapsed();
    let retained_writer=Path::new(&format!("/proc/{pid}")).exists();
    // Release only our controlled guardian after Stop returned; the real background reaper owns wait().
    fs::write(&release,b"release").unwrap();let deadline=Instant::now()+Duration::from_secs(3);
    while Path::new(&format!("/proc/{pid}")).exists() && Instant::now()<deadline {thread::sleep(Duration::from_millis(20));}
    assert!(!Path::new(&format!("/proc/{pid}")).exists(),"controlled guardian was not reaped");
    let error=result.expect_err("unconfirmed cleanup must not publish completed authority");assert_eq!(error.code,"ffmpeg_stop_timeout");
    assert!(retained_writer,"cleanup depended on writer EOF");assert!(elapsed<STOP_TIMEOUT+Duration::from_secs(3),"Stop blocked for {elapsed:?}");
    assert_eq!(fs::read(dir.0.join("capture.json")).unwrap(),marker);assert!(receive.try_recv().is_ok());
    {let runtime=state.cleanup_runtime();assert!(!runtime.finalizing&&runtime.finalizing_output_path.is_none()&&runtime.last_completed.is_none());}
    tauri::async_runtime::block_on(shutdown(&state));assert!(state.cleanup_runtime().shutting_down);
}

#[test]
fn final_authority_failure_retains_source_and_never_sets_completed() {
    let dir=Directory::new("final-authority");synthetic_video(&dir.0);
    let active=active_without_sampler(&dir.0,false);
    capture::publish(&dir.0,&active.native_capture.as_ref().unwrap().manifest).unwrap();
    // A concurrently damaged authority must fail closed after media validation.
    fs::write(dir.0.join("capture.json"),b"invalid authority").unwrap();
    let original=fs::read(dir.0.join("screen.mp4")).unwrap();let state=RecorderState::new();state.lock_runtime().unwrap().active=Some(active);
    let error=tauri::async_runtime::block_on(stop_active_recording(&state)).unwrap_err();assert_eq!(error.code,"capture_finalize_failed");
    assert_eq!(fs::read(dir.0.join("screen.mp4")).unwrap(),original);assert_eq!(fs::read(dir.0.join("capture.json")).unwrap(),b"invalid authority");
    let runtime=state.cleanup_runtime();assert!(!runtime.finalizing&&runtime.last_completed.is_none());
}

#[test]
#[ignore="requires root-owned authenticated DISPLAY=:98, generated content, actual FFmpeg and guardian"]
fn native_start_stop_lifecycle_fixture() {
    use x11rb::{connection::Connection, protocol::xproto::*, wrapper::ConnectionExt as _};
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::env::var("DISPLAY").as_deref(),Ok(":98"));
    assert_eq!(std::env::var("FOCUSFLOW_ISOLATED_X11").as_deref(),Ok("1"));
    let root=PathBuf::from(std::env::var_os("FOCUSFLOW_NATIVE_EVIDENCE").expect("existing owned evidence directory"));
    let guardian=PathBuf::from(std::env::var_os("FOCUSFLOW_NATIVE_GUARDIAN").expect("actual built executable"));
    let encoder=ffmpeg();
    assert!(root.is_absolute()&&root.is_dir()&&guardian.is_absolute()&&guardian.is_file()&&encoder.is_absolute()&&encoder.is_file());
    let (c,screen)=x11::connect().unwrap();let screen=&c.setup().roots[screen];
    let window=c.generate_id().unwrap();
    c.create_window(x11rb::COPY_DEPTH_FROM_PARENT,window,screen.root,100,100,320,240,0,WindowClass::INPUT_OUTPUT,0,
        &CreateWindowAux::new().background_pixel(0x228844).override_redirect(1)).unwrap().check().unwrap();
    c.change_property8(PropMode::REPLACE,window,AtomEnum::WM_NAME,AtomEnum::STRING,b"Generated lifecycle fixture").unwrap().check().unwrap();
    c.map_window(window).unwrap().check().unwrap();c.warp_pointer(0u32,window,0,0,0,0,80,80).unwrap().check().unwrap();c.flush().unwrap();
    thread::sleep(Duration::from_millis(100));
    let root_bounds=CaptureBounds{x:0,y:0,width:u32::from(screen.width_in_pixels)&!1,height:u32::from(screen.height_in_pixels)&!1};
    let region=CaptureBounds{x:100,y:100,width:320,height:240};
    let mut receipts=Vec::new();
    for (name,kind,fault) in [("screen-0",0,""),("region-0",1,""),("window-0",2,""),("screen-1",0,""),("region-1",1,""),("window-1",2,""),
        ("optional-sidecars",1,"sidecars"),("output-cap",1,"cap"),("unsupported",1,"unsupported"),("authority-failure",1,"authority"),("guardian-failure",1,"guardian"),("runtime-poison",1,"poison")] {
        let dir=root.join(name);fs::create_dir(&dir).unwrap();let video=dir.join("screen.mp4");
        let calls=dir.join("ffmpeg-calls.ndjson");let wrapper=dir.join("observed-ffmpeg");
        let script=format!("#!/usr/bin/python3\nimport os,sys,json\nwith open({:?},'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\nif {:?} == 'unsupported' and '-stats_enc_pre:v:0' in sys.argv: sys.exit(1)\nos.execv({:?},[{:?}]+sys.argv[1:])\n",calls.to_str().unwrap(),fault,encoder.to_str().unwrap(),encoder.to_str().unwrap());
        fs::write(&wrapper,script).unwrap();fs::set_permissions(&wrapper,fs::Permissions::from_mode(0o700)).unwrap();
        let bounds=if kind==0 {root_bounds}else{region};let id=(kind==2).then_some(window);
        let source=match kind {0=>RecordingSource::Screen,1=>RecordingSource::Region{x:bounds.x,y:bounds.y,width:bounds.width,height:bounds.height},_=>RecordingSource::Window{hwnd:format!("0x{window:X}"),title:"Generated lifecycle fixture".into()}};
        let mut args=x11::args(bounds,id,&video).unwrap();
        if fault=="cap" {let i=args.iter().position(|v|v=="-t").unwrap();args[i+1]="2".into();}
        let plan=CapturePlan{source,monitor:MonitorInfo{name:None,x:bounds.x,y:bounds.y,width:bounds.width,height:bounds.height,scale_factor:1.0},bounds,backend:CaptureBackend::Ffmpeg,ffmpeg_args:args,
            window_capture:id.map(|id|WindowCapturePlan{hwnd:id as usize,title:"Generated lifecycle fixture".into(),width:320,height:240})};
        let mut defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();defaults.pan_transition_us=180_000;defaults.scale=3.0;
        let state=RecorderState::new();let mut reservation=StartReservation::acquire(&state).unwrap();reservation.directory=Some(dir.clone());
        if fault=="authority" {fs::create_dir(dir.join("capture.json")).unwrap();}
        if fault=="poison" {let _=std::panic::catch_unwind(||{let _guard=state.runtime.lock().unwrap();panic!("controlled runtime poison before active transfer")});}
        let startup=tauri::async_runtime::block_on(start_prepared_recording(&state,&mut reservation,video.clone(),plan,wrapper.clone(),if fault=="guardian" {Path::new("/usr/bin/false")}else{&guardian},defaults,None));
        drop(reservation);
        if ["authority","guardian","poison"].contains(&fault) {
            assert!(startup.is_err(),"{name}");assert!(dir.exists()&&dir.join("capture.json").exists());{let runtime=state.cleanup_runtime();assert!(!runtime.starting&&runtime.active.is_none());}
            let calls=fs::read_to_string(&calls).unwrap();let launches=calls.lines().filter(|line|line.contains("x11grab")).count();
            assert_eq!(launches,usize::from(fault=="poison"),"launch occurred before admission: {name}");
            if fault=="poison" {
                let manifest:CaptureManifestV1=serde_json::from_slice(&fs::read(dir.join("capture.json")).unwrap()).unwrap();assert!(matches!(manifest.state,CaptureStateV1::Incomplete{..}));
                // EOF after guardian reaping makes a nonempty finalized video independently inspectable.
                assert!(fs::metadata(&video).unwrap().len()>0);
            }
            receipts.push(serde_json::json!({"name":name,"expectedStartupFailure":startup.unwrap_err().code,"recordingLaunches":launches}));continue;
        }
        let startup=startup.unwrap();let pid=startup.pid.unwrap();
        assert_eq!(startup.notice.is_some(),fault=="unsupported");
        if fault!="unsupported" {
            assert!(matches!(serde_json::from_slice::<CaptureManifestV1>(&fs::read(dir.join("capture.json")).unwrap()).unwrap().state,CaptureStateV1::Incomplete{..}));
        }
        assert!(StartReservation::acquire(&state).is_err());
        // Controlled delivery-time input exercises the production normalization/rebase/snapshot path.
        {let runtime=state.lock_runtime().unwrap();let active=runtime.active.as_ref().unwrap();
            active.click_tracker.clicks.lock().unwrap().push(ClickEvent{timestamp:0.8,x:if kind==2 {80}else{180},y:if kind==2 {80}else{180},button:ClickButton::Left});}
        thread::sleep(Duration::from_millis(if fault=="cap" {2700}else{1700}));
        if fault=="cap" {
            let mut runtime=state.lock_runtime().unwrap();let ActiveCapture::Ffmpeg{child,..}=&mut runtime.active.as_mut().unwrap().capture;
            assert!(child.try_wait().unwrap().is_some(),"output cap did not end encoder with normal drain");
        }
        if fault=="sidecars" {for file in ["video-clock.json","clicks.json","drags.json","timeline.json"] {let path=dir.join(file);if path.exists(){fs::remove_file(&path).unwrap();}fs::create_dir(&path).unwrap();}}
        let completed=tauri::async_runtime::block_on(stop_active_recording(&state)).unwrap();
        assert_eq!(completed.phase,RecordingPhase::Completed);assert!(!Path::new(&format!("/proc/{pid}")).exists());assert!(!state.cleanup_runtime().finalizing);
        let calls:Vec<Vec<String>>=fs::read_to_string(&calls).unwrap().lines().map(|line|serde_json::from_str(line).unwrap()).collect();
        let probes=calls.iter().filter(|args|args.iter().any(|arg|arg=="framehash")).count();assert_eq!(probes,2,"completion must reuse one first/tail pair");
        let launch=calls.iter().find(|args|args.iter().any(|arg|arg=="x11grab")).unwrap();let mouse=launch.iter().position(|arg|arg=="-draw_mouse").unwrap();assert_eq!(launch[mouse+1],if fault=="unsupported" {"1"}else{"0"});
        if fault=="unsupported" {assert!(!dir.join("capture.json").exists());assert!(completed.notice.is_some());receipts.push(serde_json::json!({"name":name,"embedded":true,"completionProbes":probes}));continue;}
        let manifest:CaptureManifestV1=serde_json::from_slice(&fs::read(dir.join("capture.json")).unwrap()).unwrap();manifest.validate(name).unwrap();
        let CaptureStateV1::Finalized{video_timing,initial_project:doc,..}=&manifest.state else {panic!("no finalized capture")};
        assert!(video_timing.clock.is_some(),"{name}: actual completed encoder clock rejected; retained evidence must be reviewed");
        assert_eq!(doc.video.cursor,CursorMode::Separate);assert_eq!(doc.camera.pan_transition_us,180_000);assert!(!doc.clicks.is_empty());assert!(!doc.zooms.is_empty());
        assert!(doc.clicks[0].at_us<800_000,"snapshot failed to retain once-corrected event");assert_eq!(doc.cursor_settings.style,CursorStyle::Native);assert!(doc.microphone.is_none());
        if fault=="cap" {let clock=video_timing.clock.as_ref().unwrap();assert_eq!(clock.completed_frames,60);assert_eq!(doc.video.duration_us,2_000_000);}
        let prepared=focusflow_core::cursor::prepare_cursor(&dir,doc,&AtomicBool::new(false)).unwrap().unwrap();
        assert!(prepared.sample_at(1_000_000,1).is_some(),"native consumer has no stationary cursor");
        assert!(!dir.join("project.json").exists());
        receipts.push(serde_json::json!({"name":name,"encoderPid":pid,"completionProbes":probes,"completedFrames":video_timing.clock.as_ref().unwrap().completed_frames,"durationUs":doc.video.duration_us,"correctedClickUs":doc.clicks[0].at_us,"nativeSamples":prepared.sample_count()}));
        fs::write(root.join("lifecycle-receipts.json"),serde_json::to_vec_pretty(&receipts).unwrap()).unwrap();
    }
    c.destroy_window(window).unwrap().check().unwrap();c.flush().unwrap();
    fs::write(root.join("lifecycle-receipts.json"),serde_json::to_vec_pretty(&receipts).unwrap()).unwrap();
}

#[test]
fn shutdown_during_start_returns_ownership_to_startup_cleanup() {
    let dir=Directory::new("shutdown-start-transfer");let state=RecorderState::new();let reservation=StartReservation::acquire(&state).unwrap();
    let active=active_without_sampler(&dir.0,false);state.request_shutdown().unwrap();
    let (error,active)=transfer_active(&state,active).err().expect("Shutdown must reject publication");assert_eq!(error.code,"recorder_shutting_down");assert!(state.cleanup_runtime().active.is_none());
    active.click_tracker.request_stop();match active.capture{ActiveCapture::Ffmpeg{child,..}=>crate::capture_guard::close_and_reap(child)};active.click_tracker.finish().unwrap();drop(active.native_capture);drop(reservation);
    tauri::async_runtime::block_on(shutdown(&state));assert!(!state.cleanup_runtime().starting);
}
