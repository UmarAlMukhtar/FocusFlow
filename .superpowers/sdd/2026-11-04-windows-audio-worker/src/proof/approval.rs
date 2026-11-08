//! Independent authenticated discovery admission. No collection capability is produced here.
use super::helper;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub const REPOSITORY: &str = "UmarAlMukhtar/FocusFlow";
pub const SOURCE_REF: &str = "refs/heads/codex-audio-native-qualification";
pub const WORKFLOW_REF: &str = "UmarAlMukhtar/FocusFlow/.github/workflows/windows-audio-native-qualification.yml@refs/heads/codex-audio-native-qualification";
const MANIFEST_CAP: usize = 256 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequestV1 {
    pub schema_version: u32,
    pub repository: String,
    pub source_ref: String,
    pub workflow_ref: String,
    pub workflow_sha: String,
    pub source_sha: String,
    pub run_id: u64,
    pub attempt: u32,
    pub job_id: u64,
    pub job_name: String,
    pub runner_name: String,
    pub runner_image: String,
    pub arch: String,
    pub os: String,
    pub environment: String,
    pub actions: bool,
    pub request_id: String,
    pub request_sha256: String,
    pub output_namespace: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    path: String,
    sha256: String,
    bytes: u64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    discovery_seconds: u32,
    phase_seconds: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    review: String,
    request: ApprovalRequestV1,
    issued: u64,
    not_before: u64,
    expires: u64,
    phase: String,
    budget: Budget,
    executable: Pin,
    input_map: Pin,
    files: Vec<Pin>,
}
/// This value can only be returned by the fixed owned authenticated transport.
/// ```compile_fail
/// use focusflow_windows_audio_worker::proof::guard::approval::RemoteApproval;
/// let forged = RemoteApproval {};
/// ```
pub struct RemoteApproval {
    manifest: Manifest,
    commit: String,
    blob: String,
    publication_blob: String,
    raw: Vec<u8>,
}
/// Noninterchangeable with ProofPermit; construction and retained file locks are private.
/// ```compile_fail
/// use focusflow_windows_audio_worker::proof::guard::{ProofPermit,approval::DiscoveryPermit};
/// fn collect(discovery: DiscoveryPermit) -> ProofPermit { discovery.into() }
/// ```
/// ```compile_fail
/// use focusflow_windows_audio_worker::proof::guard::approval::DiscoveryPermit;
/// let forged = DiscoveryPermit {};
/// ```
pub struct DiscoveryPermit {
    _remote: RemoteApproval,
    _executable: PathBuf,
    _locks: Vec<File>,
    _manifest_sha256: String,
    root: PathBuf,
    observations: Vec<super::Identity>,
}
pub(crate) struct TransportPermit {
    request: ApprovalRequestV1,
}
impl TransportPermit {
    pub(super) fn request(&self) -> &ApprovalRequestV1 {
        &self.request
    }
}
impl RemoteApproval {
    /// Immutable delivery identities are diagnostic receipts, not constructors.
    pub fn delivery_ids(&self) -> (&str, &str, &str) {
        (&self.commit, &self.blob, &self.publication_blob)
    }
}
fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.trim() == s && !s.chars().any(char::is_control)
}
fn normalized(s: &str) -> bool {
    text(s, 1024)
        && s.split('/').all(|c| {
            let stem = c.split('.').next().unwrap_or("").to_ascii_uppercase();
            c.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
                && !c.is_empty()
                && c != "."
                && c != ".."
                && !c.ends_with(['.', ' '])
                && !c.contains(['\\', ':', '<', '>', '"', '|', '?', '*'])
                && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        })
}
pub fn validate_request(r: &ApprovalRequestV1) -> Result<()> {
    if r.schema_version != 1
        || r.repository != REPOSITORY
        || r.source_ref != SOURCE_REF
        || r.workflow_ref != WORKFLOW_REF
        || !hex(&r.workflow_sha, 40)
        || !hex(&r.source_sha, 40)
        || r.run_id == 0
        || r.attempt == 0
        || r.job_id == 0
        || !r.actions
        || r.environment != "github-hosted"
        || r.os != "Windows"
        || r.arch != "X64"
        || [&r.job_name, &r.runner_name, &r.runner_image]
            .iter()
            .any(|v| !text(v, 128))
        || !hex(&r.request_id, 32)
        || !hex(&r.request_sha256, 64)
        || !normalized(&r.output_namespace)
    {
        return Err("Invalid observed hosted discovery request".into());
    }
    Ok(())
}
fn bounds(raw: &[u8], cap: usize) -> Result<()> {
    if raw.is_empty() || raw.len() > cap {
        return Err("Approval byte cap exceeded".into());
    }
    let (mut depth, mut quoted, mut escape) = (0u32, false, false);
    for b in raw {
        if quoted {
            if escape {
                escape = false;
            } else if *b == b'\\' {
                escape = true;
            } else if *b == b'"' {
                quoted = false;
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 16 {
                        return Err("Approval nesting cap exceeded".into());
                    }
                }
                b'}' | b']' => {
                    depth = depth.checked_sub(1).ok_or("Unbalanced approval")?;
                }
                _ => (),
            }
        }
    }
    Ok(())
}
fn parse(raw: &[u8], request: &ApprovalRequestV1, now: u64) -> Result<Manifest> {
    bounds(raw, MANIFEST_CAP)?;
    let m: Manifest = serde_json::from_slice(raw).map_err(|_| "Invalid exact approval schema")?;
    validate_request(request)?;
    if &m.request != request
        || m.schema_version != 1
        || !text(&m.review, 256)
        || m.phase != "DiscoverRender"
        || m.budget.discovery_seconds != 10
        || m.budget.phase_seconds != 600
        || m.issued > m.not_before
        || m.not_before > now
        || now >= m.expires
        || m.expires.checked_sub(m.issued).is_none_or(|v| v > 900)
        || m.files.is_empty()
        || m.files.len() > 510
        || m.executable.path != "@executable"
        || !normalized(&m.input_map.path)
    {
        return Err("Approval request, phase or validity mismatch".into());
    }
    let (mut names, mut total) = (BTreeSet::new(), 0u64);
    for p in std::iter::once(&m.executable)
        .chain(std::iter::once(&m.input_map))
        .chain(&m.files)
    {
        if !hex(&p.sha256, 64)
            || p.bytes == 0
            || p.bytes > 512 * 1024 * 1024
            || !names.insert(p.path.to_ascii_lowercase())
            || (p.path != "@executable" && !normalized(&p.path))
        {
            return Err("Invalid or duplicate runtime pin".into());
        }
        total = total.checked_add(p.bytes).ok_or("Pin byte overflow")?;
    }
    if total > 1024 * 1024 * 1024 {
        return Err("Runtime input closure exceeds cap".into());
    }
    Ok(m)
}
/// Pure validation only. Caller bytes cannot create any runtime authority.
pub fn validate_manifest(raw: &[u8], request: &ApprovalRequestV1, now: u64) -> Result<()> {
    parse(raw, request, now).map(|_| ())
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|e| e.to_string())
}
fn observe(request: &ApprovalRequestV1) -> Result<()> {
    validate_request(request)?;
    let env = |k| std::env::var(k).unwrap_or_default();
    let pairs = [
        ("GITHUB_REPOSITORY", request.repository.clone()),
        ("GITHUB_REF", request.source_ref.clone()),
        ("GITHUB_WORKFLOW_REF", request.workflow_ref.clone()),
        ("GITHUB_WORKFLOW_SHA", request.workflow_sha.clone()),
        ("GITHUB_SHA", request.source_sha.clone()),
        ("GITHUB_RUN_ID", request.run_id.to_string()),
        ("GITHUB_RUN_ATTEMPT", request.attempt.to_string()),
        ("FOCUSFLOW_AUDIO_JOB_ID", request.job_id.to_string()),
        ("GITHUB_JOB", request.job_name.clone()),
        ("RUNNER_NAME", request.runner_name.clone()),
        ("ImageOS", request.runner_image.clone()),
        ("RUNNER_ARCH", request.arch.clone()),
        ("RUNNER_OS", request.os.clone()),
        ("RUNNER_ENVIRONMENT", request.environment.clone()),
        ("GITHUB_ACTIONS", "true".into()),
    ];
    if pairs.iter().any(|(k, v)| env(k) != *v) {
        return Err("Observed process context differs from request".into());
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delivery {
    schema_version: u32,
    commit: String,
    blob: String,
    publication_blob: String,
    manifest: String,
    publication_raw: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    schema_version: u32,
    request: ApprovalRequestV1,
}
fn delivered(raw: &[u8], request: &ApprovalRequestV1, now: u64) -> Result<RemoteApproval> {
    bounds(raw, 512 * 1024)?;
    let d: Delivery =
        serde_json::from_slice(raw).map_err(|_| "Invalid authenticated delivery receipt")?;
    if d.schema_version != 1
        || !hex(&d.commit, 40)
        || !hex(&d.blob, 40)
        || !hex(&d.publication_blob, 40)
    {
        return Err("Conflicting immutable publication receipt".into());
    }
    bounds(d.publication_raw.as_bytes(), MANIFEST_CAP)?;
    let publication: Publication =
        serde_json::from_str(&d.publication_raw).map_err(|_| "Invalid exact publication schema")?;
    if publication.schema_version != 1 || publication.request != *request {
        return Err("Conflicting publication request".into());
    }
    let manifest = parse(d.manifest.as_bytes(), request, now)?;
    Ok(RemoteApproval {
        manifest,
        commit: d.commit,
        blob: d.blob,
        publication_blob: d.publication_blob,
        raw: d.manifest.into_bytes(),
    })
}
/// The only production RemoteApproval constructor. No local manifest or transport override exists.
pub fn fetch_approval(
    request: &ApprovalRequestV1,
    active: &impl Fn() -> Result<()>,
) -> Result<RemoteApproval> {
    active()?;
    observe(request)?;
    let permit = TransportPermit {
        request: request.clone(),
    };
    let raw = helper::fetch(&permit, active)?;
    delivered(&raw, request, now()?)
}
/// Startup diagnostics reuse only the hosted transport binding; no remote authority is minted.
#[cfg(windows)]
pub(super) fn startup_permit(request: &ApprovalRequestV1) -> Result<TransportPermit> {
    observe(request)?;
    Ok(TransportPermit { request: request.clone() })
}
/// Actual measurements are recomputed; observed request fields never supply expected hashes.
pub fn authorize_discovery(source_root: &Path, remote: RemoteApproval) -> Result<DiscoveryPermit> {
    observe(&remote.manifest.request)?;
    parse(&remote.raw, &remote.manifest.request, now()?)?;
    #[cfg(windows)]
    {
        authorize_actual(source_root, remote)
    }
    #[cfg(not(windows))]
    {
        let _ = (source_root, remote);
        Err("Discovery admission requires Windows CNG".into())
    }
}
#[cfg(windows)]
fn authorize_actual(source_root: &Path, remote: RemoteApproval) -> Result<DiscoveryPermit> {
    let root = source_root.canonicalize().map_err(|e| e.to_string())?;
    let workspace =
        std::env::var_os("GITHUB_WORKSPACE").ok_or("Missing observed frozen workspace")?;
    let workspace = PathBuf::from(workspace)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if root != workspace || root != source_root || !root.is_dir() {
        return Err("Source root must be a canonical directory".into());
    }
    crate::wav::reject_links(&root)?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut observations = Vec::new();
    let locks = measure(&remote.manifest, |name| {
        let path = if name == "@executable" {
            executable.clone()
        } else {
            root.join(name)
        };
        let (actual, lock) = super::fingerprint_locked(name, &path)?;
        observations.push(actual.clone());
        Ok((actual, lock))
    })?;
    let digest = super::cng::digest_bytes(&remote.raw)?;
    parse(&remote.raw, &remote.manifest.request, now()?)?;
    Ok(DiscoveryPermit {
        _remote: remote,
        _executable: executable,
        _locks: locks,
        _manifest_sha256: digest,
        root,
        observations,
    })
}
fn namespace(request: &ApprovalRequestV1) -> String {
    format!(
        "audio-discovery-{}-{}-{}",
        request.run_id, request.attempt, request.request_id
    )
}
impl DiscoveryPermit {
    pub(super) fn check(&self) -> Result<()> {
        parse(&self._remote.raw, &self._remote.manifest.request, now()?)?;
        if self._remote.manifest.request.output_namespace
            != namespace(&self._remote.manifest.request)
        {
            return Err("Discovery namespace must derive from authenticated request".into());
        }
        Ok(())
    }
    pub(super) fn init(&self, nonce: String) -> Result<helper::HelperInitV1> {
        self.check()?;
        Ok(helper::HelperInitV1 {
            nonce,
            approval_commit: self._remote.commit.clone(),
            approval_digest: self._manifest_sha256.clone(),
            request_digest: self._remote.manifest.request.request_sha256.clone(),
            phase: "DiscoverRender".into(),
            output_namespace: namespace(&self._remote.manifest.request),
        })
    }
    pub(super) fn root(&self) -> &Path {
        &self.root
    }
    pub(super) fn executable(&self) -> &Path {
        &self._executable
    }
    pub(super) fn record(&self) -> &[u8] {
        &self._remote.raw
    }
    pub(super) fn receipt(&self) -> serde_json::Value {
        serde_json::json!({"commit":self._remote.commit,"manifest_blob":self._remote.blob,"publication_blob":self._remote.publication_blob,"manifest_sha256":self._manifest_sha256,"manifest":String::from_utf8_lossy(&self._remote.raw),"request":self._remote.manifest.request,"actual_fingerprints":self.observations.iter().map(|i|serde_json::json!({"path":i.path,"sha256":i.sha256,"bytes":i.bytes})).collect::<Vec<_>>()})
    }
}
fn delegated_manifest(
    raw: &[u8],
    init: &helper::HelperInitV1,
    actual_digest: &str,
    time: u64,
) -> Result<Manifest> {
    bounds(raw, MANIFEST_CAP)?;
    if actual_digest != init.approval_digest || init.phase != "DiscoverRender" {
        return Err("Internal admission digest or phase mismatch".into());
    }
    let envelope: Manifest =
        serde_json::from_slice(raw).map_err(|_| "Invalid internal admission record")?;
    let manifest = parse(raw, &envelope.request, time)?;
    if manifest.request.request_sha256 != init.request_digest
        || manifest.request.output_namespace != init.output_namespace
        || namespace(&manifest.request) != init.output_namespace
    {
        return Err("Internal admission request namespace mismatch".into());
    }
    Ok(manifest)
}
#[cfg(windows)]
pub(super) fn admit_internal(
    root: &Path,
    init: &helper::HelperInitV1,
    raw: &[u8],
) -> Result<(u64, Vec<File>)> {
    let digest = super::cng::digest_bytes(raw)?;
    let manifest = delegated_manifest(raw, init, &digest, now()?)?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let locks = measure(&manifest, |name| {
        super::fingerprint_locked(
            name,
            &if name == "@executable" {
                executable.clone()
            } else {
                root.join(name)
            },
        )
    })?;
    delegated_manifest(raw, init, &digest, now()?)?;
    Ok((manifest.expires, locks))
}
fn measure<T>(
    manifest: &Manifest,
    mut fingerprint: impl FnMut(&str) -> Result<(super::Identity, T)>,
) -> Result<Vec<T>> {
    let mut locks = Vec::new();
    for pin in std::iter::once(&manifest.executable)
        .chain(std::iter::once(&manifest.input_map))
        .chain(&manifest.files)
    {
        let (actual, lock) = fingerprint(&pin.path)?;
        compare(pin, &actual)?;
        locks.push(lock);
    }
    Ok(locks)
}
fn compare(pin: &Pin, actual: &super::Identity) -> Result<()> {
    if pin.path != actual.path || pin.bytes != actual.bytes || pin.sha256 != actual.sha256 {
        return Err(format!("Actual file identity mismatch: {}", pin.path));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> (ApprovalRequestV1, Vec<u8>) {
        let r = ApprovalRequestV1 {
            schema_version: 1,
            repository: REPOSITORY.into(),
            source_ref: SOURCE_REF.into(),
            workflow_ref: WORKFLOW_REF.into(),
            workflow_sha: "a".repeat(40),
            source_sha: "b".repeat(40),
            run_id: 1,
            attempt: 1,
            job_id: 2,
            job_name: "admission".into(),
            runner_name: "Hosted Agent".into(),
            runner_image: "windows-2022".into(),
            arch: "X64".into(),
            os: "Windows".into(),
            environment: "github-hosted".into(),
            actions: true,
            request_id: "c".repeat(32),
            request_sha256: "d".repeat(64),
            output_namespace: "proof/request".into(),
        };
        let pin = |path| serde_json::json!({"path":path,"sha256":"a".repeat(64),"bytes":10});
        let raw=serde_json::to_vec(&serde_json::json!({"schema_version":1,"review":"root-review","request":r,"issued":100,"not_before":100,"expires":1000,"phase":"DiscoverRender","budget":{"discovery_seconds":10,"phase_seconds":600},"executable":pin("@executable"),"input_map":pin("review/inputs.json"),"files":[pin("src/lib.rs")]})).unwrap();
        (r, raw)
    }
    #[test]
    fn changed_actual_exe_source_denied() {
        let (r, raw) = sample();
        let m = parse(&raw, &r, 101).unwrap();
        for changed in ["@executable", "review/inputs.json", "src/lib.rs"] {
            for fault in 0..4 {
                assert!(measure(&m, |name| {
                    if name == changed && fault == 3 {
                        return Err("actual file unreadable".into());
                    }
                    let mut identity = super::super::Identity {
                        path: name.into(),
                        sha256: "a".repeat(64),
                        bytes: 10,
                    };
                    if name == changed {
                        match fault {
                            0 => identity.sha256 = "f".repeat(64),
                            1 => identity.bytes += 1,
                            2 => identity.path = "substitution".into(),
                            _ => (),
                        }
                    }
                    Ok((identity, ()))
                })
                .is_err());
            }
        }
        let mut observed = Vec::new();
        let locks = measure(&m, |name| {
            observed.push(name.to_string());
            Ok((
                super::super::Identity {
                    path: name.into(),
                    sha256: "a".repeat(64),
                    bytes: 10,
                },
                (),
            ))
        })
        .unwrap();
        assert_eq!(
            observed,
            ["@executable", "review/inputs.json", "src/lib.rs"]
        );
        assert_eq!(locks.len(), 3);
    }
    #[test]
    fn authenticated_delivery_requires_exact_publication_and_receipts() {
        let (r, raw) = sample();
        let publication =
            serde_json::to_string(&serde_json::json!({"schema_version":1,"request":r})).unwrap();
        let receipt = serde_json::json!({"schema_version":1,"commit":"a".repeat(40),"blob":"b".repeat(40),"publication_blob":"c".repeat(40),"manifest":String::from_utf8(raw).unwrap(),"publication_raw":publication});
        let remote = delivered(&serde_json::to_vec(&receipt).unwrap(), &r, 101).unwrap();
        assert_eq!(remote.delivery_ids().0, "a".repeat(40));
        for key in ["commit", "blob", "publication_blob"] {
            let mut bad = receipt.clone();
            bad[key] = "invalid".into();
            assert!(delivered(&serde_json::to_vec(&bad).unwrap(), &r, 101).is_err());
        }
        for publication in [
            format!("{{\"schema_version\":1,{}", &publication[1..]),
            publication.replace("\"attempt\":1", "\"attempt\":2"),
        ] {
            let mut bad = receipt.clone();
            bad["publication_raw"] = publication.into();
            assert!(delivered(&serde_json::to_vec(&bad).unwrap(), &r, 101).is_err());
        }
    }
    #[test]
    fn manifest_bounds_are_not_reset_by_strings_or_whitespace() {
        assert!(bounds(
            format!("{}0{}", "[".repeat(17), "]".repeat(17)).as_bytes(),
            MANIFEST_CAP
        )
        .is_err());
        assert!(bounds(br#"{"x":"[[[[[[[[[[[[[[[[[[[[[[["}"#, MANIFEST_CAP).is_ok());
        assert!(bounds(&vec![b' '; MANIFEST_CAP + 1], MANIFEST_CAP).is_err());
    }
    #[test]
    fn internal_delegation_binds_authenticated_bytes_and_request_namespace() {
        let (mut request, _) = sample();
        request.output_namespace = format!("audio-discovery-1-1-{}", request.request_id);
        let (_, raw) = sample();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["request"] = serde_json::to_value(&request).unwrap();
        let raw = serde_json::to_vec(&value).unwrap();
        let init = super::helper::HelperInitV1 {
            nonce: "12345678-1234-1234-1234-123456789abc".into(),
            approval_commit: "a".repeat(40),
            approval_digest: "b".repeat(64),
            request_digest: request.request_sha256.clone(),
            phase: "DiscoverRender".into(),
            output_namespace: request.output_namespace.clone(),
        };
        assert!(delegated_manifest(&raw, &init, &"b".repeat(64), 101).is_ok());
        assert!(delegated_manifest(&raw, &init, &"c".repeat(64), 101).is_err());
        assert!(delegated_manifest(&raw, &init, &"b".repeat(64), 1000).is_err());
        for field in ["output_namespace", "request_sha256"] {
            let mut value = value.clone();
            value["request"][field] = "different".into();
            assert!(delegated_manifest(
                &serde_json::to_vec(&value).unwrap(),
                &init,
                &"b".repeat(64),
                101
            )
            .is_err());
        }
        let mut changed = init.clone();
        changed.output_namespace = "audio-discovery-other".into();
        assert!(delegated_manifest(&raw, &changed, &"b".repeat(64), 101).is_err());
    }
}
