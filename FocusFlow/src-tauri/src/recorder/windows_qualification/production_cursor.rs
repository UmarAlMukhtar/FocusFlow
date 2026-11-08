//! Source-only production qualification entry. No release-family switch; separate native approval required.
use super::*;
#[path = "production_failure.rs"]
mod production_failure;
use production_failure::{report, Stage};
use focusflow_core::cursor_observation::ModelProfile;
pub(in crate::recorder) const MODE: &str = "--focusflow-owned-production-cursor-fixture";
/// Only this owned-fixture module constructs candidate access. It never changes release admission.
pub(in crate::recorder) struct OwnedFixturePermission {
    window: u64,
    pid: u32,
    session: PathBuf,
    profile: ModelProfile,
}
impl OwnedFixturePermission {
    fn new(
        window: &GeneratedWindow,
        session: &Path,
        profile: ModelProfile,
    ) -> Result<Self, String> {
        if session.parent().map(|p| p.join("source")) != Some(window.dir.clone()) {
            return Err("Fixture source/session ownership layout mismatch".into());
        }
        let selected =
            fixture_read(&window.dir.join("source-selection.json")).map_err(|e| e.to_string())?;
        let pid = u32::try_from(
            selected["sourcePid"]
                .as_u64()
                .ok_or("Missing owned source PID")?,
        )
        .map_err(|e| e.to_string())?;
        Self::from_worker(
            window.hwnd as u64,
            pid,
            session,
            match profile {
                ModelProfile::Popup => "popup",
                ModelProfile::Decorated => "decorated",
            },
        )
    }
    pub(in crate::recorder) fn from_worker(
        window: u64,
        pid: u32,
        session: &Path,
        profile: &str,
    ) -> Result<Self, String> {
        cursor::require_disposable()?;
        let profile = match profile {
            "popup" => ModelProfile::Popup,
            "decorated" => ModelProfile::Decorated,
            _ => return Err("Unsupported owned fixture profile".into()),
        };
        let permit = Self {
            window,
            pid,
            session: session.canonicalize().map_err(|e| e.to_string())?,
            profile,
        };
        permit.check_session(session)?;
        permit.check(window, pid)?;
        Ok(permit)
    }
    pub(in crate::recorder) fn check_session(&self, session: &Path) -> Result<(), String> {
        if session.canonicalize().map_err(|e| e.to_string())? != self.session
            || session
                .read_dir()
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
        {
            return Err("Production fixture requires its fresh empty owned session".into());
        }
        Ok(())
    }
    pub(in crate::recorder) fn check(&self, window: u64, pid: u32) -> Result<(), String> {
        cursor::require_disposable()?;
        if (window, pid) != (self.window, self.pid) || pid == 0 || pid == std::process::id() {
            return Err("Fixture cursor owner mismatch".into());
        }
        let root = self.session.parent().ok_or("Fixture root missing")?;
        let observed =
            fixture_read(&root.join("source/source-observed.json")).map_err(|e| e.to_string())?;
        let selected =
            fixture_read(&root.join("source/source-selection.json")).map_err(|e| e.to_string())?;
        if observed["hwnd"].as_u64() != Some(window)
            || observed["pid"].as_u64() != Some(u64::from(pid))
            || selected["hwnd"].as_u64() != Some(window)
            || selected["sourcePid"].as_u64() != Some(u64::from(pid))
        {
            return Err("Owned fixture handshake mismatch".into());
        }
        let mut actual = 0;
        unsafe {
            GetWindowThreadProcessId(HWND(window as usize as *mut _), Some(&mut actual));
        }
        if actual != pid {
            return Err("Owned fixture HWND changed process".into());
        }
        Ok(())
    }
    pub(in crate::recorder) fn profile(&self) -> ModelProfile {
        self.profile
    }
    pub(in crate::recorder) fn profile_arg(&self) -> &'static str {
        match self.profile {
            ModelProfile::Popup => "popup",
            ModelProfile::Decorated => "decorated",
        }
    }
}
/// Runs only after a separately authorized outer owner supplies the source process and worker.
/// Action timestamps remain independent checker evidence and never enter the producer.
pub(super) fn case(
    root: &Path,
    w: u32,
    h: u32,
    nonce: u16,
    ffmpeg: &Path,
    worker: &Path,
) -> TestResult<()> {
    cursor::require_disposable()?;
    static_policy::case_tuple(w, h, nonce)?;
    let mut dpi = cursor::DpiScope::enter()?;
    let profile = if nonce % 2 == 0 {
        ModelProfile::Popup
    } else {
        ModelProfile::Decorated
    };
    let source = root.join("source");
    fs::create_dir(&source)?;
    fixture_json(
        &source.join("source-request.json"),
        &serde_json::json!({"width":w,"height":h,"nonce":nonce,"animated":false,"privateProfile":if profile==ModelProfile::Popup{"borderless"}else{"decorated"}}),
    )?;
    let mut window = GeneratedWindow::observed(source)?;
    let session = root.join("session-0");
    fs::create_dir(&session)?;
    let permission = OwnedFixturePermission::new(&window, &session, profile)?;
    for name in [
        "production-start-authorized.json",
        "production-actions-complete.json",
    ] {
        if fs::symlink_metadata(root.join(name)).is_ok() {
            return Err("Stale production authorization".into());
        }
    }
    fixture_json(
        &root.join("production-source-ready.json"),
        &serde_json::json!({"hwnd":window.hwnd,"sourcePid":permission.pid}),
    )?;
    wait_static_action(
        root,
        "production-start-authorized.json",
        "static-owned-canvas-v1",
        window.hwnd as u64,
        permission.pid,
    )?;
    permission.check(window.hwnd as u64, permission.pid)?;
    let placement = cursor::place_owned_static(window.hwnd as u64, permission.pid, [w, h])?;
    fixture_json(&root.join("production-placement.json"), &placement)?;
    let mut initial_pointer = ::windows::Win32::Foundation::POINT::default();
    unsafe {
        GetPhysicalCursorPos(&mut initial_pointer)?;
    }
    let mut physical = Vec::with_capacity(3);
    for [x, y] in [[8, 8], [w as i32 - 8, 8], [8, h as i32 - 8]] {
        let mut point = ::windows::Win32::Foundation::POINT { x, y };
        // Independent physical client anchors are checker truth, never arguments to the cursor producer.
        if !unsafe { ClientToScreen(HWND(window.hwnd as *mut _), &mut point) }.as_bool() {
            return Err("Production physical anchor query failed".into());
        }
        physical.push([point.x, point.y]);
    }
    fixture_json(
        &root.join("production-geometry.json"),
        &serde_json::json!({"profile":profile,"physicalAnchors":physical,"initialPointerPosition":[initial_pointer.x,initial_pointer.y],"clientSize":[w,h],"originPolicy":"dwmExtendedFrameTranslationV1; reject disagreement; never fit"}),
    ).map_err(|error| report(Stage::Geometry, error))?;
    let (window_capture, bounds) = window_capture_plan_from_hwnd(
        HWND(window.hwnd as *mut _),
        "FocusFlow generated owned content",
    ).map_err(|error| report(Stage::Plan, error))?;
    let plan = CapturePlan {
        source: RecordingSource::Window {
            hwnd: window.hwnd.to_string(),
            title: window_capture.title.clone(),
        },
        monitor: MonitorInfo {
            name: Some("generated".into()),
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
            scale_factor: 1.0,
        },
        bounds,
        backend: CaptureBackend::WindowsCapture,
        ffmpeg_args: vec![],
        window_capture: Some(window_capture),
    };
    let defaults = crate::export::ExportSettings::default().legacy_defaults()
        .map_err(|error| report(Stage::Defaults, error))?;
    let state = RecorderState::new();
    let mut reservation = StartReservation::acquire(&state)
        .map_err(|error| report(Stage::Reservation, error))?;
    let mut recording = windows_capture::Session::prepare_fixture(
        &session,
        &plan,
        defaults,
        ffmpeg,
        worker,
        &permission,
    ).map_err(|error| report(Stage::Prepare, error))?;
    recording.arm(&session, &mut reservation)
        .map_err(|error| report(Stage::Arm, error))?;
    let owned = windows_capture::qualification::Handles::observe(&recording)
        .map_err(|error| report(Stage::Observe, error))?;
    if fs::symlink_metadata(root.join("production-actions-complete.json")).is_ok() {
        return Err("Premature production hold completion".into());
    }
    fixture_json(
        &root.join("production-armed.json"),
        &serde_json::json!({"hwnd":window.hwnd,"sourcePid":permission.pid,"profile":profile,"candidateOnly":true}),
    )?;
    // Outer holds the fixed neutral point; producer never reads action controls.
    wait_static_action(
        root,
        "production-actions-complete.json",
        "static-hold-complete-v1",
        window.hwnd as u64,
        permission.pid,
    )?;
    permission.check(window.hwnd as u64, permission.pid)?;
    cursor::verify_owned_static(window.hwnd as u64, permission.pid, [w, h], &placement)?;
    let oracle = root.join("native-raster-oracle");
    fs::create_dir(&oracle)?;
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        GetCursorInfo(&mut info)?;
    }
    let shape = cursor::copied_shape(info.hCursor, &oracle, 0, 0)?;
    fixture_json(
        &oracle.join("shape.json"),
        &serde_json::json!({"path":"cursor-0.pam","width":shape.width,"height":shape.height,"hotspot":shape.hotspot,"phase":"unknown"}),
    )?;
    let start = Instant::now();
    let completion = recording.finish()?;
    let elapsed = start.elapsed();
    owned.verify(&session)?;
    fixture_json(
        &root.join("production-observer-completion.json"),
        &completion
            .observation_receipt()?
            .ok_or("Missing production cursor completion")?,
    )?;
    window.close()?;
    fixture_json(
        &root.join("production-phase.json"),
        &serde_json::json!({"phase":"postcapture","hwnd":window.hwnd,"sourcePid":permission.pid,"workerEncoderJoined":true,"sourceClosed":true,"qpc":observation_qpc().ok_or("Postcapture QPC unavailable")?}),
    )?;
    let grid = probe_cfr_grid(
        ffmpeg,
        &session.join("screen.mp4"),
        completion.count(),
        &AtomicBool::new(false),
        Instant::now() + Duration::from_secs(30),
    )?;
    fs::write(
        session.join("before-publish"),
        b"complete production media before real Completion publication",
    )?;
    let hash_deadline = Instant::now() + Duration::from_secs(2);
    while !session.join("media-hashed").is_file() {
        if Instant::now() >= hash_deadline {
            return Err("Independent production media hash missing".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
    completion.publish(&session, &grid, false)?;
    let data = focusflow_core::cursor_observation::read_observations(&session)?;
    let manifest = focusflow_core::capture::windows_observations::decode_windows_observations(
        &fs::read(session.join("capture.json"))?,
        "session-0",
    )?;
    let sampled =
        focusflow_core::capture::windows_observations::checked_interactions(&manifest, &data)?;
    let evidence = serde_json::to_vec(&sampled)?;
    if evidence.len() > focusflow_core::cursor_observation::MAX_INTERACTION_BYTES {
        return Err("Sampled interaction evidence exceeds fixed cap".into());
    }
    fs::write(root.join("production-sampled-interactions.json"), evidence)?;
    let doc = ProjectStore::new(root)?.open_or_import("session-0", ffmpeg, defaults)?;
    if data.poses.is_empty()
        || data.inputs.is_empty()
        || doc.video.cursor != focusflow_core::project::CursorMode::Separate
    {
        return Err("Production fixture has no usable canonical cursor observations".into());
    }
    fixture_json(
        &root.join("production-source-receipt.json"),
        &serde_json::json!({"profile":profile,"finishMs":elapsed.as_millis(),"poseRows":data.poses.len(),"inputRows":data.inputs.len(),"clock":data.index.clock,"terminal":data.index.terminal,"ordinaryImport":true,"productionCursorQualified":false,"independentPixelAndInputOracleRequired":true}),
    )?;
    dpi.restore()?;
    Ok(())
}

fn wait_static_action(
    root: &Path,
    name: &str,
    action: &str,
    hwnd: u64,
    pid: u32,
) -> TestResult<()> {
    use std::{
        io::Read,
        os::windows::fs::{MetadataExt, OpenOptionsExt},
    };
    let path = root.join(name);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if !meta.is_file()
                    || meta.file_attributes() & 0x400 != 0
                    || meta.len() > 4096
                    || path.canonicalize()?.parent() != Some(root.canonicalize()?.as_path())
                {
                    return Err("Foreign or oversized static authorization".into());
                }
                // FILE_FLAG_OPEN_REPARSE_POINT prevents following a replaced control link.
                let file = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(0x00200000)
                    .open(&path)?;
                if file.metadata()?.file_attributes() & 0x400 != 0 {
                    return Err("Replaced static authorization link".into());
                }
                let mut bytes = Vec::with_capacity(4097);
                file.take(4097).read_to_end(&mut bytes)?;
                return static_policy::authorize(&bytes, action, hwnd, pid).map_err(Into::into);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        if Instant::now() >= deadline {
            return Err("Static production control timed out".into());
        }
        thread::sleep(Duration::from_millis(5));
    }
}

mod static_policy {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct Action {
        action: String,
        hwnd: u64,
        source_pid: u32,
    }
    pub(super) fn authorize(bytes: &[u8], action: &str, hwnd: u64, pid: u32) -> Result<(), String> {
        let value: Action = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if bytes.len() > 4096
            || hwnd == 0
            || pid == 0
            || value.action != action
            || value.hwnd != hwnd
            || value.source_pid != pid
        {
            return Err("Static fixture authorization mismatch".into());
        }
        Ok(())
    }
    pub(super) fn case_tuple(w: u32, h: u32, nonce: u16) -> Result<(), String> {
        if !matches!(
            (w, h, nonce),
            (320, 240, 700 | 701 | 704 | 705) | (641, 359, 702 | 703 | 706 | 707)
        ) {
            return Err("Unsupported static production case tuple".into());
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn authorization_rejects_missing_extra_duplicate_and_wrong_owner() {
            assert!(authorize(
                br#"{"action":"static-owned-canvas-v1","hwnd":123,"sourcePid":22}"#,
                "static-owned-canvas-v1",
                123,
                22
            )
            .is_ok());
            for bytes in [
                br#"{"action":"static-owned-canvas-v1","hwnd":123}"#.as_slice(),
                br#"{"action":"static-owned-canvas-v1","hwnd":123,"sourcePid":22,"extra":true}"#,
                br#"{"action":"static-owned-canvas-v1","hwnd":123,"hwnd":123,"sourcePid":22}"#,
                br#"{"action":"static-owned-canvas-v1","hwnd":124,"sourcePid":22}"#,
                br#"{"action":"static-owned-canvas-v1","hwnd":123,"sourcePid":23}"#,
                br#"{"action":"static-hold-complete-v1","hwnd":123,"sourcePid":22}"#,
            ] {
                assert!(authorize(bytes, "static-owned-canvas-v1", 123, 22).is_err());
            }
            assert!(authorize(
                br#"{"action":"static-hold-complete-v1","hwnd":123,"sourcePid":22}"#,
                "static-hold-complete-v1",
                123,
                22
            )
            .is_ok());
        }
        #[test]
        fn only_fixed_static_case_tuples_are_accepted() {
            for (w, h, nonce) in [
                (320, 240, 700),
                (320, 240, 701),
                (641, 359, 702),
                (641, 359, 703),
                (320, 240, 704),
                (320, 240, 705),
                (641, 359, 706),
                (641, 359, 707),
            ] {
                assert!(case_tuple(w, h, nonce).is_ok());
            }
            for (w, h, nonce) in [
                (320, 240, 702),
                (641, 359, 700),
                (320, 240, 708),
                (321, 240, 700),
            ] {
                assert!(case_tuple(w, h, nonce).is_err());
            }
        }
    }
}
