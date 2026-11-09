use focusflow_windows_audio_worker::worker::dispatch;
use std::ffi::OsString;

const MODE: &str = "--private-owned-audio-worker";
fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = OsString> + 'a {
    values.iter().map(OsString::from)
}

#[test]
fn ordinary_startup_and_unrelated_modes_are_not_audio_requests() {
    for values in [&[][..], &["--other-mode"][..], &["--other-mode", MODE][..]] {
        assert_eq!(dispatch(args(values)), None);
    }
}

#[test]
fn recognized_worker_without_required_arguments_never_falls_through_to_app() {
    for values in [&[MODE][..], &[MODE, "job"][..], &[MODE, "job", "stop"][..]] {
        assert_eq!(dispatch(args(values)), Some(64));
    }
}

#[test]
fn recognized_worker_rejects_extra_or_invalid_arguments_before_native_dispatch() {
    for values in [
        [MODE, "job", "stop", "nonce", "extra"].as_slice(),
        [MODE, "", "stop", "nonce"].as_slice(),
        [MODE, "job", "", "nonce"].as_slice(),
        [MODE, "job", "stop", ""].as_slice(),
        [MODE, "job\0", "stop", "nonce"].as_slice(),
        [MODE, "job", "stop\0", "nonce"].as_slice(),
        [MODE, "job", "stop", "nonce\0"].as_slice(),
    ] {
        assert_eq!(dispatch(args(values)), Some(64));
    }
}

#[cfg(unix)]
#[test]
fn recognized_worker_rejects_non_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;
    for index in 1..4 {
        let mut values: Vec<_> = args(&[MODE, "job", "stop", "nonce"]).collect();
        values[index] = OsString::from_vec(vec![0xff]);
        assert_eq!(dispatch(values.into_iter()), Some(64));
    }
}

#[cfg(not(windows))]
#[test]
fn valid_skipped_argv_routes_to_unsupported_worker_without_app_fallthrough() {
    assert_eq!(dispatch(args(&[MODE, "job", "stop", "nonce"])), Some(69));
}
