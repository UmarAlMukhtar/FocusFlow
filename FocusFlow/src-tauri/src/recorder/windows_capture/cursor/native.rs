//! Owned Win32 sampling. The input path has no DWM, GDI, file or serialization work.
use super::super::process::{frequency, qpc, SourceOwner};
use super::image_native::{decode, Icon};
use super::storage::{Cadence, CursorObservationCompletion, Streams};
use super::*;
use focusflow_core::cursor_observation::*;
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{GetLastError, SetLastError, HWND, POINT, RECT, WIN32_ERROR},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::{
            ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
    },
    UI::{
        HiDpi::*,
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON},
        WindowsAndMessaging::*,
    },
};
const INPUT_PERIOD: Duration = Duration::from_millis(8);
const POSE_PERIOD: Duration = Duration::from_nanos(16_666_667);
const QUEUE_CAP: usize = 64;
struct DpiScope(DPI_AWARENESS_CONTEXT, bool);
impl DpiScope {
    fn enter() -> Result<Self, String> {
        // This thread alone changes context; the original opaque context is restored on every exit.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err("Cannot establish observer PMv2 context".into());
        }
        let scope = Self(previous, false);
        if !unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        }
        .as_bool()
        {
            return Err("Observer PMv2 confirmation failed".into());
        }
        Ok(scope)
    }
    fn restore(mut self) -> Result<(), String> {
        let restored = unsafe { SetThreadDpiAwarenessContext(self.0) };
        if restored.0.is_null()
            || !unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), self.0) }
                .as_bool()
        {
            return Err("Observer DPI restoration failed".into());
        }
        self.1 = true;
        Ok(())
    }
}
impl Drop for DpiScope {
    fn drop(&mut self) {
        if !self.1 {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
}
fn hwnd(n: u64) -> Result<HWND, String> {
    Ok(HWND(
        usize::try_from(n).map_err(|e| e.to_string())? as *mut _
    ))
}
fn rect(r: RECT) -> [i32; 4] {
    [r.left, r.top, r.right, r.bottom]
}
fn geometry(window: u64) -> Result<WindowObservation, String> {
    let h = hwnd(window)?;
    let mut client = RECT::default();
    let mut frame = RECT::default();
    let mut dwm = RECT::default();
    let mut origin = POINT::default();
    let mut pid = 0;
    // All outputs are initialized local storage; HWND is borrowed and never destroyed.
    unsafe {
        if GetAncestor(h, GA_ROOT) != h {
            return Err("Cursor source is not top-level".into());
        }
        GetClientRect(h, &mut client).map_err(|e| e.to_string())?;
        GetWindowRect(h, &mut frame).map_err(|e| e.to_string())?;
        DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut dwm as *mut _ as _,
            std::mem::size_of::<RECT>() as u32,
        )
        .map_err(|e| e.to_string())?;
        if !ClientToScreen(h, &mut origin).as_bool() {
            return Err("Physical client origin unavailable".into());
        }
        let tid = GetWindowThreadProcessId(h, Some(&mut pid));
        if tid == 0 || pid == 0 {
            return Err("Cursor source identity unavailable".into());
        }
        let style = |key| -> Result<u32, String> {
            SetLastError(WIN32_ERROR(0));
            let s = GetWindowLongW(h, key);
            if s == 0 && GetLastError() != WIN32_ERROR(0) {
                Err("Source style query failed".into())
            } else {
                Ok(s as u32)
            }
        };
        let monitor = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return Err("Monitor geometry unavailable".into());
        }
        let size = |a: i32, b: i32| {
            u32::try_from(i64::from(b) - i64::from(a))
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| "Invalid current client extent".to_owned())
        };
        Ok(WindowObservation {
            hwnd: window.to_string(),
            pid,
            tid,
            client_origin: [origin.x, origin.y],
            client_size: [
                size(client.left, client.right)?,
                size(client.top, client.bottom)?,
            ],
            window_rect: rect(frame),
            dwm_rect: rect(dwm),
            dpi: GetDpiForWindow(h),
            observer_pmv2: AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
            .as_bool(),
            target_pmv2: AreDpiAwarenessContextsEqual(
                GetWindowDpiAwarenessContext(h),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
            .as_bool(),
            style: style(GWL_STYLE)?,
            ex_style: style(GWL_EXSTYLE)?,
            visible: IsWindowVisible(h).as_bool(),
            minimized: IsIconic(h).as_bool(),
            monitor_rect: rect(info.rcMonitor),
            monitor_count: u32::try_from(GetSystemMetrics(SM_CMONITORS)).unwrap_or(0),
        })
    }
}
fn pointer() -> Result<PointerObservation, String> {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    let mut p = POINT::default();
    // CURSORINFO's cursor is borrowed; CopyIcon below creates the only owned image handle.
    unsafe {
        GetCursorInfo(&mut info).map_err(|e| e.to_string())?;
        GetPhysicalCursorPos(&mut p).map_err(|e| e.to_string())?;
        let hit = WindowFromPhysicalPoint(p);
        let root = GetAncestor(hit, GA_ROOT);
        let mut pid = 0;
        let tid = GetWindowThreadProcessId(hit, Some(&mut pid));
        Ok(PointerObservation {
            physical_position: [p.x, p.y],
            cursor_position: [info.ptScreenPos.x, info.ptScreenPos.y],
            flags: info.flags.0,
            handle: (info.hCursor.0 as usize as u64).to_string(),
            hit_hwnd: (hit.0 as usize as u64).to_string(),
            root_hwnd: (root.0 as usize as u64).to_string(),
            hit_pid: pid,
            hit_tid: tid,
        })
    }
}
#[derive(Clone, Copy)]
struct Basis {
    row: u32,
    q1: i64,
    revision: u64,
}
struct Shared {
    stop: AtomicBool,
    failed: AtomicBool,
    revision: AtomicU64,
    basis: Mutex<Option<Basis>>,
}
impl Shared {
    fn fail(&self) {
        self.failed.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
}
struct InputSample {
    q: [i64; 2],
    position: [i32; 2],
    hit: u64,
    root: u64,
    pid: u32,
    tid: u32,
    swapped: bool,
    left: bool,
    right: bool,
    basis: Option<u32>,
    revision: [u64; 2],
}
impl InputSample {
    fn row(self) -> WindowsInputRowV1 {
        WindowsInputRowV1::Observed {
            q0: self.q[0].to_string(),
            q1: self.q[1].to_string(),
            physical_position: self.position,
            hit_hwnd: self.hit.to_string(),
            root_hwnd: self.root.to_string(),
            hit_pid: self.pid,
            hit_tid: self.tid,
            buttons_swapped: self.swapped,
            left: Some(self.left),
            right: Some(self.right),
            basis_pose_row: self.basis,
            barrier_revision_before: self.revision[0],
            barrier_revision_after: self.revision[1],
        }
    }
}
// This is deliberately the complete light input boundary: no image, DWM, disk or serialization call.
fn input_sample(shared: &Shared, freq: i64) -> Result<InputSample, String> {
    let revision = shared.revision.load(Ordering::SeqCst);
    let basis = *shared.basis.lock().map_err(|_| "Input basis poisoned")?;
    let q0 = qpc()?;
    let mut p = POINT::default();
    let (hit, root, pid, tid, swapped, left, right) = unsafe {
        GetPhysicalCursorPos(&mut p).map_err(|e| e.to_string())?;
        let hit = WindowFromPhysicalPoint(p);
        let root = GetAncestor(hit, GA_ROOT);
        let mut pid = 0;
        let tid = GetWindowThreadProcessId(hit, Some(&mut pid));
        (
            hit.0 as usize as u64,
            root.0 as usize as u64,
            pid,
            tid,
            GetSystemMetrics(SM_SWAPBUTTON) != 0,
            GetAsyncKeyState(i32::from(VK_LBUTTON.0)) < 0,
            GetAsyncKeyState(i32::from(VK_RBUTTON.0)) < 0,
        )
    };
    let q1 = qpc()?;
    let after = shared.revision.load(Ordering::SeqCst);
    let basis = basis
        .filter(|b| {
            b.revision == revision
                && revision == after
                && b.q1 <= q0
                && i128::from(q1 - b.q1) * 30 <= i128::from(freq)
        })
        .map(|b| b.row);
    Ok(InputSample {
        q: [q0, q1],
        position: [p.x, p.y],
        hit,
        root,
        pid,
        tid,
        swapped,
        left,
        right,
        basis,
        revision: [revision, after],
    })
}
struct InputEnd {
    stats: Cadence,
    barrier: Option<QpcBarrier>,
}
fn failure_barrier(start: Option<i64>, reason: &str) -> Option<QpcBarrier> {
    let q0 = start?;
    let q1 = qpc().ok()?;
    if q1 < q0 {
        return None;
    }
    Some(QpcBarrier {
        q0: q0.to_string(),
        q1: q1.to_string(),
        reason: reason.into(),
    })
}
fn terminal_barrier(reason: &str) -> Option<QpcBarrier> {
    qpc().ok().map(|q| QpcBarrier {
        q0: q.to_string(),
        q1: q.to_string(),
        reason: reason.into(),
    })
}
fn input_loop(shared: Arc<Shared>, tx: SyncSender<InputSample>, freq: i64) -> InputEnd {
    let mut stats = Cadence::default();
    let mut barrier = None;
    while !shared.stop.load(Ordering::SeqCst) {
        let began = qpc().ok();
        let sample = match input_sample(&shared, freq) {
            Ok(s) => s,
            Err(_) => {
                barrier = failure_barrier(began, "Input query failed");
                shared.fail();
                break;
            }
        };
        stats.observe(sample.q);
        let bracket = sample.q;
        match offer_sample(&tx, sample, &shared.stop) {
            Ok(()) => {}
            Err(OfferFailure::Stopped) => {
                barrier = Some(QpcBarrier {
                    q0: bracket[0].to_string(),
                    q1: bracket[1].to_string(),
                    reason: "Input sampling stopped".into(),
                });
                break;
            }
            Err(OfferFailure::Overflow) => {
                stats.overflow = true;
                barrier = Some(QpcBarrier {
                    q0: bracket[0].to_string(),
                    q1: bracket[1].to_string(),
                    reason: "Input queue overflow or lost consumer".into(),
                });
                shared.fail();
                break;
            }
        }
        thread::park_timeout(INPUT_PERIOD);
    }
    InputEnd {
        stats,
        barrier: barrier.or_else(|| {
            terminal_barrier(if shared.failed.load(Ordering::SeqCst) {
                "Input sampler ended after failure"
            } else {
                "Input sampler stopped"
            })
        }),
    }
}
struct PoseEnd {
    streams: Streams,
    stats: Cadence,
    barrier: Option<QpcBarrier>,
    degraded: bool,
    kind: StopKind,
    usable: bool,
}
fn pose_loop(
    owner: SourceOwner,
    window: u64,
    pid: u32,
    profile: ModelProfile,
    raw: [u32; 2],
    freq: i64,
    shared: Arc<Shared>,
    rx: Receiver<InputSample>,
    mut streams: Streams,
) -> PoseEnd {
    let mut stats = Cadence::default();
    let mut degraded = false;
    let mut usable = true;
    let mut kind = StopKind::Stop;
    let mut previous = None;
    let mut barrier = None;
    let write_inputs = |streams: &mut Streams| -> Result<(), String> {
        for _ in 0..QUEUE_CAP {
            match rx.try_recv() {
                Ok(s) => streams.input(&s.row())?,
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    };
    while !shared.stop.load(Ordering::SeqCst) {
        let began = qpc().ok();
        let observed =
            (|| -> Result<(WindowsCursorRowV2, Option<Icon>, bool, WindowObservation), String> {
                owner.require_available()?;
                let scope = DpiScope::enter()?;
                let q0 = qpc()?;
                let before = geometry(window)?;
                let a = pointer()?;
                let copied = if a.flags == 1 && a.handle != "0" {
                    unsafe {
                        CopyIcon(HICON(
                            a.handle.parse::<u64>().map_err(|e| e.to_string())? as usize as *mut _,
                        ))
                    }
                    .ok()
                    .map(Icon)
                } else {
                    None
                };
                let b = pointer()?;
                let after = geometry(window)?;
                let q1 = qpc()?;
                scope.restore()?;
                owner.require_available()?;
                let good = before == after
                    && before.pid == pid
                    && candidate_family(&before, raw) == Some(profile)
                    && a == b
                    && a.flags == 1
                    && a.handle != "0"
                    && a.physical_position == a.cursor_position
                    && a.root_hwnd == window.to_string()
                    && a.hit_pid == pid
                    && a.hit_tid == before.tid
                    && a.hit_hwnd != "0"
                    && (0..i64::from(raw[0])).contains(
                        &(i64::from(a.physical_position[0]) - i64::from(before.dwm_rect[0])),
                    )
                    && (0..i64::from(raw[1])).contains(
                        &(i64::from(a.physical_position[1]) - i64::from(before.dwm_rect[1])),
                    )
                    && q1 >= q0
                    && i128::from(q1 - q0) * 30 <= i128::from(freq);
                let changed = previous.as_ref().is_some_and(|p| p != &before);
                if !good || changed {
                    shared.revision.fetch_add(1, Ordering::SeqCst);
                }
                let revision = shared.revision.load(Ordering::SeqCst);
                let row = WindowsCursorRowV2::Observed {
                    q0: q0.to_string(),
                    q1: q1.to_string(),
                    before: before.clone(),
                    after,
                    pointer_before: a,
                    pointer_after: b,
                    shape_id: None,
                    barrier_revision: revision,
                };
                Ok((row, copied, good, before))
            })();
        match observed {
            Ok((mut row, copy, good, geometry)) => {
                let bracket = match row.bracket() {
                    Ok(b) => b,
                    Err(_) => {
                        usable = false;
                        degraded = true;
                        shared.fail();
                        break;
                    }
                };
                stats.observe(bracket);
                if let Some(copy) = copy {
                    match decode(&copy).and_then(|shape| streams.shape(shape)) {
                        Ok(id) => {
                            if let WindowsCursorRowV2::Observed { shape_id, .. } = &mut row {
                                *shape_id = Some(id)
                            }
                        }
                        Err(_) => degraded = true,
                    }
                } else if good {
                    degraded = true;
                }
                match streams.pose(&row) {
                    Ok(index) => {
                        if let Ok(mut basis) = shared.basis.lock() {
                            *basis = good.then_some(Basis {
                                row: index,
                                q1: bracket[1],
                                revision: shared.revision.load(Ordering::SeqCst),
                            });
                        } else {
                            shared.fail();
                        }
                        previous = Some(geometry);
                    }
                    Err(e) => {
                        kind = if e.contains("budget") {
                            StopKind::Budget
                        } else {
                            StopKind::SamplerFailure
                        };
                        degraded = true;
                        usable &= kind == StopKind::Budget;
                        barrier = Some(QpcBarrier {
                            q0: bracket[0].to_string(),
                            q1: bracket[1].to_string(),
                            reason: "Pose storage stopped".into(),
                        });
                        shared.fail();
                    }
                }
            }
            Err(_) => {
                degraded = true;
                barrier = failure_barrier(began, "Pose query failed");
                usable &= barrier.is_some();
                shared.fail();
                kind = StopKind::SamplerFailure;
            }
        }
        if let Err(e) = write_inputs(&mut streams) {
            degraded = true;
            kind = if e.contains("budget") {
                StopKind::Budget
            } else {
                StopKind::SamplerFailure
            };
            usable &= kind == StopKind::Budget;
            shared.fail();
        }
        if !shared.stop.load(Ordering::SeqCst) {
            thread::park_timeout(POSE_PERIOD);
        }
    }
    // Both lanes end together; no queue overflow ever resumes a stale button segment.
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match rx.recv_timeout(Duration::from_millis(8)) {
            Ok(sample) => {
                if let Err(e) = streams.input(&sample.row()) {
                    if !e.contains("budget") {
                        usable = false;
                    }
                    degraded = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= drain_deadline {
                    usable = false;
                    degraded = true;
                    break;
                }
            }
        }
    }
    if shared.failed.load(Ordering::SeqCst) && kind != StopKind::Budget {
        kind = StopKind::SamplerFailure;
        degraded = true;
    }
    PoseEnd {
        streams,
        stats,
        barrier: barrier.or_else(|| terminal_barrier("Pose sampler stopped")),
        degraded,
        kind,
        usable,
    }
}
pub(in crate::recorder::windows_capture) struct CursorObserver {
    window: u64,
    pid: u32,
    profile: ModelProfile,
    raw: [u32; 2],
    frequency: i64,
    owner: Option<SourceOwner>,
    streams: Option<Streams>,
    shared: Arc<Shared>,
    gate: ArmGate,
    pose: Option<JoinHandle<PoseEnd>>,
    input: Option<JoinHandle<InputEnd>>,
}
impl CursorObserver {
    pub fn prepare(window: u64, pid: u32) -> Result<Self, String> {
        let value = Self::candidate(window, pid)?;
        if !release_qualified(value.profile) {
            return Err("Sampled cursor tracking is not yet qualified for this window; recording uses embedded cursor pixels".into());
        }
        Ok(value)
    }
    fn candidate(window: u64, pid: u32) -> Result<Self, String> {
        let owner = SourceOwner::prepare(window, pid)?;
        let scope = DpiScope::enter()?;
        let w = geometry(window)?;
        scope.restore()?;
        owner.require_available()?;
        let raw = [
            u32::try_from(i64::from(w.dwm_rect[2]) - i64::from(w.dwm_rect[0]))
                .map_err(|e| e.to_string())?,
            u32::try_from(i64::from(w.dwm_rect[3]) - i64::from(w.dwm_rect[1]))
                .map_err(|e| e.to_string())?,
        ];
        let profile = candidate_family(&w, raw)
            .ok_or("Current window is outside the qualified cursor model family")?;
        Ok(Self {
            window,
            pid,
            profile,
            raw,
            frequency: frequency()?,
            owner: Some(owner),
            streams: None,
            shared: Arc::new(Shared {
                stop: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                revision: AtomicU64::new(0),
                basis: Mutex::new(None),
            }),
            gate: ArmGate::default(),
            pose: None,
            input: None,
        })
    }
    #[cfg(test)]
    pub(in crate::recorder) fn prepare_fixture(
        window: u64,
        pid: u32,
        permission:&crate::recorder::windows_qualification::production_cursor::OwnedFixturePermission,
    ) -> Result<Self, String> {
        permission.check(window, pid)?;
        let candidate = Self::candidate(window, pid)?;
        if candidate.profile != permission.profile() {
            return Err("Fixture model family mismatch".into());
        }
        Ok(candidate)
    }
    pub fn profile(&self) -> ModelProfile {
        self.profile
    }
    pub fn configure(&mut self, dir: &Path, raw: [u32; 2]) -> Result<(), String> {
        if self.gate.sampling_allowed() || self.streams.is_some() || self.raw != raw {
            return Err("Cursor model does not match the frozen raw media grid".into());
        }
        self.streams = Some(Streams::create(dir)?);
        Ok(())
    }
    pub fn start(&mut self, admission_qpc: i64) -> Result<(), String> {
        self.owner
            .as_ref()
            .ok_or("Cursor source owner missing")?
            .require_available()?;
        let scope = DpiScope::enter()?;
        let now = geometry(self.window)?;
        scope.restore()?;
        self.gate.arm(
            candidate_family(&now, self.raw) == Some(self.profile) && now.pid == self.pid,
            admission_qpc,
        )?;
        let streams = self
            .streams
            .take()
            .ok_or("Cursor streams were not prepared")?;
        let owner = self.owner.take().ok_or("Cursor owner already consumed")?;
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAP);
        let shared = self.shared.clone();
        let frequency = self.frequency;
        self.input = Some(
            thread::Builder::new()
                .name("wgc-cursor-input".into())
                .spawn(move || input_loop(shared, tx, frequency))
                .map_err(|e| e.to_string())?,
        );
        let shared = self.shared.clone();
        let (window, pid, profile, raw) = (self.window, self.pid, self.profile, self.raw);
        self.pose = Some(
            thread::Builder::new()
                .name("wgc-cursor-pose".into())
                .spawn(move || {
                    pose_loop(
                        owner, window, pid, profile, raw, frequency, shared, rx, streams,
                    )
                })
                .map_err(|e| {
                    self.shared.fail();
                    e.to_string()
                })?,
        );
        Ok(())
    }
    pub fn request_stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for t in [
            self.pose.as_ref().map(|t| t.thread()),
            self.input.as_ref().map(|t| t.thread()),
        ]
        .into_iter()
        .flatten()
        {
            t.unpark();
        }
    }
    pub fn finish(mut self, deadline: Instant) -> Result<CursorObservationCompletion, String> {
        self.request_stop();
        let input = join_before(&mut self.input, deadline)?;
        let pose = join_before(&mut self.pose, deadline)?;
        let trustworthy_stop = pose.barrier.is_some() && input.barrier.is_some();
        let terminal = ObservationTerminal {
            pose_barrier: pose.barrier,
            input_barrier: input.barrier,
            source_cutoff_qpc: 0,
            stop_kind: pose.kind,
        };
        let mut completion = pose.streams.finish(
            self.profile,
            terminal,
            pose.stats,
            input.stats,
            pose.degraded,
        )?;
        completion.pose_joined = true;
        completion.input_joined = true;
        completion.files_usable &= pose.usable && trustworthy_stop;
        if !completion.files_usable {
            completion.degraded = true;
        }
        Ok(completion)
    }
}
impl Drop for CursorObserver {
    fn drop(&mut self) {
        self.request_stop();
    }
}
