use focusflow_windows_audio_worker::proof::guard::helper::{
    collect_inventory, validate_helper_init, HelperInitV1, RenderEndpoint, RenderInventoryOps,
};
use focusflow_windows_audio_worker::Result;
struct Inventory {
    count: u32,
    endpoint: RenderEndpoint,
    calls: Vec<&'static str>,
}
impl RenderInventoryOps for Inventory {
    fn count(&mut self) -> Result<u32> {
        self.calls.push("render-active-count");
        Ok(self.count)
    }
    fn item(&mut self, index: u32) -> Result<RenderEndpoint> {
        self.calls.push("identity-state-flow-label");
        let mut endpoint = self.endpoint.clone();
        if self.count > 1 {
            endpoint.id.push_str(&index.to_string());
        }
        Ok(endpoint)
    }
}
fn inventory() -> Inventory {
    Inventory {
        count: 1,
        endpoint: RenderEndpoint {
            id: "owned-render".into(),
            label: "label".into(),
            state: 1,
            flow: 0,
        },
        calls: vec![],
    }
}
#[test]
fn discovery_render_only_no_activate() {
    let mut ops = inventory();
    let result = collect_inventory(&mut ops).unwrap();
    assert_eq!(result[0].id, "owned-render");
    assert_eq!(
        ops.calls,
        ["render-active-count", "identity-state-flow-label"]
    );
    ops.count = 0;
    assert!(collect_inventory(&mut ops).unwrap().is_empty());
}
#[test]
fn discovery_caps_fail_closed() {
    let mut ops = inventory();
    ops.count = 33;
    assert!(collect_inventory(&mut ops).is_err());
    assert_eq!(ops.calls.len(), 1);
    for (id, label, state, flow) in [
        ("x".repeat(2049), "ok".into(), 1, 0),
        ("id".into(), "x".repeat(513), 1, 0),
        ("id".into(), "ok".into(), 2, 0),
        ("id".into(), "ok".into(), 1, 1),
        ("".into(), "ok".into(), 1, 0),
    ] {
        let mut ops = inventory();
        ops.endpoint = RenderEndpoint {
            id,
            label,
            state,
            flow,
        };
        assert!(collect_inventory(&mut ops).is_err());
    }
    let mut ops = inventory();
    ops.count = 32;
    ops.endpoint.id = "x".repeat(1990);
    ops.endpoint.label = "x".repeat(512);
    assert!(collect_inventory(&mut ops).is_err());
}
fn init() -> HelperInitV1 {
    HelperInitV1 {
        nonce: "12345678-1234-1234-1234-123456789abc".into(),
        approval_commit: "a".repeat(40),
        approval_digest: "b".repeat(64),
        request_digest: "c".repeat(64),
        phase: "DiscoverRender".into(),
        output_namespace: format!("audio-discovery-1-1-{}", "d".repeat(32)),
    }
}
#[test]
fn helper_nonce_job_eof_gate() {
    let i = init();
    let raw = serde_json::to_vec(&i).unwrap();
    assert!(validate_helper_init(&raw, &i.nonce, true, 0, false, false).is_ok());
    for (nonce, job, queued, eof, stop) in [
        ("wrong", true, 0, false, false),
        (i.nonce.as_str(), false, 0, false, false),
        (i.nonce.as_str(), true, 1, false, false),
        (i.nonce.as_str(), true, 0, true, false),
        (i.nonce.as_str(), true, 0, false, true),
    ] {
        assert!(validate_helper_init(&raw, nonce, job, queued, eof, stop).is_err());
    }
    let mut altered = i.clone();
    altered.phase = "CollectExactRender".into();
    assert!(validate_helper_init(
        &serde_json::to_vec(&altered).unwrap(),
        &i.nonce,
        true,
        0,
        false,
        false
    )
    .is_err());
    assert!(validate_helper_init(&vec![b' '; 4097], &i.nonce, true, 0, false, false).is_err());
    for raw in [
        format!("{},{{}}", String::from_utf8(raw.clone()).unwrap()),
        String::from_utf8(raw.clone())
            .unwrap()
            .replace("\"nonce\":", "\"extra\":0,\"nonce\":"),
        String::from_utf8(raw)
            .unwrap()
            .replace("\"nonce\":", "\"nonce\":\"other\",\"nonce\":"),
    ] {
        assert!(validate_helper_init(raw.as_bytes(), &i.nonce, true, 0, false, false).is_err());
    }
}
#[test]
fn normal_worker_gate_unchanged() {
    use focusflow_windows_audio_worker::protocol::{
        encode_init, read_init, InitV1, TrackKind, TrackRequest,
    };
    let mut request = TrackRequest {
        kind: TrackKind::System,
        endpoint_id: "id".into(),
        explicitly_enabled: false,
        output_dir: std::env::temp_dir().join("audio"),
    };
    assert!(encode_init(&InitV1 {
        version: 1,
        nonce: "12345678-1234-1234-1234-123456789abc".into(),
        request: request.clone()
    })
    .is_err());
    request.explicitly_enabled = true;
    let raw = encode_init(&InitV1 {
        version: 1,
        nonce: "12345678-1234-1234-1234-123456789abc".into(),
        request,
    })
    .unwrap();
    assert!(read_init(&mut raw.as_slice(), "wrong").is_err());
    assert!(read_init(&mut raw.as_slice(), "12345678-1234-1234-1234-123456789abc").is_ok());
    let helper = serde_json::to_vec(&init()).unwrap();
    assert!(read_init(&mut helper.as_slice(), "n").is_err());
}

#[test]
fn operational_inventory_errors_fail_and_collection_stays_disabled() {
    struct Fault;
    impl RenderInventoryOps for Fault {
        fn count(&mut self) -> Result<u32> {
            Err("COM failure".into())
        }
        fn item(&mut self, _: u32) -> Result<RenderEndpoint> {
            panic!("No item after count failure")
        }
    }
    assert!(collect_inventory(&mut Fault).is_err());
    assert!(focusflow_windows_audio_worker::proof::guard::compiled_approval().is_none());
    assert_eq!(
        focusflow_windows_audio_worker::proof::dispatch(
            ["private.exe", "--private-audio-proof", "id", "output"]
                .into_iter()
                .map(Into::into)
        ),
        77
    );
}
