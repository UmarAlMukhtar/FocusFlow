//! Finite transport supervisor using the audio owner's exact child, Job and pending registry.
use super::approval::TransportPermit;
use crate::{
    owned_child::{Bundle, ChildOps, PipeRead, SharedRegistry},
    pending::RetainedChild,
    Result,
};
use std::time::{Duration, Instant};
#[cfg(any(windows, test))]
#[path = "startup.rs"]
pub(crate) mod startup;
const SCRIPT: &[u8] = include_bytes!("../../../../../tools/audio-proof/fetch-approval.ps1");
const OUTPUT_CAP: usize = 512 * 1024;
#[derive(Debug)]
enum Reply {
    Pending(Option<String>),
    Complete(Vec<u8>),
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    schema_version: u32,
    pending_commit: String,
}
fn pending(raw: &[u8]) -> Result<Option<String>> {
    let p: Pending =
        serde_json::from_slice(raw).map_err(|_| "Invalid pending transport receipt")?;
    if p.schema_version != 1
        || (!p.pending_commit.is_empty()
            && (p.pending_commit.len() != 40
                || !p
                    .pending_commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))))
    {
        return Err("Invalid pending commit".into());
    }
    Ok(if p.pending_commit.is_empty() {
        None
    } else {
        Some(p.pending_commit)
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum FailurePhase {
    Init,
    Gate,
    Ref,
    Manifest,
    Request,
    Reply,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum FailureKind {
    Initialization,
    Input,
    Transport,
    Response,
    Shape,
    Output,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FailureDiagnostic {
    schema_version: u32,
    phase: FailurePhase,
    failure: FailureKind,
    http_status: serde_json::Value,
}
fn failure_diagnostic(raw: &[u8]) -> Option<FailureDiagnostic> {
    if raw.len() > 1024 {
        return None;
    }
    let diagnostic: FailureDiagnostic = serde_json::from_slice(raw).ok()?;
    if diagnostic.schema_version != 1
        || !(diagnostic.http_status.is_null()
            || diagnostic.http_status.as_u64().is_some_and(|s| (100..=599).contains(&s)))
    {
        return None;
    }
    Some(diagnostic)
}
struct TransportOwner {
    child: RetainedChild<Bundle>,
    registry: SharedRegistry,
    forced: bool,
    exited: bool,
}
impl TransportOwner {
    fn ops(&mut self) -> &mut dyn ChildOps {
        self.child
            .owned
            .as_mut()
            .expect("owned during supervision")
            .child
            .as_mut()
    }
    fn force(&mut self) {
        if !self.forced && !self.exited {
            self.forced = true;
            self.ops().close_control();
            let _ = self.ops().force();
        }
    }
}
impl Drop for TransportOwner {
    fn drop(&mut self) {
        if !self.exited {
            self.force();
            self.child
                .retain_pending(&mut self.registry.0.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }
}
fn supervise(
    child: Box<dyn ChildOps + Send>,
    gate: &[u8],
    registry: SharedRegistry,
    deadline: Instant,
    now: impl FnMut() -> Instant,
    pause: impl FnMut(),
    active: &impl Fn() -> Result<()>,
) -> Result<Reply> {
    let outcome = supervise_finite(
        child,
        gate,
        registry,
        deadline,
        Duration::ZERO,
        OUTPUT_CAP,
        now,
        pause,
        active,
    );
    if let Some(reason) = outcome.reason {
        return Err(reason);
    }
    match outcome.exit_code {
        Some(0) if !outcome.raw_response.is_empty() => Ok(Reply::Complete(outcome.raw_response)),
        Some(75) => pending(&outcome.raw_response).map(Reply::Pending),
        _ => {
            let diagnostic = if outcome.exit_code == Some(71) {
                failure_diagnostic(&outcome.raw_response)
            } else {
                None
            };
            Err(format!(
                "Transport TLS/fetch/shape/exit failure; outcome={}",
                serde_json::json!({
                    "helperExit": outcome.exit_code,
                    "stdoutBytes": outcome.raw_response.len(),
                    "eof": outcome.eof,
                    "exactExit": outcome.exact_exit,
                    // The owner's flag records an attempted force, not its success.
                    "forceAttempted": outcome.forced,
                    "diagnostic": diagnostic,
                })
            ))
        }
    }
}
fn supervise_finite(
    child: Box<dyn ChildOps + Send>,
    gate: &[u8],
    registry: SharedRegistry,
    deadline: Instant,
    release: Duration,
    output_cap: usize,
    mut now: impl FnMut() -> Instant,
    mut pause: impl FnMut(),
    active: &impl Fn() -> Result<()>,
) -> DiscoveryObservation {
    let mut owner = TransportOwner {
        child: RetainedChild::new(Bundle { id: 0, child }),
        registry,
        forced: false,
        exited: false,
    };
    let mut result = DiscoveryObservation::failed("Helper did not complete".into());
    result.reason = (|| -> Result<()> {
        if now() >= deadline || gate.is_empty() || gate.len() > 4096 {
            return Err("Helper deadline or Init cap exceeded".into());
        }
        active()?;
        owner.ops().assign_job()?;
        if now() >= deadline {
            return Err("Helper deadline expired before gate".into());
        }
        active()?;
        owner.ops().send_init(gate)
    })()
    .err();
    while result.reason.is_none() {
        if let Err(reason) = active() {
            result.reason = Some(reason);
            break;
        }
        if now() >= deadline {
            result.reason = Some("Helper absolute deadline expired".into());
            break;
        }
        for _ in 0..4 {
            let mut chunk = [0; 4096];
            match owner.ops().read_stdout(&mut chunk) {
                Ok(PipeRead::Data(n)) if n > 0 && n <= chunk.len() => {
                    if result.raw_response.len() + n > output_cap {
                        result.reason = Some("Helper output cap exceeded".into());
                        break;
                    }
                    result.raw_response.extend_from_slice(&chunk[..n]);
                }
                Ok(PipeRead::Empty) => break,
                Ok(PipeRead::Eof) => {
                    result.eof = true;
                    break;
                }
                Ok(_) => {
                    result.reason = Some("Invalid helper read count".into());
                    break;
                }
                Err(e) => {
                    result.reason = Some(e);
                    break;
                }
            }
        }
        match owner.ops().observe_exit() {
            Ok(Some(exit)) => {
                owner.exited = true;
                result.exit_code = exit.code;
            }
            Ok(None) => (),
            Err(e) => result.reason = Some(e),
        }
        if now() >= deadline {
            result.reason = Some("Helper absolute deadline expired".into());
        }
        if result.reason.is_none() && owner.exited && result.eof {
            result.exact_exit = true;
            return result;
        }
        pause();
    }
    if !owner.exited && !release.is_zero() {
        let _ = owner.ops().signal_stop();
        owner.ops().close_control();
        let until = now() + release;
        while !owner.exited && now() < until {
            if let Ok(Some(exit)) = owner.ops().observe_exit() {
                owner.exited = true;
                result.exit_code = exit.code;
            } else {
                pause();
            }
        }
    }
    owner.force();
    let until = now() + Duration::from_secs(2);
    while !owner.exited && now() < until {
        if let Ok(Some(exit)) = owner.ops().observe_exit() {
            owner.exited = true;
            result.exit_code = exit.code;
        } else {
            pause();
        }
    }
    result.exact_exit = owner.exited;
    result.forced = owner.forced;
    result.cleanup_pending = !owner.exited;
    if result.cleanup_pending {
        result.reason = Some(format!(
            "{}; CleanupPending: exact helper owner retained",
            result.reason.as_deref().unwrap_or("Helper failed")
        ));
    }
    result
}
fn supervise_discovery(
    spawn: impl FnOnce() -> Result<Box<dyn ChildOps + Send>>,
    gate: &[u8],
    registry: SharedRegistry,
    deadline: Instant,
    now: impl FnMut() -> Instant,
    pause: impl FnMut(),
    active: &impl Fn() -> Result<()>,
) -> DiscoveryObservation {
    if !registry.replacement_allowed() {
        return DiscoveryObservation::blocked();
    }
    if let Err(reason) = active() {
        return DiscoveryObservation::failed(reason);
    }
    let child = match spawn() {
        Ok(child) => child,
        Err(e) => return DiscoveryObservation::failed(e),
    };
    let mut outcome = supervise_finite(
        child,
        gate,
        registry,
        deadline,
        Duration::from_secs(3),
        OUTPUT_CAP,
        now,
        pause,
        active,
    );
    if outcome.reason.is_none() {
        let parsed = (|| -> Result<Vec<RenderEndpoint>> {
            if !outcome.eof || !outcome.exact_exit || outcome.exit_code != Some(0) {
                return Err("Discovery requires EOF and exact exit 0".into());
            }
            if outcome.raw_response.len() > 65536 {
                return Err("Discovery inventory response exceeds 64 KiB".into());
            }
            let reply: InventoryReply = serde_json::from_slice(&outcome.raw_response)
                .map_err(|_| "Invalid finite inventory response")?;
            if reply.schema_version != 1 {
                return Err("Invalid inventory version".into());
            }
            validate_inventory(&reply.endpoints)?;
            Ok(reply.endpoints)
        })();
        match parsed {
            Ok(items) => {
                outcome.status = if items.is_empty() {
                    DiscoveryStatus::Unavailable
                } else {
                    DiscoveryStatus::Inventory
                };
                outcome.endpoints = items;
            }
            Err(e) => outcome.reason = Some(e),
        }
    }
    outcome
}
pub(super) fn fetch(permit: &TransportPermit, active: &impl Fn() -> Result<()>) -> Result<Vec<u8>> {
    #[cfg(not(windows))]
    {
        let _ = (permit, active);
        Err("Authenticated transport requires Windows".into())
    }
    #[cfg(windows)]
    {
        let registry = crate::owned_child::native_registry();
        let phase_deadline = Instant::now() + Duration::from_secs(600);
        let mut previous_commit: Option<String> = None;
        loop {
            active()?;
            if !registry.replacement_allowed() {
                return Err("Previous transport/audio cleanup remains pending".into());
            }
            if Instant::now() >= phase_deadline {
                return Err("Approval polling phase expired".into());
            }
            let started = Instant::now();
            let script = Script::create()?;
            #[derive(serde::Serialize)]
            struct Gate<'a> {
                request: &'a super::approval::ApprovalRequestV1,
                previous_commit: &'a Option<String>,
            }
            let gate = serde_json::to_vec(&Gate {
                request: permit.request(),
                previous_commit: &previous_commit,
            })
            .map_err(|e| e.to_string())?;
            let mut gate = gate;
            gate.push(b'\n');
            if gate.len() > 4096 {
                return Err("Transport request exceeds gate cap".into());
            }
            let script_path = script.path.clone();
            active()?;
            let child = crate::owned_child::windows_child::WindowsChild::spawn_transport(
                permit,
                &script_path,
                Box::new(script),
            )?;
            match supervise(
                child,
                &gate,
                registry.clone(),
                (started + Duration::from_secs(30)).min(phase_deadline),
                Instant::now,
                || std::thread::sleep(Duration::from_millis(10)),
                active,
            )? {
                Reply::Complete(raw) => return Ok(raw),
                Reply::Pending(commit) => {
                    previous_commit = commit;
                    let next = (started + Duration::from_secs(15)).min(phase_deadline);
                    while Instant::now() < next {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
    }
}
#[cfg(windows)]
struct Script {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
}
#[cfg(windows)]
impl Script {
    fn create() -> Result<Self> { Self::create_bytes(SCRIPT) }
    fn create_bytes(bytes: &[u8]) -> Result<Self> {
        use std::{
            io::{Read, Write},
            os::windows::fs::OpenOptionsExt,
        };
        if bytes.is_empty() || bytes.len() > 65536 {
            return Err("Embedded fetcher cap exceeded".into());
        }
        let root = std::env::temp_dir()
            .canonicalize()
            .map_err(|e| e.to_string())?;
        crate::wav::reject_links(&root)?;
        // SAFETY: CoCreateGuid needs no COM apartment and produces only a fresh owned name.
        let nonce =
            unsafe { windows::Win32::System::Com::CoCreateGuid().map_err(|e| e.to_string())? };
        let path = root.join(format!("focusflow-audio-approval-{nonce:?}.ps1"));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .share_mode(1)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let mut script = Self {
            path,
            file: Some(file),
        };
        let file = script.file.as_mut().ok_or("Missing owned script")?;
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .map_err(|e| e.to_string())?;
        // A read-only lease permits PowerShell's reader while denying write/delete sharing.
        script.file.take();
        let mut lease = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&script.path)
            .map_err(|e| e.to_string())?;
        let mut actual = Vec::new();
        (&mut lease)
            .take(65537)
            .read_to_end(&mut actual)
            .map_err(|e| e.to_string())?;
        if actual != bytes {
            return Err("Owned embedded fetcher changed before launch".into());
        }
        script.file = Some(lease);
        Ok(script)
    }
}
#[cfg(windows)]
impl Drop for Script {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pending::ObservedExit, protocol::ClockStamp};
    use std::sync::{Arc, Mutex};
    struct Facts {
        output: Vec<u8>,
        calls: Vec<&'static str>,
        exit: Option<i32>,
        flood: bool,
        assign_failure: bool,
        no_eof: bool,
        observe_failure: bool,
        unknown_exit: bool,
    }
    struct Fake(Arc<Mutex<Facts>>);
    impl ChildOps for Fake {
        fn assign_job(&mut self) -> Result<()> {
            let mut f = self.0.lock().unwrap();
            f.calls.push("assign");
            if f.assign_failure {
                Err("assign failed".into())
            } else {
                Ok(())
            }
        }
        fn send_init(&mut self, _: &[u8]) -> Result<()> {
            self.0.lock().unwrap().calls.push("init");
            Ok(())
        }
        fn read_stdout(&mut self, b: &mut [u8]) -> Result<PipeRead> {
            if self.0.lock().unwrap().flood {
                b.fill(b'x');
                Ok(PipeRead::Data(b.len()))
            } else {
                let mut f = self.0.lock().unwrap();
                if f.output.is_empty() {
                    Ok(if f.no_eof {
                        PipeRead::Empty
                    } else {
                        PipeRead::Eof
                    })
                } else {
                    let n = b.len().min(f.output.len());
                    b[..n].copy_from_slice(&f.output[..n]);
                    f.output.drain(..n);
                    Ok(PipeRead::Data(n))
                }
            }
        }
        fn clock(&mut self) -> Result<(Instant, ClockStamp)> {
            unreachable!()
        }
        fn observe_exit(&mut self) -> Result<Option<ObservedExit>> {
            if self.0.lock().unwrap().unknown_exit {
                return Ok(Some(ObservedExit { code: None }));
            }
            if self.0.lock().unwrap().observe_failure {
                return Err("exact handle observation error".into());
            }
            Ok(self
                .0
                .lock()
                .unwrap()
                .exit
                .map(|code| ObservedExit { code: Some(code) }))
        }
        fn signal_stop(&mut self) -> Result<()> {
            self.0.lock().unwrap().calls.push("stop");
            Ok(())
        }
        fn close_control(&mut self) {
            self.0.lock().unwrap().calls.push("close");
        }
        fn force(&mut self) -> Result<()> {
            self.0.lock().unwrap().calls.push("force");
            Ok(())
        }
    }
    fn facts(exit: Option<i32>) -> Arc<Mutex<Facts>> {
        Arc::new(Mutex::new(Facts {
            output: Vec::new(),
            calls: Vec::new(),
            exit,
            flood: false,
            assign_failure: false,
            no_eof: false,
            observe_failure: false,
            unknown_exit: false,
        }))
    }
    fn run(f: Arc<Mutex<Facts>>, r: SharedRegistry) -> Result<Reply> {
        let start = Instant::now();
        let mut t = start;
        supervise(
            Box::new(Fake(f)),
            b"{}\n",
            r,
            start + Duration::from_secs(30),
            || {
                let result = t;
                t += Duration::from_millis(10);
                result
            },
            || {},
            &|| Ok(()),
        )
    }
    #[test]
    fn startup_owner_caps_before_acceptance_and_retains_uncertain_exit() {
        let start = Instant::now();
        let mut time = start;
        let f = facts(Some(0));
        f.lock().unwrap().output = vec![b'x'; 1025];
        let outcome = supervise_finite(Box::new(Fake(f.clone())), b"owned\n", SharedRegistry::new(), start + Duration::from_secs(28), Duration::ZERO, 1024,
            || { let t = time; time += Duration::from_millis(10); t }, || {}, &|| Ok(()));
        assert!(outcome.reason.is_some(), "oversized diagnostic was accepted");
        assert!(outcome.raw_response.len() <= 1024);
    }
    #[test]
    fn startup_owner_deadline_assignment_eof_and_retention_use_existing_owner() {
        for fault in 0..5 {
            let start = Instant::now();
            let mut time = start;
            let f = facts(if fault == 2 { Some(0) } else { None });
            match fault { 0 => f.lock().unwrap().assign_failure = true, 1 => f.lock().unwrap().flood = true,
                2 => f.lock().unwrap().no_eof = true, 3 => f.lock().unwrap().observe_failure = true, _ => () }
            let registry = SharedRegistry::new();
            let outcome = supervise_finite(Box::new(Fake(f.clone())), b"owned\n", registry.clone(), start + Duration::from_secs(28), Duration::ZERO, 1024,
                || { let t = time; time += Duration::from_millis(10); t }, || {}, &|| Ok(()));
            assert!(outcome.reason.is_some());
            assert!(outcome.raw_response.len() <= 1024);
            assert!(time.duration_since(start) <= Duration::from_millis(30100));
            if fault == 0 { assert!(!f.lock().unwrap().calls.contains(&"init")); }
            if fault != 2 {
                assert!(outcome.cleanup_pending);
                assert_eq!(registry.len(), 1);
                assert_eq!(f.lock().unwrap().calls.iter().filter(|c| **c == "force").count(), 1);
            }
        }
    }
    #[test]
    fn controller_request_reads_only_one_bounded_frame() {
        let mut raw = std::io::Cursor::new(b"{}\nqueued-control");
        assert_eq!(read_controller_request(&mut raw).unwrap(), b"{}\n");
        assert_eq!(raw.position(), 3);
        for bytes in [b"{}".to_vec(), vec![b'x'; 4097]] {
            assert!(read_controller_request(&mut bytes.as_slice()).is_err());
        }
        let mut maximum = vec![b' '; 4095];
        maximum.push(b'\n');
        assert_eq!(
            read_controller_request(&mut maximum.as_slice())
                .unwrap()
                .len(),
            4096
        );
    }
    #[test]
    fn controller_eof_denies_each_helper_transition_and_retains_existing_owner() {
        use std::cell::Cell;
        for deny_at in 1..=5 {
            let f = facts(None);
            let r = SharedRegistry::new();
            let checks = Cell::new(0);
            let spawned = Cell::new(false);
            let start = Instant::now();
            let mut time = start;
            let result = supervise_discovery(
                || {
                    spawned.set(true);
                    Ok(Box::new(Fake(f.clone())))
                },
                b"{}\n",
                r.clone(),
                start + Duration::from_secs(10),
                || {
                    let now = time;
                    time += Duration::from_millis(100);
                    now
                },
                || {},
                &|| {
                    checks.set(checks.get() + 1);
                    if crate::worker::pipe_control(false, 0, checks.get() >= deny_at)?
                        == crate::worker::Control::Live
                    {
                        Ok(())
                    } else {
                        Err("controller EOF".into())
                    }
                },
            );
            assert_eq!(result.status, DiscoveryStatus::Failed);
            assert!(result.endpoints.is_empty());
            assert!(result.reason.as_ref().unwrap().contains("controller EOF"));
            assert_eq!(spawned.get(), deny_at > 1);
            assert_eq!(result.cleanup_pending, deny_at > 1);
            let calls = f.lock().unwrap().calls.clone();
            assert_eq!(calls.contains(&"assign"), deny_at > 2);
            assert_eq!(calls.contains(&"init"), deny_at > 3);
            if deny_at > 1 {
                f.lock().unwrap().exit = Some(71);
                assert_eq!(
                    finish_controller(Ok(result), &r, |_| Ok(()), || panic!("already exited")),
                    71
                );
                assert_eq!(r.len(), 0);
                assert_eq!(f.lock().unwrap().calls, calls);
            }
        }
    }
    #[test]
    fn controller_reap_observation_error_and_receipt_error_cannot_release_owner_early() {
        let f = facts(None);
        let r = SharedRegistry::new();
        let error = run(f.clone(), r.clone()).unwrap_err();
        f.lock().unwrap().observe_failure = true;
        f.lock().unwrap().exit = Some(71);
        let mut polls = 0;
        let mut receipts = Vec::new();
        let code = finish_controller(
            Err(error),
            &r,
            |receipt| {
                receipts.push(receipt);
                Err("receipt pipe failed".into())
            },
            || {
                polls += 1;
                assert_eq!(r.len(), 1);
                if polls == 3 {
                    f.lock().unwrap().observe_failure = false;
                }
            },
        );
        assert_eq!(code, 77);
        assert_eq!(polls, 3);
        assert_eq!(r.len(), 0);
        assert_eq!(receipts[1]["exact_exit"], true);
        assert_eq!(receipts[1]["exit_code"], 71);
    }
    #[test]
    fn controller_completion_waits_for_delayed_exact_exit_on_failure_and_error() {
        for transport_error in [false, true] {
            let f = facts(None);
            let r = SharedRegistry::new();
            let error = run(f.clone(), r.clone()).unwrap_err();
            let calls = f.lock().unwrap().calls.clone();
            let result = if transport_error {
                Err(error)
            } else {
                Ok(DiscoveryObservation::blocked())
            };
            let mut receipts = Vec::new();
            let mut polls = 0;
            let code = finish_controller(
                result,
                &r,
                |receipt| {
                    receipts.push(receipt);
                    Ok(())
                },
                || {
                    assert!(!r.replacement_allowed());
                    polls += 1;
                    if polls == 3 {
                        f.lock().unwrap().exit = Some(71);
                    }
                },
            );
            assert_eq!(code, if transport_error { 77 } else { 71 });
            assert_eq!(polls, 3);
            assert_eq!(r.len(), 0);
            assert_eq!(f.lock().unwrap().calls, calls);
            assert_eq!(receipts[0]["cleanup_pending"], true);
            assert_eq!(receipts[0]["status"], "Failed");
            assert_eq!(receipts[1]["exact_exit"], true);
            assert_eq!(receipts[1]["exit_code"], 71);
            assert_eq!(receipts[2]["cleanup_pending"], false);
            assert_eq!(receipts[2]["status"], "Failed");
        }
    }
    #[test]
    fn controller_completion_retains_permanently_uncertain_owner_even_if_receipt_fails() {
        for write_failure in [false, true] {
            let f = facts(None);
            let r = SharedRegistry::new();
            let error = run(f.clone(), r.clone()).unwrap_err();
            let calls = f.lock().unwrap().calls.clone();
            let mut receipts = Vec::new();
            let mut polls = 0;
            let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                finish_controller(
                    Err(error),
                    &r,
                    |receipt| {
                        receipts.push(receipt);
                        if write_failure {
                            Err("closed receipt pipe".into())
                        } else {
                            Ok(())
                        }
                    },
                    || {
                        polls += 1;
                        assert!(!r.replacement_allowed());
                        if polls == 5 {
                            panic!("test observes external job teardown boundary");
                        }
                    },
                )
            }));
            assert!(
                interrupted.is_err(),
                "controller returned with uncertain ownership"
            );
            assert_eq!(polls, 5);
            assert_eq!(r.len(), 1);
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0]["cleanup_pending"], true);
            assert_eq!(f.lock().unwrap().calls, calls);
        }
    }
    #[test]
    fn controller_completion_no_owner_preserves_failure_and_unavailable() {
        let r = SharedRegistry::new();
        let mut unavailable = DiscoveryObservation::failed("unused".into());
        unavailable.status = DiscoveryStatus::Unavailable;
        assert_eq!(
            finish_controller(Ok(unavailable), &r, |_| Ok(()), || panic!("no owner")),
            0
        );
        assert_eq!(
            finish_controller(Err("denied".into()), &r, |_| Ok(()), || panic!("no owner")),
            77
        );
        assert_eq!(
            finish_controller(
                Err("denied".into()),
                &r,
                |_| Err("closed".into()),
                || panic!("no owner")
            ),
            77
        );
    }
    #[test]
    fn transport_tls_redirect_output_failure_denied() {
        for code in [1, 71, 0] {
            assert!(run(facts(Some(code)), SharedRegistry::new()).is_err());
        }
        let f = facts(Some(0));
        f.lock().unwrap().flood = true;
        assert!(run(f, SharedRegistry::new()).is_err());
    }
    // Rejecting these receipts must not grant or poll, and must never echo their bytes.
    fn failure_facts(code: i32, raw: &[u8]) -> serde_json::Value {
        let f = facts(Some(code));
        f.lock().unwrap().output = raw.to_vec();
        let error = run(f, SharedRegistry::new()).unwrap_err();
        assert!(error.starts_with("Transport TLS/fetch/shape/exit failure; outcome="), "{error}");
        serde_json::from_str(error.split_once("; outcome=").unwrap().1).unwrap()
    }
    #[test]
    fn transport_failure_retains_actual_exit_and_output_facts_without_raw_bytes() {
        for (code, raw) in [(1, b"private body".as_slice()), (71, b""), (0, b"")] {
            let observed = failure_facts(code, raw);
            assert_eq!(observed["helperExit"], code);
            assert_eq!(observed["stdoutBytes"], raw.len());
            assert_eq!(observed["eof"], true);
            assert_eq!(observed["exactExit"], true);
            assert_eq!(observed["forceAttempted"], false);
            assert!(observed["diagnostic"].is_null());
            assert!(!observed.to_string().contains("private body"));
        }
    }
    #[test]
    fn transport_failure_keeps_unknown_exit_null_and_cannot_bypass_eof() {
        let f = facts(None);
        f.lock().unwrap().unknown_exit = true;
        let error = run(f, SharedRegistry::new()).unwrap_err();
        let observed: serde_json::Value = serde_json::from_str(error.split_once("; outcome=").unwrap().1).unwrap();
        assert!(observed["helperExit"].is_null());
        assert_eq!(observed["exactExit"], true);
        let f = facts(Some(71));
        {
            let mut f = f.lock().unwrap();
            f.output = br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":404}"#.to_vec();
            f.no_eof = true;
        }
        let error = run(f, SharedRegistry::new()).unwrap_err();
        assert!(error.contains("deadline"));
        assert!(!error.contains("outcome="));
    }
    #[test]
    fn transport_failure_diagnostic_only_accepts_bounded_strict_failure71_receipt() {
        let valid = br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":404}"#;
        let observed = failure_facts(71, valid);
        assert_eq!(observed["diagnostic"], serde_json::json!({"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":404}));
        assert!(failure_facts(1, valid)["diagnostic"].is_null());
        for phase in ["init", "gate", "ref", "manifest", "request", "reply"] {
            for failure in ["initialization", "input", "transport", "response", "shape", "output"] {
                for status in ["null", "100", "599"] {
                    let raw = format!(r#"{{"schemaVersion":1,"phase":"{phase}","failure":"{failure}","httpStatus":{status}}}"#);
                    assert_eq!(failure_facts(71, raw.as_bytes())["diagnostic"]["phase"], phase);
                }
            }
        }
        for raw in [
            b"".to_vec(), b"not JSON".to_vec(), vec![b' '; 1025],
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":null,"errorMessage":"private body"}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","phase":"gate","failure":"transport","httpStatus":null}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"other","failure":"transport","httpStatus":null}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"other","httpStatus":null}"#.to_vec(),
            br#"{"schemaVersion":2,"phase":"ref","failure":"transport","httpStatus":null}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport"}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":99}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":600}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":"404"}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":404.0}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":true}"#.to_vec(),
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":null}{}"#.to_vec(),
        ] {
            let observed = failure_facts(71, &raw);
            assert!(observed["diagnostic"].is_null(), "{observed}");
            assert!(!observed.to_string().contains("private body"));
        }
        let mut exact_cap = valid.to_vec();
        exact_cap.resize(1024, b' ');
        assert_eq!(failure_facts(71, &exact_cap)["diagnostic"]["httpStatus"], 404);
        exact_cap.push(b' ');
        assert!(failure_facts(71, &exact_cap)["diagnostic"].is_null());
    }
    #[test]
    fn transport_failure_script_source_fixture_covers_init_and_fixed_diagnostic_only() {
        // Static fixture evidence only; this does not execute or parse PowerShell.
        let script = std::str::from_utf8(SCRIPT).unwrap();
        assert!(script.find("try {").unwrap() < script.find("[Console]::InputEncoding").unwrap());
        for phase in ["init", "gate", "ref", "manifest", "request", "reply"] {
            assert!(script.contains(&format!("$script:phase = '{phase}'")));
        }
        assert!(script.contains("$depth -lt 8"));
        assert!(script.contains("schemaVersion=1; phase=$script:phase; failure=$script:failure; httpStatus=$script:httpStatus"));
        assert!(script.contains("GetByteCount($json) -le 1024"));
        assert!(script.contains("if ($pending -and $_.Exception.Response -and [int]$_.Exception.Response.StatusCode -eq 404) { Pending }"));
    }
    #[test]
    fn transport_failure_diagnostic_does_not_change_pending_protocol() {
        for raw in [
            br#"{"schema_version":1,"pending_commit":""}"#.as_slice(),
            br#"{"schema_version":1,"pending_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        ] {
            let f = facts(Some(75)); f.lock().unwrap().output = raw.to_vec();
            assert!(matches!(run(f, SharedRegistry::new()).unwrap(), Reply::Pending(_)));
            assert!(failure_facts(71, raw)["diagnostic"].is_null());
        }
        for raw in [
            br#"{"schemaVersion":1,"phase":"ref","failure":"transport","httpStatus":404}"#.as_slice(),
            br#"{"schema_version":1,"pending_commit":"","extra":1}"#,
            br#"{"schema_version":1,"pending_commit":"","pending_commit":""}"#,
        ] {
            let f = facts(Some(75)); f.lock().unwrap().output = raw.to_vec();
            assert!(run(f, SharedRegistry::new()).is_err());
        }
    }
    #[test]
    fn transport_timeout_retains_exact_owner() {
        let f = facts(None);
        let r = SharedRegistry::new();
        assert!(run(f.clone(), r.clone())
            .unwrap_err()
            .contains("CleanupPending"));
        assert!(!r.replacement_allowed());
        assert_eq!(
            f.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|v| **v == "force")
                .count(),
            1
        );
        f.lock().unwrap().exit = Some(71);
        assert_eq!(r.reap(), 0);
        assert!(r.replacement_allowed());
    }
    #[test]
    fn transport_requires_complete_output_eof_and_exact_zero_exit() {
        let f = facts(Some(0));
        f.lock().unwrap().output = b"response".to_vec();
        assert!(
            matches!(run(f.clone(),SharedRegistry::new()).unwrap(),Reply::Complete(raw) if raw==b"response")
        );
        assert_eq!(f.lock().unwrap().calls, ["assign", "init"]);
        let f = facts(Some(75));
        f.lock().unwrap().output = br#"{"schema_version":1,"pending_commit":""}"#.to_vec();
        assert!(matches!(
            run(f, SharedRegistry::new()).unwrap(),
            Reply::Pending(None)
        ));
        for raw in [
            br#"{"schema_version":1}"#.as_slice(),
            br#"{"schema_version":1,"pending_commit":null}"#,
            br#"{"schema_version":1,"pending_commit":"bad"}"#,
        ] {
            assert!(pending(raw).is_err());
        }
    }
    #[test]
    fn transport_cannot_reach_com_audio() {
        let f = facts(Some(71));
        f.lock().unwrap().assign_failure = true;
        assert!(run(f.clone(), SharedRegistry::new()).is_err());
        assert!(!f.lock().unwrap().calls.contains(&"init"));
        assert!(SCRIPT.len() <= 65536);
        let script = std::str::from_utf8(SCRIPT).unwrap();
        assert!(!script.contains("New-Object -ComObject"));
        assert!(script.contains("AllowAutoRedirect = $false"));
    }
    fn discovery(f: Arc<Mutex<Facts>>, registry: SharedRegistry) -> DiscoveryObservation {
        let start = Instant::now();
        let mut time = start;
        supervise_discovery(
            || Ok(Box::new(Fake(f))),
            b"{}\n",
            registry,
            start + Duration::from_secs(10),
            || {
                let t = time;
                time += Duration::from_millis(10);
                t
            },
            || {},
            &|| Ok(()),
        )
    }
    #[test]
    fn no_inventory_without_clean_exact_exit() {
        for (exit, eof) in [(Some(0), false), (Some(71), true), (None, true)] {
            let f = facts(exit);
            {
                let mut f = f.lock().unwrap();
                f.output = br#"{"schema_version":1,"endpoints":[]}"#.to_vec();
                f.no_eof = !eof;
            }
            let result = discovery(f, SharedRegistry::new());
            assert_eq!(result.status, DiscoveryStatus::Failed);
            assert!(result.endpoints.is_empty());
        }
        let f = facts(Some(0));
        f.lock().unwrap().output = br#"{"schema_version":1,"endpoints":[]}"#.to_vec();
        let result = discovery(f, SharedRegistry::new());
        assert_eq!(result.status, DiscoveryStatus::Unavailable);
        assert!(result.exact_exit && result.eof);
        assert!(!result.forced);
    }
    #[test]
    fn helper_flood_cannot_reset_deadline() {
        let f = facts(None);
        f.lock().unwrap().flood = true;
        let registry = SharedRegistry::new();
        let result = discovery(f.clone(), registry.clone());
        assert_eq!(result.status, DiscoveryStatus::Failed);
        assert!(result.raw_response.len() <= 512 * 1024);
        assert!(result.cleanup_pending);
        assert!(!registry.replacement_allowed());
        assert_eq!(
            f.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|c| **c == "force")
                .count(),
            1
        );
    }
    #[test]
    fn pending_blocks_replacement() {
        let f = facts(None);
        let r = SharedRegistry::new();
        assert!(discovery(f.clone(), r.clone()).cleanup_pending);
        let next = facts(Some(0));
        assert_eq!(
            discovery(next.clone(), r.clone()).status,
            DiscoveryStatus::Failed
        );
        assert!(next.lock().unwrap().calls.is_empty());
        f.lock().unwrap().exit = Some(71);
        assert_eq!(r.reap(), 0);
        assert!(r.replacement_allowed());
    }

    #[test]
    fn helper_absolute_deadline_and_release_force_windows_do_not_reset() {
        let f = facts(None);
        f.lock().unwrap().flood = true;
        let start = Instant::now();
        let mut time = start;
        let result = supervise_discovery(
            || Ok(Box::new(Fake(f.clone()))),
            b"{}\n",
            SharedRegistry::new(),
            start + Duration::from_secs(10),
            || {
                let t = time;
                time += Duration::from_millis(250);
                t
            },
            || {},
            &|| Ok(()),
        );
        assert!(result.reason.unwrap().contains("absolute deadline"));
        assert!(result.raw_response.len() < 512 * 1024);
        assert!(time.duration_since(start) >= Duration::from_secs(15));
        assert!(time.duration_since(start) < Duration::from_secs(17));
        let calls = &f.lock().unwrap().calls;
        assert_eq!(calls.iter().filter(|c| **c == "stop").count(), 1);
        assert_eq!(calls.iter().filter(|c| **c == "force").count(), 1);
        assert!(calls.iter().position(|c| *c == "stop") < calls.iter().position(|c| *c == "force"));
    }
    #[test]
    fn discovery_assignment_failure_never_sends_init() {
        let f = facts(Some(71));
        f.lock().unwrap().assign_failure = true;
        let result = discovery(f.clone(), SharedRegistry::new());
        assert_eq!(result.status, DiscoveryStatus::Failed);
        assert!(!f.lock().unwrap().calls.contains(&"init"));
        assert!(result.endpoints.is_empty());
    }
    #[test]
    fn discovery_rejects_trailing_unknown_duplicate_and_bad_inventory() {
        for raw in [
            br#"{"schema_version":1,"endpoints":[]} {}"#.as_slice(),
            br#"{"schema_version":1,"endpoints":[],"extra":true}"#,
            br#"{"schema_version":1,"schema_version":1,"endpoints":[]}"#,
            br#"{"schema_version":1,"endpoints":[{"id":"x","label":"","state":1,"flow":1}]}"#,
        ] {
            let f = facts(Some(0));
            f.lock().unwrap().output = raw.to_vec();
            let result = discovery(f, SharedRegistry::new());
            assert_eq!(result.status, DiscoveryStatus::Failed);
            assert_eq!(result.raw_response, raw);
            assert!(result.endpoints.is_empty());
        }
    }
}

/// Discovery has its own bounded protocol; it never accepts an audio TrackRequest.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperInitV1 {
    pub nonce: String,
    pub approval_commit: String,
    pub approval_digest: String,
    pub request_digest: String,
    pub phase: String,
    pub output_namespace: String,
}
fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn init_shape(init: &HelperInitV1, nonce: &str) -> Result<()> {
    let valid_nonce = nonce.len() == 36
        && nonce.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        });
    if !valid_nonce
        || init.nonce != nonce
        || !hex(&init.approval_commit, 40)
        || !hex(&init.approval_digest, 64)
        || !hex(&init.request_digest, 64)
        || init.phase != "DiscoverRender"
        || init.output_namespace.len() > 128
        || !init.output_namespace.starts_with("audio-discovery-")
        || !init
            .output_namespace
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Invalid delegated discovery init".into());
    }
    Ok(())
}
/// Pure validation is not a native permit. Production supplies actual Job/pipe observations.
pub fn validate_helper_init(
    raw: &[u8],
    nonce: &str,
    in_job: bool,
    queued: u32,
    eof: bool,
    stop: bool,
) -> Result<HelperInitV1> {
    if raw.is_empty()
        || raw.len() > 4096
        || !in_job
        || crate::worker::pipe_control(stop, queued, eof)? != crate::worker::Control::Live
    {
        return Err("Discovery ownership gate cancelled".into());
    }
    let init: HelperInitV1 =
        serde_json::from_slice(raw).map_err(|_| "Invalid exact helper init schema")?;
    init_shape(&init, nonce)?;
    Ok(init)
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RenderEndpoint {
    pub id: String,
    pub label: String,
    pub state: u32,
    pub flow: i32,
}
/// This adapter exposes only the permitted render-active metadata operations.
pub trait RenderInventoryOps {
    fn count(&mut self) -> Result<u32>;
    fn item(&mut self, index: u32) -> Result<RenderEndpoint>;
}
fn validate_inventory(items: &[RenderEndpoint]) -> Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    if items.len() > 32
        || items.iter().any(|e| {
            e.id.is_empty()
                || e.id.len() > 2048
                || e.id.contains('\0')
                || e.label.len() > 512
                || e.label.contains('\0')
                || e.state != 1
                || e.flow != 0
                || !ids.insert(e.id.as_str())
        })
    {
        return Err("Render inventory count, identity, state, flow or label cap failed".into());
    }
    if serde_json::to_vec(items).map_err(|e| e.to_string())?.len() > 65536 {
        return Err("Render inventory exceeds 64 KiB".into());
    }
    Ok(())
}
pub fn collect_inventory(ops: &mut impl RenderInventoryOps) -> Result<Vec<RenderEndpoint>> {
    let count = ops.count()?;
    if count > 32 {
        return Err("Render endpoint count exceeds 32".into());
    }
    let mut items = Vec::new();
    for index in 0..count {
        items.push(ops.item(index)?);
        validate_inventory(&items)?;
    }
    Ok(items)
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryReply {
    schema_version: u32,
    endpoints: Vec<RenderEndpoint>,
}
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub enum DiscoveryStatus {
    Inventory,
    Unavailable,
    Failed,
}
#[derive(Debug, serde::Serialize)]
pub struct DiscoveryObservation {
    pub status: DiscoveryStatus,
    pub endpoints: Vec<RenderEndpoint>,
    pub raw_response: Vec<u8>,
    pub eof: bool,
    pub exact_exit: bool,
    pub exit_code: Option<i32>,
    pub forced: bool,
    pub cleanup_pending: bool,
    pub reason: Option<String>,
}
impl DiscoveryObservation {
    fn blocked() -> Self {
        let mut result = Self::failed("Previous exact owner cleanup remains pending".into());
        result.cleanup_pending = true;
        result
    }
    fn failed(reason: String) -> Self {
        Self {
            status: DiscoveryStatus::Failed,
            endpoints: vec![],
            raw_response: vec![],
            eof: false,
            exact_exit: false,
            exit_code: None,
            forced: false,
            cleanup_pending: false,
            reason: Some(reason),
        }
    }
}

#[cfg(windows)]
pub(crate) struct AdmissionRecord {
    _permit: super::approval::DiscoveryPermit,
    _file: std::fs::File,
}
#[cfg(windows)]
impl AdmissionRecord {
    pub(crate) fn paths(&self) -> (std::path::PathBuf, std::path::PathBuf) {
        (
            self._permit.executable().to_path_buf(),
            self._permit.root().to_path_buf(),
        )
    }
    fn create(permit: super::approval::DiscoveryPermit, init: &HelperInitV1) -> Result<Self> {
        use std::{
            io::{Read, Write},
            os::windows::fs::OpenOptionsExt,
        };
        permit.check()?;
        if permit.record().is_empty() || permit.record().len() > 256 * 1024 {
            return Err("Internal admission record cap exceeded".into());
        }
        let dir = permit.root().join(&init.output_namespace);
        std::fs::create_dir(&dir).map_err(|e| format!("Create fresh admission namespace: {e}"))?;
        crate::wav::reject_links(&dir)?;
        if dir.canonicalize().map_err(|e| e.to_string())? != dir {
            return Err("Admission namespace is not canonical".into());
        }
        let path = dir.join("admission.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(1)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(permit.record())
            .and_then(|_| file.flush())
            .map_err(|e| e.to_string())?;
        drop(file);
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes != permit.record() {
            return Err("Admission record changed before locking".into());
        }
        Ok(Self {
            _permit: permit,
            _file: file,
        })
    }
}
/// Native capability is private and requires the actual shared pipe/Job gate plus bound record.
#[cfg(windows)]
pub(crate) struct AdmittedDiscovery {
    gate: crate::native::OwnedGate,
    expires: u64,
    _record: std::fs::File,
    _locks: Vec<std::fs::File>,
}
#[cfg(windows)]
impl AdmittedDiscovery {
    pub(crate) fn check(&self) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        if now >= self.expires || self.gate.control()? != crate::worker::Control::Live {
            return Err("Discovery capability expired or control cancelled".into());
        }
        Ok(())
    }
    fn receive(job: &str, stop: &str, nonce: &str) -> Result<Self> {
        use std::{io::Read, os::windows::fs::OpenOptionsExt, path::PathBuf};
        let (gate, init) = crate::native::OwnedGate::receive(job, stop, |input| {
            let mut raw = Vec::new();
            for _ in 0..4096 {
                let mut byte = [0];
                input.read_exact(&mut byte).map_err(|e| e.to_string())?;
                raw.push(byte[0]);
                if byte[0] == b'\n' {
                    return validate_helper_init(&raw, nonce, true, 0, false, false);
                }
            }
            Err("Helper Init exceeds 4 KiB".into())
        })?;
        let root =
            PathBuf::from(std::env::var_os("GITHUB_WORKSPACE").ok_or("Missing owned workspace")?);
        crate::wav::reject_links(&root)?;
        if root.canonicalize().map_err(|e| e.to_string())? != root {
            return Err("Owned workspace is not canonical".into());
        }
        let path = root.join(&init.output_namespace).join("admission.json");
        crate::wav::reject_links(&path)?;
        let mut record = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .map_err(|e| e.to_string())?;
        if !record.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("Internal admission is not a regular file".into());
        }
        let mut raw = Vec::new();
        (&mut record)
            .take(256 * 1024 + 1)
            .read_to_end(&mut raw)
            .map_err(|e| e.to_string())?;
        let (expires, locks) = super::approval::admit_internal(&root, &init, &raw)?;
        let permit = Self {
            gate,
            expires,
            _record: record,
            _locks: locks,
        };
        permit.check()?;
        Ok(permit)
    }
}
pub fn discover_render(
    permit: super::approval::DiscoveryPermit,
    active: &impl Fn() -> Result<()>,
) -> DiscoveryObservation {
    #[cfg(not(windows))]
    {
        let _ = (permit, active);
        DiscoveryObservation::failed("Discovery requires Windows".into())
    }
    #[cfg(windows)]
    {
        let registry = crate::owned_child::native_registry();
        if !registry.replacement_allowed() {
            return DiscoveryObservation::blocked();
        }
        let prepared = (|| -> Result<_> {
            active()?;
            permit.check()?;
            // SAFETY: fresh namespace only; no apartment or audio operation.
            let nonce = format!("{:?}", unsafe {
                windows::Win32::System::Com::CoCreateGuid().map_err(|e| e.to_string())?
            });
            let init = permit.init(nonce.clone())?;
            let mut gate = serde_json::to_vec(&init).map_err(|e| e.to_string())?;
            gate.push(b'\n');
            if gate.len() > 4096 {
                return Err("Helper Init cap exceeded".into());
            }
            let record = AdmissionRecord::create(permit, &init)?;
            Ok((registry, nonce, gate, record))
        })();
        match prepared {
            Err(e) => DiscoveryObservation::failed(e),
            Ok((registry, nonce, gate, record)) => supervise_discovery(
                || crate::owned_child::windows_child::WindowsChild::spawn_discovery(record, &nonce),
                &gate,
                registry,
                Instant::now() + Duration::from_secs(10),
                Instant::now,
                || std::thread::sleep(Duration::from_millis(10)),
                active,
            ),
        }
    }
}
#[cfg(windows)]
pub(crate) fn dispatch_discovery(job: &str, stop: &str, nonce: &str) -> i32 {
    use std::io::Write;
    let result = (|| -> Result<()> {
        let permit = AdmittedDiscovery::receive(job, stop, nonce)?;
        let endpoints = crate::native::discover_render(&permit)?;
        permit.check()?;
        let bytes = serde_json::to_vec(&InventoryReply {
            schema_version: 1,
            endpoints,
        })
        .map_err(|e| e.to_string())?;
        if bytes.len() > 65536 {
            return Err("Inventory reply exceeds 64 KiB".into());
        }
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(|e| e.to_string())
    })();
    if result.is_ok() {
        0
    } else {
        71
    }
}
pub(crate) fn dispatch_controller() -> i32 {
    use std::io::Write;
    #[cfg(not(windows))]
    let result = Err("Controller requires Windows".into());
    #[cfg(windows)]
    let result = (|| -> Result<DiscoveryObservation> {
        let mut input = crate::native::duplicate_stdin()?;
        let raw = read_controller_request(&mut input)?;
        let active = || {
            if crate::native::pipe_control(&input, false)? == crate::worker::Control::Live {
                Ok(())
            } else {
                Err("Controller active work cancelled by staging".into())
            }
        };
        let request: super::approval::ApprovalRequestV1 =
            serde_json::from_slice(&raw).map_err(|_| "Invalid observed request")?;
        let remote = super::approval::fetch_approval(&request, &active)?;
        let root = std::path::PathBuf::from(
            std::env::var_os("GITHUB_WORKSPACE").ok_or("Missing observed workspace")?,
        );
        active()?;
        let permit = super::approval::authorize_discovery(&root, remote)?;
        let admission = permit.receipt();
        let observation = discover_render(permit, &active);
        active()?;
        let reply = serde_json::json!({"schema_version":1,"admission":admission,"discovery":observation,"collection":"Disabled","qualification":"Unqualified","route_ownership":"Root review required"});
        let mut out = std::io::stdout().lock();
        serde_json::to_writer(&mut out, &reply).map_err(|e| e.to_string())?;
        out.write_all(b"\n")
            .and_then(|_| out.flush())
            .map_err(|e| e.to_string())?;
        Ok(observation)
    })();
    finish_controller(
        result,
        &crate::owned_child::native_registry(),
        |receipt| {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &receipt).map_err(|e| e.to_string())?;
            out.write_all(b"\n")
                .and_then(|_| out.flush())
                .map_err(|e| e.to_string())
        },
        || std::thread::sleep(Duration::from_millis(100)),
    )
}
fn finish_controller(
    result: Result<DiscoveryObservation>,
    registry: &SharedRegistry,
    publish: impl FnMut(serde_json::Value) -> Result<()>,
    pause: impl FnMut(),
) -> i32 {
    let code = match &result {
        Ok(o) if o.status != DiscoveryStatus::Failed => 0,
        Ok(_) => 71,
        Err(_) => 77,
    };
    finish_retained_controller(code, result.err(), registry, publish, pause)
}
fn finish_retained_controller(
    mut code: i32,
    reason: Option<String>,
    registry: &SharedRegistry,
    mut publish: impl FnMut(serde_json::Value) -> Result<()>,
    mut pause: impl FnMut(),
) -> i32 {
    let pending = registry.len();
    if pending == 0 {
        if let Some(reason) = reason {
            let _ = publish(serde_json::json!({"schema_version":1,"status":"Failed",
                "cleanup_pending":false,"reason":reason,"collection":"Disabled","qualification":"Unqualified"}));
        }
        return code;
    }
    if code == 0 {
        code = 71;
    }
    let receipt = serde_json::json!({"schema_version":1,"status":"Failed","cleanup_pending":true,
        "pending_owners":pending,"reason":reason,"collection":"Disabled","qualification":"Unqualified"});
    if publish(receipt).is_err() {
        code = 77;
    }
    // No active-work deadline can release an uncertain exact owner; only observation or job teardown can.
    while registry.reap_with(|owner, exit| {
        if publish(
            serde_json::json!({"schema_version":1,"status":"Failed","owner":owner,
            "exact_exit":true,"exit_code":exit.code}),
        )
        .is_err()
        {
            code = 77;
        }
    }) != 0
    {
        pause();
    }
    if publish(
        serde_json::json!({"schema_version":1,"status":"Failed","cleanup_pending":false,
        "pending_owners":0,"collection":"Disabled","qualification":"Unqualified"}),
    )
    .is_err()
    {
        code = 77;
    }
    code
}
fn read_controller_request(input: &mut impl std::io::Read) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    for _ in 0..4096 {
        let mut byte = [0];
        input.read_exact(&mut byte).map_err(|e| e.to_string())?;
        raw.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(raw);
        }
    }
    Err("Observed request exceeds 4 KiB framed input".into())
}
