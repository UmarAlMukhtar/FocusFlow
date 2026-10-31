//! X11 only: the opaque `hwnd` wire field contains an XID on Linux.
use super::*;
use std::sync::Arc;
use x11rb::{connection::Connection, protocol::{xproto::*, xinput::{self, ConnectionExt as _}, Event}, rust_connection::RustConnection};

fn error(e: impl std::fmt::Display) -> RecorderError { RecorderError::new("x11_unavailable", format!("X11 capture unavailable: {e}"), true) }
pub(super) fn connect() -> RecorderResult<(RustConnection, usize)> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
        return Err(RecorderError::new("wayland_unsupported", "Wayland capture requires a portal backend, which is not implemented yet. Use an X11 session for recording.", false));
    }
    x11rb::connect(None).map_err(error)
}
thread_local! { static CONNECTION: std::cell::RefCell<Option<(RustConnection, usize)>> = const { std::cell::RefCell::new(None) }; }
fn with_connection<T>(f: impl FnOnce(&RustConnection, u32) -> Option<T>) -> Option<T> {
    CONNECTION.with(|cell| {
        let mut connection = cell.borrow_mut();
        if connection.is_none() { *connection = connect().ok(); }
        let (c, screen) = connection.as_ref()?;
        f(c, c.setup().roots[*screen].root)
    })
}
pub(super) fn pointer() -> Option<QueryPointerReply> { with_connection(|c, root| c.query_pointer(root).ok()?.reply().ok()) }
pub(super) fn window_origin(id: usize) -> Option<(i32, i32)> {
    with_connection(|c, root| { let p = c.translate_coordinates(id.try_into().ok()?, root, 0, 0).ok()?.reply().ok()?; Some((p.dst_x.into(), p.dst_y.into())) })
}
pub(super) fn under_pointer() -> Option<usize> {
    with_connection(|c, root| {
        let mut id = c.query_pointer(root).ok()?.reply().ok()?.child;
        // EWMH client can be nested in window-manager frames.
        let clients = client_ids(c, root).ok()?;
        for _ in 0..16 { if clients.contains(&id) { return Some(id as usize); }
            let child = c.query_pointer(id).ok()?.reply().ok()?.child;
            if child == 0 { break; } id = child;
        }
        Some(id as usize)
    })
}
fn atom(c: &RustConnection, name: &[u8]) -> RecorderResult<u32> { c.intern_atom(false, name).map_err(error)?.reply().map(|r| r.atom).map_err(error) }
fn client_ids(c: &RustConnection, root: u32) -> RecorderResult<Vec<u32>> {
    let p = c.get_property(false, root, atom(c, b"_NET_CLIENT_LIST")?, AtomEnum::WINDOW, 0, 65536).map_err(error)?.reply().map_err(error)?;
    let clients: Vec<u32> = p.value32().map(|v| v.collect()).unwrap_or_default();
    if !clients.is_empty() { return Ok(clients); }
    // Minimal X11 window managers need not publish EWMH client lists.
    c.query_tree(root).map_err(error)?.reply().map(|r| r.children).map_err(error)
}
fn inspect(c: &RustConnection, root: u32, id: u32) -> RecorderResult<RecordableWindow> {
    let attr = c.get_window_attributes(id).map_err(error)?.reply().map_err(error)?;
    if attr.map_state != MapState::VIEWABLE || attr.override_redirect { return Err(error("Window is hidden, minimized, or not a recordable application")); }
    let types = c.get_property(false, id, atom(c, b"_NET_WM_WINDOW_TYPE")?, AtomEnum::ATOM, 0, 64).map_err(error)?.reply().map_err(error)?;
    if let Some(types) = types.value32() {
        let excluded = [atom(c, b"_NET_WM_WINDOW_TYPE_DESKTOP")?, atom(c, b"_NET_WM_WINDOW_TYPE_DOCK")?];
        if types.into_iter().any(|t| excluded.contains(&t)) { return Err(error("Desktop and panel windows cannot be recorded as application windows")); }
    }
    let pid = c.get_property(false, id, atom(c, b"_NET_WM_PID")?, AtomEnum::CARDINAL, 0, 1).map_err(error)?.reply().map_err(error)?;
    if pid.value32().and_then(|mut v| v.next()) == Some(std::process::id()) { return Err(error("FocusFlow windows cannot be recorded as application windows")); }
    let g = c.get_geometry(id).map_err(error)?.reply().map_err(error)?;
    let xy = c.translate_coordinates(id, root, 0, 0).map_err(error)?.reply().map_err(error)?;
    let mut title = String::new();
    for name in [b"_NET_WM_NAME".as_slice(), b"WM_NAME"] {
        let p = c.get_property(false, id, atom(c, name)?, AtomEnum::ANY, 0, 4096).map_err(error)?.reply().map_err(error)?;
        title = String::from_utf8_lossy(&p.value).trim_matches('\0').to_string();
        if !title.is_empty() { break; }
    }
    if title.is_empty() || title == "FocusFlow" { return Err(error("Window has no title or is FocusFlow")); }
    Ok(RecordableWindow { id: format!("0x{id:X}"), title, x: xy.dst_x.into(), y: xy.dst_y.into(), width: g.width.into(), height: g.height.into() })
}
pub(super) fn windows() -> RecorderResult<Vec<RecordableWindow>> {
    let (c, screen) = connect()?; let root = c.setup().roots[screen].root;
    let mut windows: Vec<_> = client_ids(&c, root)?.into_iter().filter_map(|id| inspect(&c, root, id).ok()).collect();
    windows.sort_by(|a,b| a.title.to_lowercase().cmp(&b.title.to_lowercase())); Ok(windows)
}
fn parse_id(value: &str) -> RecorderResult<u32> {
    let value = value.trim();
    let id = if let Some(hex) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) { u32::from_str_radix(hex, 16) } else { value.parse() }.map_err(error)?;
    if id == 0 { return Err(error("Window id must be nonzero")); } Ok(id)
}
pub(super) fn validate_region(b: CaptureBounds, width: u32, height: u32) -> RecorderResult<CaptureBounds> {
    let mut b = validated_capture_bounds(b.x, b.y, b.width, b.height)?;
    if b.x < 0 || b.y < 0 || u64::from(b.x as u32) + u64::from(b.width) > u64::from(width) || u64::from(b.y as u32) + u64::from(b.height) > u64::from(height) { return Err(error("Capture region is outside the X11 screen bounds")); }
    b.width &= !1; b.height &= !1; Ok(b)
}
pub(super) fn plan(app: &AppHandle, source: RecordingSource, output: &Path) -> RecorderResult<CapturePlan> {
    let (c, screen) = connect()?; let root = &c.setup().roots[screen];
    let mut window_capture = None;
    let bounds = match &source {
        RecordingSource::Screen => capture_bounds_from_monitor(&primary_monitor_info(app)?),
        RecordingSource::Region { x,y,width,height } => CaptureBounds { x:*x,y:*y,width:*width,height:*height },
        RecordingSource::Window { hwnd, .. } => {
            let id = parse_id(hwnd)?; let w = inspect(&c, root.root, id)?;
            let b = validated_capture_bounds(w.x,w.y,w.width & !1,w.height & !1)?;
            window_capture = Some(WindowCapturePlan { hwnd: id as usize, title:w.title, width:b.width,height:b.height }); b
        }
    };
    let bounds = if window_capture.is_none() { validate_region(bounds, root.width_in_pixels.into(),root.height_in_pixels.into())? } else { bounds };
    let monitor = MonitorInfo { name: window_capture.as_ref().map(|w|w.title.clone()).or(Some("X11 capture".into())), x:bounds.x,y:bounds.y,width:bounds.width,height:bounds.height,scale_factor:1.0 };
    let ffmpeg_args = args(bounds, window_capture.as_ref().map(|w|w.hwnd as u32), output)?;
    let source = match (source, window_capture.as_ref()) {
        (RecordingSource::Window { .. }, Some(w)) => RecordingSource::Window { hwnd: format!("0x{:X}", w.hwnd), title: w.title.clone() },
        (RecordingSource::Region { .. }, _) => RecordingSource::Region { x:bounds.x,y:bounds.y,width:bounds.width,height:bounds.height },
        (source, _) => source,
    };
    Ok(CapturePlan { source,monitor,bounds,backend:CaptureBackend::Ffmpeg,ffmpeg_args,window_capture })
}
pub(super) fn args(bounds: CaptureBounds, window: Option<u32>, output: &Path) -> RecorderResult<Vec<String>> {
    let display = std::env::var("DISPLAY").map_err(error)?;
    let mut args = vec!["-hide_banner".into(),"-loglevel".into(),"info".into(),"-f".into(),"x11grab".into(),"-framerate".into(),TARGET_FPS.to_string(),"-video_size".into(),format!("{}x{}",bounds.width,bounds.height),"-draw_mouse".into(),"1".into()];
    if let Some(id) = window { args.extend(["-window_id".into(),id.to_string()]); }
    else { args.extend(["-grab_x".into(),bounds.x.to_string(),"-grab_y".into(),bounds.y.to_string()]); }
    args.extend(["-i".into(),display,"-r".into(),TARGET_FPS.to_string(),"-t".into(),"86400".into(),"-an".into(),"-c:v".into(),"libx264".into(),"-preset".into(),"veryfast".into(),"-crf".into(),"23".into(),"-pix_fmt".into(),"yuv420p".into(),"-movflags".into(),"+faststart".into(),"-y".into(),path_to_string(output)?]); Ok(args)
}
pub(super) fn track(stop: Arc<AtomicBool>, clicks: Arc<Mutex<Vec<ClickEvent>>>, drags: Arc<Mutex<Vec<DragInteraction>>>, started: Instant, filter: ClickFilter) {
    let result = (|| -> RecorderResult<()> {
        let (c, screen) = connect()?; let root = c.setup().roots[screen].root;
        c.xinput_xi_query_version(2,0).map_err(error)?.reply().map_err(error)?;
        c.xinput_xi_select_events(root, &[xinput::EventMask { deviceid: xinput::Device::ALL_MASTER.into(), mask: vec![xinput::XIEventMask::RAW_BUTTON_PRESS | xinput::XIEventMask::RAW_BUTTON_RELEASE] }]).map_err(error)?.check().map_err(error)?;
        c.flush().map_err(error)?;
        let mut left = None; let mut right = None;
        while !stop.load(Ordering::Relaxed) {
            while let Some(event) = c.poll_for_event().map_err(error)? {
                let (button, down) = match event { Event::XinputRawButtonPress(e) => (e.detail,true), Event::XinputRawButtonRelease(e) => (e.detail,false), _ => continue };
                let (drag, button) = match button { 1 => (&mut left,ClickButton::Left), 3 => (&mut right,ClickButton::Right), _ => continue };
                // XI2 preserves brief presses; coordinates/time are delivery-time samples.
                if down { *drag = push_click(&clicks,started,button,&filter).map(|p|PendingDrag::new(button,p)); }
                else { finish_pending_drag(drag,&drags,started,&filter); }
            }
            update_pending_drag(&mut left,started,&filter); update_pending_drag(&mut right,started,&filter);
            thread::sleep(CLICK_POLL_INTERVAL);
        }
        finish_pending_drag(&mut left,&drags,started,&filter); finish_pending_drag(&mut right,&drags,started,&filter);
        Ok(())
    })();
    if let Err(error) = result { eprintln!("[FocusFlow input] Interactions unavailable: {error}"); }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn validates_x11_regions_and_even_sizes() {
        assert!(validate_region(CaptureBounds{x:-1,y:0,width:100,height:100},1920,1200).is_err());
        assert!(validate_region(CaptureBounds{x:1900,y:0,width:100,height:100},1920,1200).is_err());
        let b=validate_region(CaptureBounds{x:30,y:40,width:101,height:99},1920,1200).unwrap(); assert_eq!((b.width,b.height),(100,98));
        assert!(parse_id("0").is_err()); assert!(parse_id("0x100000000").is_err()); assert_eq!(parse_id("0xAB").unwrap(),171);
    }
}

pub(super) fn source_alive(id: &str, width: u32, height: u32) -> RecorderResult<bool> {
    let id = parse_id(id)?;
    // Admission metadata may change during capture; only drawable availability bounds liveness.
    with_connection(|c, _| Some((|| {
        let attr = match c.get_window_attributes(id).map_err(error)?.reply() {
            Ok(attr) => attr,
            Err(x11rb::errors::ReplyError::X11Error(reply)) if reply.error_kind == x11rb::protocol::ErrorKind::Window => return Ok(false),
            Err(reply) => return Err(error(reply)),
        };
        if attr.map_state != MapState::VIEWABLE { return Ok(false); }
        let geometry = c.get_geometry(id).map_err(error)?.reply().map_err(error)?;
        Ok(u32::from(geometry.width) >= width && u32::from(geometry.height) >= height)
    })())).unwrap_or_else(||Err(error("X11 source availability could not be queried")))
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use x11rb::wrapper::ConnectionExt as _;

    fn assert_reaped(pid: u32) {
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists(), "encoder {pid} was not reaped");
    }

    /// Runs only when explicitly requested on a disposable X server.
    #[test]
    #[ignore = "requires an isolated X11 DISPLAY and installed FFmpeg; generates native capture evidence"]
    fn native_capture_fixture() {
        assert_eq!(std::env::var("FOCUSFLOW_ISOLATED_X11").as_deref(), Ok("1"));
        let ffmpeg = PathBuf::from(std::env::var("FOCUSFLOW_FFMPEG").expect("absolute FFmpeg executable"));
        let guardian = PathBuf::from(std::env::var("FOCUSFLOW_NATIVE_GUARDIAN").expect("absolute built FocusFlow executable"));
        let output = PathBuf::from(std::env::var("FOCUSFLOW_NATIVE_EVIDENCE").expect("evidence directory"));
        fs::create_dir_all(&output).unwrap();
        let (c, screen_index) = connect().unwrap();
        let screen = &c.setup().roots[screen_index];
        let window = c.generate_id().unwrap();
        c.create_window(x11rb::COPY_DEPTH_FROM_PARENT, window, screen.root, 120, 100, 640, 480, 0,
            WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().background_pixel(0x22aa66)).unwrap().check().unwrap();
        c.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, b"FocusFlow generated native fixture").unwrap().check().unwrap();
        c.map_window(window).unwrap().check().unwrap(); c.flush().unwrap();
        thread::sleep(Duration::from_millis(100));
        let w = inspect(&c,screen.root,window).unwrap();
        assert_eq!((w.width,w.height),(640,480));
        let root_bounds = CaptureBounds { x:0,y:0,width: u32::from(screen.width_in_pixels) & !1,height:u32::from(screen.height_in_pixels) & !1 };
        let region = CaptureBounds { x:140,y:120,width:320,height:240 };
        let window_bounds = CaptureBounds { x:120,y:100,width:640,height:480 };
        let mut receipts = Vec::new();
        for cycle in 0..2 {
            for (name, bounds, id) in [("screen",root_bounds,None),("region",region,None),("window",window_bounds,Some(window))] {
                let file = output.join(format!("{name}-{cycle}.mp4"));
                let log = Arc::new(Mutex::new(ProcessLog::default()));
                let mut capture = spawn_ffmpeg_capture_with_guardian(Command::new(&ffmpeg), &output, &args(bounds,id,&file).unwrap(), Arc::clone(&log), &guardian).unwrap();
                let pid = capture.pid().unwrap();
                thread::sleep(Duration::from_millis(650));
                let ActiveCapture::Ffmpeg { child, .. } = &mut capture;
                assert!(child.try_wait().unwrap().is_none(), "capture exited early: {name}");
                if id.is_some() {
                    c.configure_window(window,&ConfigureWindowAux::new().x(200).y(150)).unwrap().check().unwrap(); c.flush().unwrap();
                    assert_eq!(window_origin(window as usize),Some((200,150)));
                    thread::sleep(Duration::from_millis(300));
                }
                tauri::async_runtime::block_on(stop_capture_backend(capture, &log)).unwrap();
                assert_reaped(pid);
                fs::write(output.join(format!("{name}-{cycle}.log")), read_log_text(&log)).unwrap();
                let probe = focusflow_core::ffmpeg::probe_video(&ffmpeg,&file).unwrap();
                assert_eq!((probe.width,probe.height),(bounds.width,bounds.height));
                assert!(probe.duration_us > 0);
                receipts.push(format!("{name}-{cycle}: pid={pid} reaped=true width={} height={} duration_us={}",probe.width,probe.height,probe.duration_us));
                c.configure_window(window,&ConfigureWindowAux::new().x(120).y(100)).unwrap().check().unwrap(); c.flush().unwrap();
            }
        }
        // Real XI2 delivery with injected input; this is not physical-device proof.
        let tracker = ClickTracker::start(Instant::now(), ClickFilter::Window { hwnd:window as usize,encoder_width:640,encoder_height:480 });
        thread::sleep(Duration::from_millis(150));
        assert!(Command::new("xdotool").args(["mousemove","200","180","mousedown","1","mouseup","1"]).status().unwrap().success());
        thread::sleep(Duration::from_millis(100));
        assert!(Command::new("xdotool").args(["mousemove","220","200","mousedown","1"]).status().unwrap().success());
        thread::sleep(Duration::from_millis(100));
        assert!(Command::new("xdotool").args(["mousemove","300","260"]).status().unwrap().success());
        thread::sleep(Duration::from_millis(100));
        assert!(Command::new("xdotool").args(["mouseup","1"]).status().unwrap().success());
        thread::sleep(Duration::from_millis(100));
        tracker.request_stop();
        let interactions = tracker.finish().unwrap();
        assert_eq!(interactions.clicks.len(),2,"brief XI2 press must be captured once");
        assert_eq!((interactions.clicks[0].x,interactions.clicks[0].y),(80,80));
        assert_eq!(interactions.drags.len(),1);
        assert!(interactions.drags[0].points.iter().all(|p|p.x>=0 && p.x<640 && p.y>=0 && p.y<480));
        assert_eq!((interactions.drags[0].points.last().unwrap().x,interactions.drags[0].points.last().unwrap().y),(180,160));
        fs::write(output.join("injected-clicks.json"),serde_json::to_vec_pretty(&interactions.clicks).unwrap()).unwrap();
        fs::write(output.join("injected-drags.json"),serde_json::to_vec_pretty(&interactions.drags).unwrap()).unwrap();
        for action in ["unmapped", "shrunk"] {
            let path = output.join(format!("window-{action}.mp4"));
            let log = Arc::new(Mutex::new(ProcessLog::default()));
            let capture = spawn_ffmpeg_capture_with_guardian(Command::new(&ffmpeg), &output, &args(window_bounds, Some(window), &path).unwrap(), Arc::clone(&log), &guardian).unwrap();
            let pid = capture.pid().unwrap();
            thread::sleep(Duration::from_millis(700));
            if action == "unmapped" { c.unmap_window(window).unwrap().check().unwrap(); }
            else { c.configure_window(window,&ConfigureWindowAux::new().width(300).height(200)).unwrap().check().unwrap(); }
            c.flush().unwrap();
            assert!(!source_alive(&w.id,640,480).unwrap());
            let stop = tauri::async_runtime::block_on(stop_capture_backend(capture, &log));
            assert_reaped(pid);
            let probe = focusflow_core::ffmpeg::probe_video(&ffmpeg,&path).unwrap();
            receipts.push(format!("window-{action}: pid={pid} reaped=true duration_us={} stop={stop:?}",probe.duration_us));
            fs::write(output.join(format!("window-{action}.log")),read_log_text(&log)).unwrap();
            c.configure_window(window,&ConfigureWindowAux::new().width(640).height(480)).unwrap().check().unwrap();
            c.map_window(window).unwrap().check().unwrap(); c.flush().unwrap();
        }
        c.configure_window(window,&ConfigureWindowAux::new().width(300).height(200)).unwrap().check().unwrap(); c.flush().unwrap();
        assert!(!source_alive(&w.id,640,480).unwrap());
        c.unmap_window(window).unwrap().check().unwrap(); c.flush().unwrap();
        assert!(inspect(&c,screen.root,window).is_err());
        c.configure_window(window,&ConfigureWindowAux::new().width(640).height(480)).unwrap().check().unwrap();
        c.map_window(window).unwrap().check().unwrap(); c.flush().unwrap();
        let close_file = output.join("window-closed.mp4");
        let log = Arc::new(Mutex::new(ProcessLog::default()));
        let capture = spawn_ffmpeg_capture_with_guardian(Command::new(&ffmpeg), &output, &args(window_bounds, Some(window), &close_file).unwrap(), Arc::clone(&log), &guardian).unwrap();
        let close_pid = capture.pid().unwrap();
        thread::sleep(Duration::from_millis(700));
        c.destroy_window(window).unwrap().check().unwrap(); c.flush().unwrap();
        assert!(!source_alive(&w.id,640,480).unwrap());
        // Production stop always waits/reaps, even after the drawable disappears.
        let close_result = tauri::async_runtime::block_on(stop_capture_backend(capture, &log));
        assert_reaped(close_pid);
        let probe = focusflow_core::ffmpeg::probe_video(&ffmpeg, &close_file).unwrap();
        receipts.push(format!("window-closed: pid={close_pid} reaped=true duration_us={} stop={close_result:?}", probe.duration_us));
        fs::write(output.join("window-closed.log"),read_log_text(&log)).unwrap();
        for encoder in ["/usr/bin/false", "/usr/bin/true"] {
            let fail = spawn_ffmpeg_capture_with_guardian(Command::new(encoder), &output, &[], Arc::new(Mutex::new(ProcessLog::default())), &guardian);
            assert!(fail.is_err(), "early child exit must not become a recording");
        }
        let recovery = output.join("screen-recovered.mp4");
        let log = Arc::new(Mutex::new(ProcessLog::default()));
        let capture = spawn_ffmpeg_capture_with_guardian(Command::new(&ffmpeg), &output, &args(root_bounds,None,&recovery).unwrap(),Arc::clone(&log), &guardian).unwrap();
        let pid = capture.pid().unwrap();
        thread::sleep(Duration::from_millis(600));
        let state = RecorderState::new();
        let reservation = StartReservation::acquire(&state).unwrap();
        use std::{future::Future, task::{Context, Poll, Waker}};
        let mut closing = Box::pin(shutdown(&state));
        assert!(matches!(closing.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
        // Exit has latched while startup still owns work; publish as a real startup does.
        state.lock_runtime().unwrap().active = Some(ActiveRecording {
            native_capture:None,notice:None,
            capture, video_clock: video_clock::Clock::start(video_clock::Capability::probe(&ffmpeg), Instant::now()), audio_recorder: None, mic_device_name: None,
            click_tracker: ClickTracker::start(Instant::now(), ClickFilter::All),
            output_path: recovery.clone(), started_at: Instant::now(),
            monitor: MonitorInfo { name:Some("Generated X11 fixture".into()),x:0,y:0,width:root_bounds.width,height:root_bounds.height,scale_factor:1.0 },
            capture_bounds: root_bounds, source: RecordingSource::Screen,
            ffmpeg_path: ffmpeg.clone(), log,
        });
        drop(reservation);
        tauri::async_runtime::block_on(closing);
        assert_reaped(pid);
        let runtime = state.lock_runtime().unwrap();
        assert!(runtime.shutting_down && !runtime.starting && !runtime.finalizing && runtime.active.is_none());
        assert!(runtime.last_completed.is_some());
        drop(runtime);
        assert_eq!(StartReservation::acquire(&state).err().unwrap().code,"recorder_shutting_down");
        let probe = focusflow_core::ffmpeg::probe_video(&ffmpeg,&recovery).unwrap();
        receipts.push(format!("screen-recovered after closed source and startup failure; shutdown awaited startup publication and finalized: pid={pid} reaped=true duration_us={}",probe.duration_us));
        receipts.push("injected brief clicks=2; drags=1; live movement transform=true; shrink/unmap/closed rejected=true".into());
        fs::write(output.join("receipts.txt"),receipts.join("\n")).unwrap();
    }
}
