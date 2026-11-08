//! Fixed no-network startup diagnostic; never an approval or discovery route.
use super::*;
use serde::{Deserialize, Serialize};
pub(crate) const COMMAND: &str = include_str!("../../../../../tools/audio-proof/startup-probe.ps1");
const SENTINEL: &[u8] = include_bytes!("../../../../../tools/audio-proof/startup-sentinel.ps1");
const MARKER: &[u8] = br#"{"schemaVersion":1,"sentinel":true}"#;
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
enum Policy { Restricted, AllSigned, RemoteSigned, Unrestricted, Bypass, Undefined,
    #[serde(rename = "other")] Other, #[serde(rename = "unavailable")] Unavailable }
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Phase { Encoding, Gate, Version, Read, Parser, Policy, Fromjson, Tojson, Complete }
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Probe {
    schema_version: u8, entered: bool, ps_major: u8, ps_minor: u8, policy: Policy,
    data_read: bool, parser_complete: bool, parse_errors: u8, initialization: bool, phase: Phase,
}
fn parse(raw: &[u8]) -> Option<Probe> {
    if raw.len() > 1024 { return None; }
    let p: Probe = serde_json::from_slice(raw).ok()?;
    if p.schema_version != 1 || !p.entered || p.ps_major > 16 || p.ps_minor > 16 || p.parse_errors > 64
        || (p.parser_complete && !p.data_read) || (!p.parser_complete && p.parse_errors != 0)
        || (p.initialization != (p.phase == Phase::Complete))
        || (p.initialization && (!p.data_read || !p.parser_complete)) {
        return None;
    }
    Some(p)
}
impl Probe {
    fn clean(&self) -> bool {
        self.phase == Phase::Complete && self.ps_major == 5 && self.ps_minor == 1
            && self.parse_errors == 0 && matches!(self.policy, Policy::RemoteSigned | Policy::Unrestricted | Policy::Bypass)
    }
}
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum PathForm { Drive, DeviceDrive, DeviceUnc, Other }
fn path_form(path: &str) -> PathForm {
    let bytes = path.as_bytes();
    if path.starts_with(r"\\?\UNC\") { PathForm::DeviceUnc }
    else if bytes.len() >= 7 && path.starts_with(r"\\?\") && bytes[4].is_ascii_alphabetic() && &bytes[5..7] == b":\\" { PathForm::DeviceDrive }
    else if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":\\" { PathForm::Drive }
    else { PathForm::Other }
}
// This candidate is private to the newly owned harmless sentinel, never the fetch script.
fn dos_candidate(canonical: &str) -> Option<&str> {
    if path_form(canonical) != PathForm::DeviceDrive { return None; }
    let path = &canonical[4..];
    if !path.is_ascii() || path.len() >= 248 { return None; }
    for part in path[3..].split('\\') {
        if part.is_empty() || part.ends_with(['.', ' ']) || part == "." || part == ".."
            || part.bytes().any(|b| b < 32 || b == 127 || b"<>:\"/|?*".contains(&b)) { return None; }
        let name = part.split('.').next()?.to_ascii_uppercase();
        if ["CON", "PRN", "AUX", "NUL"].contains(&name.as_str())
            || ((name.starts_with("COM") || name.starts_with("LPT")) && name.len() == 4 && (b'1'..=b'9').contains(&name.as_bytes()[3])) { return None; }
    }
    Some(path)
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChildFacts { helper_exit: Option<i32>, stdout_bytes: usize, eof: bool, exact_exit: bool, force_attempted: bool, cleanup_pending: bool }
impl From<&DiscoveryObservation> for ChildFacts {
    fn from(o: &DiscoveryObservation) -> Self { Self { helper_exit: o.exit_code, stdout_bytes: o.raw_response.len(), eof: o.eof, exact_exit: o.exact_exit, force_attempted: o.forced, cleanup_pending: o.cleanup_pending } }
}
fn finished(o: &DiscoveryObservation) -> bool { o.exit_code.is_some() && o.reason.is_none() && o.eof && o.exact_exit && !o.forced && !o.cleanup_pending }
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ProbeStatus { Observed, OwnerFailure, InvalidReceipt }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandObservation { status: ProbeStatus, facts: ChildFacts, receipt: Option<Probe> }
fn project(o: DiscoveryObservation) -> CommandObservation {
    let facts = ChildFacts::from(&o);
    let receipt = if finished(&o) && o.exit_code == Some(0) { parse(&o.raw_response) } else { None };
    let status = if !finished(&o) { ProbeStatus::OwnerFailure } else if receipt.is_none() { ProbeStatus::InvalidReceipt } else { ProbeStatus::Observed };
    CommandObservation { status, facts, receipt }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SentinelObservation { facts: ChildFacts, marker: bool }
impl SentinelObservation {
    fn new(o: &DiscoveryObservation) -> Self { Self { facts: ChildFacts::from(o), marker: finished(o) && o.exit_code == Some(0) && o.raw_response == MARKER } }
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum AbStatus { NotEligible, Unsupported, Incomplete, Observed }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report { schema_version: u8, diagnostic: &'static str, path_form: PathForm, command: CommandObservation, ab_status: AbStatus, canonical: Option<SentinelObservation>, dos: Option<SentinelObservation>, collection: &'static str, qualification: &'static str }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Command, Canonical, Dos }
fn sequence(form: PathForm, mut run: impl FnMut(Mode) -> Option<DiscoveryObservation>) -> Result<Report> {
    let command = project(run(Mode::Command).ok_or("Startup preparation unavailable")?);
    let eligible = command.receipt.as_ref().is_some_and(Probe::clean) && form == PathForm::DeviceDrive;
    let mut report = Report { schema_version: 1, diagnostic: "StartupOnly", path_form: form, command, ab_status: AbStatus::NotEligible, canonical: None, dos: None, collection: "Disabled", qualification: "Unqualified" };
    if eligible {
        let Some(canonical) = run(Mode::Canonical) else { report.ab_status = AbStatus::Unsupported; return Ok(report); };
        report.canonical = Some(SentinelObservation::new(&canonical));
        report.ab_status = AbStatus::Incomplete;
        // A normal exact nonzero -File exit is an observation; timeout/ownership/output violations stop the invocation.
        if !finished(&canonical) || (canonical.exit_code == Some(0) && canonical.raw_response != MARKER)
            || (canonical.exit_code != Some(0) && !canonical.raw_response.is_empty()) { return Ok(report); }
        if let Some(dos) = run(Mode::Dos) {
            report.dos = Some(SentinelObservation::new(&dos));
            if finished(&dos) && ((dos.exit_code == Some(0) && dos.raw_response == MARKER) || (dos.exit_code != Some(0) && dos.raw_response.is_empty())) { report.ab_status = AbStatus::Observed; }
        }
    }
    Ok(report)
}
#[cfg(windows)]
pub(crate) struct StartupLaunch { path: Option<std::path::PathBuf>, lifetime: Box<dyn Send> }
#[cfg(windows)]
impl StartupLaunch {
    pub(crate) fn into_parts(self) -> (Option<std::path::PathBuf>, Box<dyn Send>) { (self.path, self.lifetime) }
}
#[cfg(windows)]
// Close the extra read lease before Script removes the sentinel file.
struct Sentinel { _dos_lease: std::fs::File, script: std::sync::Arc<Script>, dos: std::path::PathBuf }
#[cfg(windows)]
impl Sentinel {
    fn create() -> Result<Self> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION}};
        let script = std::sync::Arc::new(Script::create_bytes(SENTINEL)?);
        let dos = std::path::PathBuf::from(dos_candidate(script.path.to_str().ok_or("Unsupported sentinel path")?).ok_or("Unsupported sentinel path")?);
        crate::wav::reject_links(&dos)?;
        if dos.canonicalize().map_err(|_| "Sentinel roundtrip failed")? != script.path { return Err("Sentinel roundtrip mismatch".into()); }
        let lease = std::fs::OpenOptions::new().read(true).share_mode(1).open(&dos).map_err(|_| "Sentinel DOS lease failed")?;
        let identity = |file: &std::fs::File| -> Result<_> {
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            // SAFETY: exact retained read-only File handle and generated writable structure live throughout this call.
            unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.map_err(|_| "Sentinel identity unavailable")?;
            if info.dwFileAttributes & (0x400 | 0x10) != 0 || info.nNumberOfLinks == 0 || (info.nFileIndexHigh == 0 && info.nFileIndexLow == 0) { return Err("Unsupported sentinel identity".into()); }
            Ok((info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
        };
        if identity(script.file.as_ref().ok_or("Missing sentinel lease")?)? != identity(&lease)? { return Err("Sentinel file identity mismatch".into()); }
        Ok(Self { script, dos, _dos_lease: lease })
    }
}
#[cfg(windows)]
fn run(permit: &TransportPermit, active: &impl Fn() -> Result<()>) -> Result<Report> {
    use std::sync::Arc;
    active()?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let registry = crate::owned_child::native_registry();
    if !registry.replacement_allowed() { return Err("Previous exact owner remains pending".into()); }
    let fetch = Arc::new(Script::create().map_err(|_| "Startup fetch lease unavailable")?);
    let form = path_form(fetch.path.to_str().ok_or("Unsupported fetch path")?);
    let mut gate = fetch.path.to_str().ok_or("Unsupported fetch path")?.as_bytes().to_vec();
    gate.push(b'\n');
    if gate.len() > 4096 || gate[..gate.len()-1].iter().any(|b| matches!(b, 0 | 10 | 13)) { return Err("Unsupported startup gate".into()); }
    let mut sentinel: Option<Arc<Sentinel>> = None;
    sequence(form, |mode| {
        if Instant::now() >= deadline - Duration::from_secs(2) || !registry.replacement_allowed() || active().is_err() { return Some(DiscoveryObservation::failed("Startup deadline/control/owner".into())); }
        let launch = match mode {
            Mode::Command => StartupLaunch { path: None, lifetime: Box::new(fetch.clone()) },
            Mode::Canonical | Mode::Dos => {
                if sentinel.is_none() { sentinel = Some(Arc::new(Sentinel::create().ok()?)); }
                let owned = sentinel.as_ref()?;
                StartupLaunch { path: Some(if mode == Mode::Canonical { owned.script.path.clone() } else { owned.dos.clone() }), lifetime: Box::new(owned.clone()) }
            }
        };
        if Instant::now() >= deadline - Duration::from_secs(2) || active().is_err() { return Some(DiscoveryObservation::failed("Startup deadline/control".into())); }
        let child = match crate::owned_child::windows_child::WindowsChild::spawn_startup(permit, launch) {
            Ok(child) => child, Err(_) => return Some(DiscoveryObservation::failed("Startup spawn failed".into())),
        };
        Some(supervise_finite(child, if mode == Mode::Command { &gate } else { b"focusflow-startup-sentinel-v1\n" }, registry.clone(),
            (Instant::now() + Duration::from_secs(28)).min(deadline - Duration::from_secs(2)), Duration::ZERO, 1024,
            Instant::now, || std::thread::sleep(Duration::from_millis(10)), active))
    })
}
#[cfg(windows)]
pub(crate) fn dispatch() -> i32 {
        use std::io::Write;
        let result = (|| -> Result<Report> {
            let mut input = crate::native::duplicate_stdin()?;
            let raw = read_controller_request(&mut input)?;
            let request: super::super::approval::ApprovalRequestV1 = serde_json::from_slice(&raw).map_err(|_| "Invalid startup request")?;
            let active = || if crate::native::pipe_control(&input, false)? == crate::worker::Control::Live { Ok(()) } else { Err("Startup cancelled".into()) };
            active()?;
            let permit = super::super::approval::startup_permit(&request)?;
            run(&permit, &active)
        })();
        let publish = |receipt: serde_json::Value| -> Result<()> {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &receipt).map_err(|_| "Startup receipt write failed")?;
            out.write_all(b"\n").and_then(|_| out.flush()).map_err(|_| "Startup receipt write failed".into())
        };
        let (code, reason) = match result {
            Ok(report) => { let ok = report.command.receipt.is_some() && report.ab_status != AbStatus::Incomplete;
                match serde_json::to_value(report).ok().and_then(|r| publish(r).ok()) { Some(()) => (if ok { 0 } else { 71 }, None), None => (77, Some("Startup receipt write failed".into())) }
            },
            Err(_) => (77, Some("Startup preparation/context failed".into())),
        };
        finish_retained_controller(code, reason, &crate::owned_child::native_registry(), publish, || std::thread::sleep(Duration::from_millis(100)))
}
#[cfg(test)]
mod tests {
    use super::*;
    const VALID: &[u8] = br#"{"schemaVersion":1,"entered":true,"psMajor":5,"psMinor":1,"policy":"RemoteSigned","dataRead":true,"parserComplete":true,"parseErrors":0,"initialization":true,"phase":"complete"}"#;
    fn observation(raw: &[u8], exit: Option<i32>) -> DiscoveryObservation {
        let mut o = DiscoveryObservation::failed("fixture only".into());
        o.reason = None; o.eof = true; o.exact_exit = true; o.exit_code = exit; o.raw_response = raw.to_vec(); o
    }
    #[cfg(windows)]
    #[test]
    fn startup_sentinel_closes_both_leases_at_final_shared_owner_release() {
        let sentinel = std::sync::Arc::new(Sentinel::create().expect("actual owned sentinel"));
        let path = sentinel.script.path.clone();
        let dos = sentinel.dos.clone();
        let retained_owner = sentinel.clone();
        assert!(path.is_file() && dos.is_file());
        drop(sentinel);
        assert!(path.is_file() && dos.is_file(), "shared owner must retain the actual sentinel");
        drop(retained_owner);
        assert_eq!(std::fs::metadata(&path).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert_eq!(std::fs::metadata(&dos).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }
    #[test]
    fn startup_sequence_clean_runs_at_most_three_fixed_children() {
        let mut modes = Vec::new();
        let report = sequence(PathForm::DeviceDrive, |mode| {
            modes.push(mode);
            Some(match mode { Mode::Command => observation(VALID, Some(0)), Mode::Canonical => observation(b"", Some(1)), Mode::Dos => observation(MARKER, Some(0)) })
        }).unwrap();
        assert!(modes == [Mode::Command, Mode::Canonical, Mode::Dos]);
        assert_eq!(report.ab_status, AbStatus::Observed);
        assert!(!report.canonical.unwrap().marker);
        assert!(report.dos.unwrap().marker);
        assert_eq!(report.collection, "Disabled");
        assert_eq!(report.qualification, "Unqualified");
    }
    #[test]
    fn startup_sequence_stops_after_unclean_probe_or_non_device_path() {
        for raw in [
            String::from_utf8(VALID.to_vec()).unwrap().replace("RemoteSigned", "Restricted"),
            String::from_utf8(VALID.to_vec()).unwrap().replace("RemoteSigned", "AllSigned"),
            String::from_utf8(VALID.to_vec()).unwrap().replace("RemoteSigned", "unavailable"),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"parseErrors\":0", "\"parseErrors\":1"),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"initialization\":true", "\"initialization\":false").replace("complete\"", "fromjson\""),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"psMajor\":5", "\"psMajor\":7"),
        ] {
            let mut calls = 0;
            let report = sequence(PathForm::DeviceDrive, |_| { calls += 1; Some(observation(raw.as_bytes(), Some(0))) }).unwrap();
            assert_eq!(calls, 1); assert_eq!(report.ab_status, AbStatus::NotEligible);
        }
        for form in [PathForm::Drive, PathForm::DeviceUnc, PathForm::Other] {
            let mut calls = 0;
            let report = sequence(form, |_| { calls += 1; Some(observation(VALID, Some(0))) }).unwrap();
            assert_eq!(calls, 1); assert_eq!(report.ab_status, AbStatus::NotEligible);
        }
        for fault in 0..7 {
            let mut calls = 0;
            let report = sequence(PathForm::DeviceDrive, |_| { calls += 1; let mut o = observation(VALID, Some(0));
                match fault { 0 => o.eof = false, 1 => o.exact_exit = false, 2 => o.forced = true, 3 => o.cleanup_pending = true, 4 => o.reason = Some("deadline".into()), 5 => o.exit_code = None, _ => o.exit_code = Some(71) }; Some(o) }).unwrap();
            assert_eq!(calls, 1); assert!(report.command.receipt.is_none());
        }
    }
    #[test]
    fn startup_sequence_unsupported_and_canonical_failures_do_not_retry() {
        for fault in 0..10 {
            let mut modes = Vec::new();
            let report = sequence(PathForm::DeviceDrive, |mode| {
                modes.push(mode);
                if mode == Mode::Command { return Some(observation(VALID, Some(0))); }
                assert!(mode == Mode::Canonical, "unexpected DOS launch after owner failure");
                if fault == 0 { return None; }
                let mut o = observation(b"", Some(1));
                match fault { 1 => o.eof = false, 2 => o.exact_exit = false, 3 => o.forced = true, 4 => o.cleanup_pending = true,
                    5 => o.reason = Some("cap or deadline".into()), 6 => o.exit_code = None, 7 => o.exit_code = Some(0),
                    8 => o.raw_response = b"private invalid output".to_vec(), _ => { o.exit_code = Some(0); o.raw_response = vec![b'x'; 1025]; } }; Some(o)
            }).unwrap();
            assert_eq!(modes.len(), 2);
            assert_eq!(report.ab_status, if fault == 0 { AbStatus::Unsupported } else { AbStatus::Incomplete });
        }
    }
    #[test]
    fn startup_projection_drops_raw_output_and_denies_wrong_exit_size_and_status() {
        for raw in [b"private body or exception".to_vec(), vec![b'x'; 1025], VALID.iter().copied().chain(b"{}".iter().copied()).collect()] {
            let o = project(observation(&raw, Some(0)));
            assert!(o.receipt.is_none());
            let projected = serde_json::to_string(&o).unwrap();
            assert!(!projected.contains("private body"));
            assert!(!projected.contains("rawResponse"));
        }
        for code in [None, Some(71), Some(75), Some(1)] { assert!(project(observation(VALID, code)).receipt.is_none()); }
    }
    #[test]
    fn startup_fixed_script_source_fixtures_are_no_network_and_no_eval() {
        assert!(COMMAND.is_ascii() && COMMAND.len() < 8192);
        assert!(COMMAND.contains("::ParseInput($source, [ref]$tokens, [ref]$errors)"));
        assert!(COMMAND.contains("[IO.File]::ReadAllBytes($path)"));
        assert!(COMMAND.contains("Get-ExecutionPolicy"));
        assert!(COMMAND.contains("for ($i = 0; $i -lt 4096; $i++)"));
        assert!(COMMAND.contains("[Console]::Write('{\"schemaVersion\":1"));
        for source in [COMMAND, std::str::from_utf8(SENTINEL).unwrap()] {
            for forbidden in ["Invoke-Expression", "Invoke-WebRequest", "WebClient", "HttpWebRequest", "Set-ExecutionPolicy", "Start-Process", "-ComObject", "Write-Output", "Write-Error", "Out-String"] { assert!(!source.contains(forbidden)); }
        }
        assert!(SENTINEL.len() < 1024);
        assert!(std::str::from_utf8(SENTINEL).unwrap().contains("$i -lt 64"));
        assert!(!std::str::from_utf8(SENTINEL).unwrap().contains("$path"));
    }
    #[cfg(not(windows))]
    #[test]
    fn startup_fixed_dispatch_is_separate_and_rejects_extra_arguments() {
        let args = |values: &[&str]| values.iter().map(std::ffi::OsString::from).collect::<Vec<_>>().into_iter();
        assert_eq!(crate::proof::dispatch(args(&["proof", "--private-audio-startup-probe"])), 69);
        assert_eq!(crate::proof::dispatch(args(&["proof", "--private-audio-startup-probe", "override"])), 64);
    }
    #[test]
    fn startup_strict_bounded_probe_projection() {
        assert!(parse(VALID).is_some());
        let value: serde_json::Value = serde_json::from_slice(VALID).unwrap();
        for key in value.as_object().unwrap().keys() {
            let mut missing = value.clone(); missing.as_object_mut().unwrap().remove(key);
            assert!(parse(&serde_json::to_vec(&missing).unwrap()).is_none(), "missing field accepted: {key}");
        }
        for raw in [
            VALID.iter().copied().chain(b"{}".iter().copied()).collect::<Vec<_>>(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"entered\":true", "\"entered\":true,\"entered\":true").into_bytes(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"entered\":true", "\"entered\":true,\"message\":\"private\"").into_bytes(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("RemoteSigned", "invalid").into_bytes(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"parseErrors\":0", "\"parseErrors\":65").into_bytes(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"parseErrors\":0", "\"parseErrors\":0.0").into_bytes(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"entered\":true", "\"entered\":1").into_bytes(),
            String::from_utf8(VALID.to_vec()).unwrap().replace("\"schemaVersion\":1,", "").into_bytes(),
            vec![b' '; 1025],
        ] { assert!(parse(&raw).is_none(), "accepted malformed probe"); }
    }
    #[test]
    fn startup_dos_candidate_is_only_short_unambiguous_local_device_drive() {
        assert_eq!(dos_candidate(r"\\?\C:\Temp\sentinel.ps1"), Some(r"C:\Temp\sentinel.ps1"));
        for path in [r"C:\Temp\s.ps1", r"\\?\UNC\host\share\s.ps1", r"\\?\C:\a.\s.ps1", r"\\?\C:\a \s.ps1", r"\\?\C:\a\..\s.ps1", r"\\?\C:\a\\s.ps1", r"\\?\C:\a:s.ps1", r"\\?\C:\CON\s.ps1"] { assert!(dos_candidate(path).is_none(), "ambiguous path accepted"); }
        assert!(dos_candidate(&format!(r"\\?\C:\{}\s.ps1", "a".repeat(250))).is_none());
    }
}
