//! Fixed no-network startup diagnostic; never an approval or discovery route.
use super::*;
use serde::Serialize;
const SENTINEL: &[u8] = include_bytes!("../../../../../tools/audio-proof/startup-sentinel.ps1");
const MARKER: &[u8] = br#"{"schemaVersion":1,"sentinel":true}"#;
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
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SentinelObservation {
    facts: ChildFacts,
    marker: bool,
}
impl SentinelObservation {
    fn project(o: DiscoveryObservation) -> (Self, bool) {
        let finished = o.exit_code.is_some()
            && o.reason.is_none()
            && o.eof
            && o.exact_exit
            && !o.forced
            && !o.cleanup_pending;
        let marker = finished && o.exit_code == Some(0) && o.raw_response == MARKER;
        // A normal nonzero exit with empty stdout is useful even if PowerShell rejected -File.
        let observed = marker || (finished && o.exit_code != Some(0) && o.raw_response.is_empty());
        (Self { facts: ChildFacts::from(&o), marker }, observed)
    }
}
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum AbStatus { Unsupported, Incomplete, Observed }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema_version: u8,
    diagnostic: &'static str,
    path_form: PathForm,
    ab_status: AbStatus,
    canonical: Option<SentinelObservation>,
    dos: Option<SentinelObservation>,
    collection: &'static str,
    qualification: &'static str,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Canonical, Dos }
fn sequence(form: PathForm, mut run: impl FnMut(Mode) -> Option<DiscoveryObservation>) -> Report {
    let mut report = Report {
        schema_version: 2,
        diagnostic: "StartupOnly",
        path_form: form,
        ab_status: AbStatus::Unsupported,
        canonical: None,
        dos: None,
        collection: "Disabled",
        qualification: "Unqualified",
    };
    if form != PathForm::DeviceDrive {
        return report;
    }
    let Some(canonical) = run(Mode::Canonical) else { return report };
    let (canonical, observed) = SentinelObservation::project(canonical);
    report.canonical = Some(canonical);
    report.ab_status = AbStatus::Incomplete;
    if !observed {
        return report;
    }
    if let Some(dos) = run(Mode::Dos) {
        let (dos, observed) = SentinelObservation::project(dos);
        report.dos = Some(dos);
        if observed {
            report.ab_status = AbStatus::Observed;
        }
    }
    report
}
#[cfg(windows)]
pub(crate) struct StartupLaunch {
    path: std::path::PathBuf,
    lifetime: Box<dyn Send>,
}
#[cfg(windows)]
impl StartupLaunch {
    pub(crate) fn into_parts(self) -> (std::path::PathBuf, Box<dyn Send>) {
        (self.path, self.lifetime)
    }
}
#[cfg(windows)]
// Close the extra read lease before Script removes the sentinel file.
struct Sentinel { _dos_lease: std::fs::File, script: std::sync::Arc<Script>, dos: std::path::PathBuf }
#[cfg(windows)]
impl Sentinel {
    fn create(script: Script) -> Result<Self> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION}};
        let script = std::sync::Arc::new(script);
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
    let deadline = Instant::now() + Duration::from_secs(60);
    let registry = crate::owned_child::native_registry();
    if !registry.replacement_allowed() {
        return Err("Previous exact owner remains pending".into());
    }
    let script = Script::create_bytes(SENTINEL).map_err(|_| "Startup sentinel lease unavailable")?;
    let form = script.path.to_str().map_or(PathForm::Other, path_form);
    let sentinel = Sentinel::create(script).ok().map(Arc::new);
    Ok(sequence(form, |mode| {
        let owned = sentinel.as_ref()?;
        let started = Instant::now();
        let child_deadline = (started + Duration::from_secs(28)).min(deadline - Duration::from_secs(2));
        if started >= child_deadline || !registry.replacement_allowed() || active().is_err() {
            return Some(DiscoveryObservation::failed("Startup deadline/control/owner".into()));
        }
        let path = match mode {
            Mode::Canonical => &owned.script.path,
            Mode::Dos => &owned.dos,
        };
        let launch = StartupLaunch { path: path.clone(), lifetime: Box::new(owned.clone()) };
        let child = match crate::owned_child::windows_child::WindowsChild::spawn_startup(permit, launch) {
            Ok(child) => child,
            Err(_) => return Some(DiscoveryObservation::failed("Startup spawn failed".into())),
        };
        Some(supervise_finite(
            child,
            b"focusflow-startup-sentinel-v1\n",
            registry.clone(),
            child_deadline,
            Duration::ZERO,
            1024,
            Instant::now,
            || std::thread::sleep(Duration::from_millis(10)),
            active,
        ))
    }))
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
            Ok(report) => { let ok = report.ab_status == AbStatus::Observed;
                match serde_json::to_value(report).ok().and_then(|r| publish(r).ok()) { Some(()) => (if ok { 0 } else { 71 }, None), None => (77, Some("Startup receipt write failed".into())) }
            },
            Err(_) => (77, Some("Startup preparation/context failed".into())),
        };
        finish_retained_controller(code, reason, &crate::owned_child::native_registry(), publish, || std::thread::sleep(Duration::from_millis(100)))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn observation(raw: &[u8], exit: Option<i32>) -> DiscoveryObservation {
        let mut o = DiscoveryObservation::failed("fixture only".into());
        o.reason = None; o.eof = true; o.exact_exit = true; o.exit_code = exit; o.raw_response = raw.to_vec(); o
    }
    #[cfg(windows)]
    #[test]
    fn startup_sentinel_closes_both_leases_at_final_shared_owner_release() {
        let script = Script::create_bytes(SENTINEL).expect("actual owned script");
        let sentinel = std::sync::Arc::new(Sentinel::create(script).expect("actual owned sentinel"));
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
    fn startup_path_pair_needs_only_two_sentinel_children() {
        let mut modes = Vec::new();
        let _ = sequence(PathForm::DeviceDrive, |mode| {
            modes.push(mode);
            Some(if mode == Mode::Canonical {
                observation(b"", Some(1))
            } else {
                observation(MARKER, Some(0))
            })
        });
        assert!(modes == [Mode::Canonical, Mode::Dos]);
    }
    #[test]
    fn startup_completed_pair_reports_markers_and_no_authority() {
        for (canonical_marker, dos_marker) in [(true, true), (false, true), (false, false)] {
            let report = sequence(PathForm::DeviceDrive, |mode| {
                let marked = if mode == Mode::Canonical { canonical_marker } else { dos_marker };
                Some(if marked { observation(MARKER, Some(0)) } else { observation(b"", Some(1)) })
            });
            assert_eq!(report.ab_status, AbStatus::Observed);
            assert_eq!(report.canonical.unwrap().marker, canonical_marker);
            assert_eq!(report.dos.unwrap().marker, dos_marker);
            assert_eq!(report.collection, "Disabled");
            assert_eq!(report.qualification, "Unqualified");
        }
    }
    #[test]
    fn startup_unsupported_path_never_launches_a_child() {
        for form in [PathForm::Drive, PathForm::DeviceUnc, PathForm::Other] {
            let report = sequence(form, |_| panic!("unexpected child for unsupported path"));
            assert_eq!(report.ab_status, AbStatus::Unsupported);
            assert!(report.canonical.is_none() && report.dos.is_none());
        }
        let report = sequence(PathForm::DeviceDrive, |mode| {
            assert!(mode == Mode::Canonical);
            None
        });
        assert_eq!(report.ab_status, AbStatus::Unsupported);
        assert!(report.canonical.is_none() && report.dos.is_none());
    }
    #[test]
    fn startup_canonical_owner_and_protocol_failures_block_dos() {
        for fault in 0..11 {
            let report = sequence(PathForm::DeviceDrive, |mode| {
                assert!(mode == Mode::Canonical, "unexpected DOS launch after fault {fault}");
                let mut o = observation(MARKER, Some(0));
                match fault {
                    0 => o.eof = false,
                    1 => o.exact_exit = false,
                    2 => { o.forced = true; o.exit_code = Some(71); o.raw_response.clear(); }
                    3 => o.cleanup_pending = true,
                    4 => o.reason = Some("deadline".into()),
                    5 => { o.exit_code = None; o.raw_response.clear(); }
                    6 => o.reason = Some("control EOF".into()),
                    7 => { o.raw_response.clear(); o.reason = Some("output cap".into()); }
                    8 => o.raw_response.clear(),
                    9 => o.exit_code = Some(1),
                    _ => o.raw_response = vec![b'x'; 1025],
                }
                Some(o)
            });
            assert_eq!(report.ab_status, AbStatus::Incomplete);
            assert!(!report.canonical.unwrap().marker);
            assert!(report.dos.is_none());
        }
    }
    #[test]
    fn startup_requires_exact_marker_and_never_projects_raw_stdout() {
        let invalid = [
            b"private body or exception".to_vec(),
            MARKER[..MARKER.len() - 1].to_vec(),
            [MARKER, b"\n"].concat(),
            [MARKER, MARKER].concat(),
            br#"{"schemaVersion":1,"sentinel":true,"sentinel":true}"#.to_vec(),
            br#"{"schemaVersion":1,"sentinel":"true"}"#.to_vec(),
        ];
        for raw in invalid {
            let report = sequence(PathForm::DeviceDrive, |mode| {
                Some(if mode == Mode::Canonical { observation(MARKER, Some(0)) } else { observation(&raw, Some(0)) })
            });
            assert_eq!(report.ab_status, AbStatus::Incomplete);
            assert!(!report.dos.as_ref().unwrap().marker);
            let projected = serde_json::to_string(&report).unwrap();
            assert!(!projected.contains("private body") && !projected.contains("rawResponse"));
        }
    }
    #[test]
    fn startup_fixed_sentinel_source_fixture_reads_only_bounded_gate_and_marks() {
        assert!(SENTINEL.is_ascii() && SENTINEL.len() < 1024);
        let source = std::str::from_utf8(SENTINEL).unwrap();
        assert!(source.contains("$i -lt 64") && source.contains("[Console]::Read()"));
        assert!(source.contains("'focusflow-startup-sentinel-v1'"));
        assert!(source.contains(&format!("[Console]::Write('{}')", std::str::from_utf8(MARKER).unwrap())));
        assert!(source.ends_with("exit 0\n"));
        for forbidden in ["Invoke-Expression", "Invoke-WebRequest", "WebClient", "HttpWebRequest", "Set-ExecutionPolicy", "Start-Process", "-ComObject", "Write-Output", "Write-Error", "[IO.", "$path"] {
            assert!(!source.contains(forbidden));
        }
    }
    #[cfg(not(windows))]
    #[test]
    fn startup_fixed_dispatch_is_separate_and_rejects_extra_arguments() {
        let args = |values: &[&str]| values.iter().map(std::ffi::OsString::from).collect::<Vec<_>>().into_iter();
        assert_eq!(crate::proof::dispatch(args(&["proof", "--private-audio-startup-probe"])), 69);
        assert_eq!(crate::proof::dispatch(args(&["proof", "--private-audio-startup-probe", "override"])), 64);
    }
    #[test]
    fn startup_dos_candidate_is_only_short_unambiguous_local_device_drive() {
        assert_eq!(dos_candidate(r"\\?\C:\Temp\sentinel.ps1"), Some(r"C:\Temp\sentinel.ps1"));
        for path in [r"C:\Temp\s.ps1", r"\\?\UNC\host\share\s.ps1", r"\\?\C:\a.\s.ps1", r"\\?\C:\a \s.ps1", r"\\?\C:\a\..\s.ps1", r"\\?\C:\a\\s.ps1", r"\\?\C:\a:s.ps1", r"\\?\C:\CON\s.ps1"] { assert!(dos_candidate(path).is_none(), "ambiguous path accepted"); }
        assert!(dos_candidate(&format!(r"\\?\C:\{}\s.ps1", "a".repeat(250))).is_none());
    }
}
