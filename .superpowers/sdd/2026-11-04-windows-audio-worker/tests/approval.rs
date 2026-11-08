use focusflow_windows_audio_worker::proof::guard::approval::{
    validate_manifest, validate_request, ApprovalRequestV1,
};
use serde_json::{json, Value};
fn request() -> ApprovalRequestV1 {
    serde_json::from_value(json!({"schema_version":1,"repository":"UmarAlMukhtar/FocusFlow","source_ref":"refs/heads/codex-audio-native-qualification","workflow_ref":"UmarAlMukhtar/FocusFlow/.github/workflows/windows-audio-native-qualification.yml@refs/heads/codex-audio-native-qualification","workflow_sha":"a".repeat(40),"source_sha":"b".repeat(40),"run_id":123,"attempt":1,"job_id":456,"job_name":"admission","runner_name":"Hosted Agent","runner_image":"windows-2022","arch":"X64","os":"Windows","environment":"github-hosted","actions":true,"request_id":"c".repeat(32),"request_sha256":"d".repeat(64),"output_namespace":"audio-proof/request"})).unwrap()
}
fn manifest() -> Value {
    json!({"schema_version":1,"review":"root-review-1","request":request(),"issued":100,"not_before":100,"expires":1000,"phase":"DiscoverRender","budget":{"discovery_seconds":10,"phase_seconds":600},"executable":{"path":"@executable","sha256":"e".repeat(64),"bytes":1},"input_map":{"path":"review/input-map.json","sha256":"f".repeat(64),"bytes":1},"files":[{"path":"src/lib.rs","sha256":"a".repeat(64),"bytes":1}]})
}
#[test]
fn local_manifest_cannot_authorize() {
    assert!(validate_manifest(&serde_json::to_vec(&manifest()).unwrap(), &request(), 101).is_ok());
    assert!(focusflow_windows_audio_worker::proof::guard::compiled_approval().is_none());
    // The public validator returns unit, never a capability.
    let _: () =
        validate_manifest(&serde_json::to_vec(&manifest()).unwrap(), &request(), 101).unwrap();
}
#[test]
fn runtime_cannot_fill_expected_fields() {
    for key in ["review", "executable", "input_map", "files"] {
        let mut m = manifest();
        m.as_object_mut().unwrap().remove(key);
        assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
    }
}
#[test]
fn wrong_repo_ref_sha_attempt_request_denied() {
    for key in [
        "repository",
        "source_ref",
        "workflow_ref",
        "workflow_sha",
        "source_sha",
        "request_id",
        "request_sha256",
    ] {
        let mut m = manifest();
        m["request"][key] = "wrong".into();
        assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
    }
    let mut m = manifest();
    m["request"]["attempt"] = 2.into();
    assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
    let mut r = request();
    r.repository = "other/repo".into();
    assert!(validate_request(&r).is_err());
}
#[test]
fn unknown_duplicate_oversize_manifest_denied() {
    let raw = serde_json::to_string(&manifest()).unwrap();
    for bad in [
        format!("{{\"schema_version\":1,{}", &raw[1..]),
        format!("{{\"extra\":1,{}", &raw[1..]),
        format!("{raw}{{}}"),
        " ".repeat(262145),
    ] {
        assert!(validate_manifest(bad.as_bytes(), &request(), 101).is_err());
    }
    for path in [
        "a//b", "a/../b", "a/./b", "a\\b", "C:/a", "a/", "/a", "a\0b", "a./b", "CON/file",
        "a:stream",
    ] {
        let mut m = manifest();
        m["files"][0]["path"] = path.into();
        assert!(
            validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err(),
            "{path:?}"
        );
    }
    for now in [99, 1000, 1001] {
        assert!(validate_manifest(raw.as_bytes(), &request(), now).is_err());
    }
    let mut m = manifest();
    m["files"][0]["bytes"] = 0.into();
    assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
}
#[test]
fn discovery_cannot_construct_collection() {
    let mut m = manifest();
    m["phase"] = "CollectExactRender".into();
    assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
    m = manifest();
    m["route_owned"] = true.into();
    assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
}

#[test]
fn nested_duplicate_unknown_type_and_budget_fields_fail_closed() {
    let raw = serde_json::to_string(&manifest()).unwrap();
    for bad in [
        raw.replacen("\"request\":{", "\"request\":{\"attempt\":1,", 1),
        raw.replacen("\"budget\":{", "\"budget\":{\"discovery_seconds\":10,", 1),
        raw.replacen("\"files\":[{", "\"files\":[{\"bytes\":1,", 1),
    ] {
        assert_ne!(bad, raw);
        assert!(validate_manifest(bad.as_bytes(), &request(), 101).is_err());
    }
    for (field, value) in [
        ("review", json!("bad\u{0000}review")),
        ("phase", json!({"DiscoverRender":null})),
        ("issued", json!(100.0)),
        ("expires", json!(1001)),
        (
            "budget",
            json!({"discovery_seconds":11,"phase_seconds":600}),
        ),
        ("files", json!([])),
    ] {
        let mut m = manifest();
        m[field] = value;
        assert!(
            validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err(),
            "{field}"
        );
    }
    let mut m = manifest();
    m["files"][0]["extra"] = true.into();
    assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
    let mut m = manifest();
    m["files"][0]["bytes"] = json!(512u64 * 1024 * 1024 + 1);
    assert!(validate_manifest(&serde_json::to_vec(&m).unwrap(), &request(), 101).is_err());
}
