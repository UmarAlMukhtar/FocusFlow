#![cfg(unix)]
mod common;
use common::TestDir;
use focusflow_core::{ffmpeg::probe_video, ErrorCode};
use std::{fs, os::unix::fs::PermissionsExt, path::{Path, PathBuf}, time::{Duration, Instant}};

// Other concurrently spawned fixtures can briefly inherit a script while it is being written.
static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn executable(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("probe-fixture");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn assert_video_error(error: focusflow_core::CoreError, text: &str) {
    assert_eq!(error.code, ErrorCode::EncoderFailed);
    assert_eq!(error.field.as_deref(), Some("video"));
    assert!(error.message.contains(text), "{}", error.message);
}

#[test]
fn stalled_metadata_probe_returns_and_reaps_owned_child() {
    let _guard = FIXTURES.lock().unwrap();
    let dir = TestDir::new();
    let tool = executable(dir.path(), "echo $$ > \"$0.pid\"\nexec sleep 30");
    let start = Instant::now();
    let error = probe_video(&tool, Path::new("screen.mp4")).unwrap_err();
    assert_video_error(error, "timed out");
    assert!(start.elapsed() < Duration::from_secs(18));
    assert_reaped(&tool);
}

fn assert_reaped(tool: &Path) {
    let pid = fs::read_to_string(tool.with_extension("pid")).unwrap();
    assert!(!std::process::Command::new("kill").args(["-0", pid.trim()]).stderr(std::process::Stdio::null()).status().unwrap().success(), "probe child still exists");
}

#[test]
fn exited_parent_with_descendant_holding_stderr_cannot_block_deadline() {
    let _guard = FIXTURES.lock().unwrap();
    let dir = TestDir::new();
    let tool = executable(dir.path(), "sleep 30 >&2 &\necho $! > \"$0.descendant\"\nexit 0");
    let start = Instant::now();
    let error = probe_video(&tool, Path::new("screen.mp4")).unwrap_err();
    // Only this fixture owns this descendant; the production probe owns its direct child.
    let pid = fs::read_to_string(tool.with_extension("descendant")).unwrap();
    assert!(std::process::Command::new("kill").arg(pid.trim()).status().unwrap().success());
    assert_video_error(error, "timed out");
    assert!(start.elapsed() < Duration::from_secs(18));
}

#[test]
fn oversized_diagnostics_reap_producer_before_deadline() {
    let _guard = FIXTURES.lock().unwrap();
    let dir = TestDir::new();
    let tool = executable(dir.path(), "echo $$ > \"$0.pid\"\nexec yes x >&2");
    let start = Instant::now();
    assert_video_error(probe_video(&tool, Path::new("screen.mp4")).unwrap_err(), "96 KiB");
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_reaped(&tool);
}

#[test]
fn standalone_probe_discards_unrequested_stdout() {
    let _guard = FIXTURES.lock().unwrap();
    let dir = TestDir::new();
    let tool = executable(dir.path(), "head -c 1048576 /dev/zero\nprintf 'Duration: 00:00:01.10, start: 0\nStream #0:0: Video: h264, yuv420p, 100x50\n' >&2");
    let (stdout, stderr) = focusflow_core::ffmpeg::run_probe(std::process::Command::new(tool), false).unwrap();
    assert!(stdout.is_empty());
    assert!(String::from_utf8(stderr).unwrap().contains("Duration:"));
}

#[test]
fn probe_rejects_malformed_and_oversized_metadata() {
    let _guard = FIXTURES.lock().unwrap();
    for (body, message) in [
        ("printf 'not metadata' >&2", "precise video evidence"),
        ("head -c 1048576 /dev/zero >&2", "96 KiB"),
    ] {
        let dir = TestDir::new();
        let tool = executable(dir.path(), body);
        assert_video_error(probe_video(&tool, Path::new("screen.mp4")).unwrap_err(), message);
    }
}

#[test]
fn probe_reports_early_process_failure_and_spawn_failure() {
    let _guard = FIXTURES.lock().unwrap();
    let dir = TestDir::new();
    let tool = executable(dir.path(), "printf 'unsupported recording' >&2\nexit 7");
    assert_video_error(probe_video(&tool, Path::new("screen.mp4")).unwrap_err(), "unsupported recording");
    assert_video_error(probe_video(&dir.path().join("absent"), Path::new("screen.mp4")).unwrap_err(), "Could not start");
}

#[test]
fn bounded_probe_captures_framehash_stdout_without_confusing_negative_dts() {
    let _guard = FIXTURES.lock().unwrap();
    let dir=TestDir::new();
    let tool=executable(dir.path(),"printf '#format: frame checksums\\n#version: 2\\n#tb 0: 1/15360\\n0, -1024, 0, 512, 1234, ab12cd34\\n'");
    let (stdout,stderr)=focusflow_core::ffmpeg::run_probe(std::process::Command::new(tool),true).unwrap();
    assert!(String::from_utf8(stdout).unwrap().contains("0, -1024, 0, 512"));
    assert!(stderr.is_empty());
}

#[test]
fn bounded_probe_rejects_unsupported_encoder_and_oversized_stdout() {
    let _guard = FIXTURES.lock().unwrap();
    for (body,message) in [("echo 'Unrecognized option stats_enc_pre' >&2; exit 1","Unrecognized option"),("exec yes stats","96 KiB")] {
        let dir=TestDir::new();let tool=executable(dir.path(),body);
        let started=Instant::now();
        assert_video_error(focusflow_core::ffmpeg::run_probe(std::process::Command::new(tool),true).unwrap_err(),message);
        assert!(started.elapsed()<Duration::from_secs(3));
    }
}

const HEADER: &str = "#format: frame checksums\n#version: 2\n#hash: adler32\n#extradata 0, 44, 11420d96\n#software: Lavf62.12.103\n#tb 0: 1/30\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 100x50\n#sar 0: 0/1\n#stream#, dts,        pts, duration,     size, hash\n";
const FIRST: &str = "0, -2, 0, 1, 1234, ab12cd34\n";
const TAIL: &str = "0, 88, 90, 1, 1234, ab12cd34\n0, 89, 89, 1, 1234, ab12cd34\n";

fn paired_tool(dir: &Path, first: &str, tail: &str) -> PathBuf {
    let tool = executable(dir, "printf '%s\\n' \"$@\" >> \"$0.argv\"\nprintf 'Duration: 00:00:03.03, start: 0\nStream #0:0: Video: h264, yuv420p, 100x50\n' >&2\ncase \" $* \" in *' -sseof -5 '*) cat \"$0.tail\";; *) cat \"$0.first\";; esac");
    fs::write(tool.with_extension("first"), first).unwrap();
    fs::write(tool.with_extension("tail"), tail).unwrap();
    tool
}

#[test]
fn precise_probe_uses_maximum_presentation_endpoint_and_constrained_pair() {
    let _guard = FIXTURES.lock().unwrap();
    let dir = TestDir::new();
    let tool = paired_tool(dir.path(), &(HEADER.to_owned()+FIRST), &(HEADER.to_owned()+TAIL));
    let v = probe_video(&tool, Path::new("screen.mp4")).unwrap();
    assert_eq!((v.width, v.height, v.duration_us), (100, 50, 3_033_333));
    let args = fs::read_to_string(tool.with_extension("argv")).unwrap();
    for argument in ["-copyts", "file,pipe", "mov", "-enable_drefs", "-use_absolute_path", "0:v:0", "-an", "copy", "framehash", "adler32", "pipe:1"] {
        assert_eq!(args.lines().filter(|s| *s == argument).count(), 2, "{argument}: {args}");
    }
    assert_eq!(args.lines().filter(|s| *s == "-frames:v").count(), 1);
    assert_eq!(args.lines().filter(|s| *s == "-sseof").count(), 1);
    assert!(args.contains("-sseof\n-5\n-protocol_whitelist"));
}

#[test]
fn precise_probe_rejects_bad_evidence_in_either_phase_despite_duration_diagnostics() {
    let _guard = FIXTURES.lock().unwrap();
    let good = HEADER.to_owned()+FIRST;
    let mut bad: Vec<String> = vec![String::new(), good.trim_end().into(), format!("{good}#version: 2\n"), good.replace("\n", "\r"), good.replace("100x50", "100\tx50")];
    for line in HEADER.lines().filter(|l| !l.starts_with("#software") && !l.starts_with("#extradata")) {
        bad.push(good.replace(&format!("{line}\n"), ""));
        bad.push(good.replace(line, &format!("{line}\n{line}")));
    }
    for (old, new) in [
        ("#version: 2", "#version: 1"), ("adler32", "sha256"), ("video", "audio"), ("h264", "mpeg4"),
        ("#tb 0: 1/30", "#tb 1: 1/30"), ("1/30", "0/30"), ("1/30", "1/0"), ("1/30", "01/30"), ("1/30", "1/2147483648"),
        ("100x50", "0x50"), ("100x50", "16385x50"), ("100x50", "100x-50"), ("100x50", "0100x50"),
        ("0/1", "0/2"), ("0/1", "-1/1"), ("0/1", "1/0"), ("0/1", "2147483648/1"),
        ("Lavf62.12.103", "../../bad"), ("44, 11420d96", "0, 11420d96"), ("44, 11420d96", "44, bad"),
        ("0, -2, 0, 1,", "1, -2, 0, 1,"), ("-2, 0,", "-2, -1,"), ("-2, 0,", "N/A, 0,"),
        ("-2, 0,", "-9223372036854775808, 0,"), ("-2, 0,", "9223372036854775807, 0,"),
        ("-2, 0,", "-0, 0,"), ("-2, 0,", "-02, 0,"), ("-2, 0,", "-2, +0,"), ("-2, 0,", "-2, 00,"),
        ("0, 1, 1234", "0, 0, 1234"), ("0, 1, 1234", "0, -1, 1234"), ("0, 1, 1234", "0, 1.0, 1234"),
        ("1, 1234,", "1, 0,"), ("ab12cd34", "ab12cd3"), ("ab12cd34", "ab12cd3g"), ("ab12cd34", "ab12cd34, extra"),
        ("#software: Lavf62.12.103", "#unknown: x"), ("#software: Lavf62.12.103", "#software: Lavf62\n#software: Lavf62"),
        ("#extradata 0, 44, 11420d96", "#extradata 0, 44, 11420d96\n#extradata 0, 44, 11420d96"),
    ] { bad.push(good.replace(old, new)); }
    bad.push(format!("{HEADER}0, 0, 9223372036854775806, 2, 1, ab12cd34\n"));
    bad.push(good.replace("#software: Lavf62.12.103", &format!("#software: {}", "a".repeat(1014))));
    for (i, invalid) in bad.iter().enumerate() {
        for tail in [false, true] {
            let dir = TestDir::new();
            let tool = paired_tool(dir.path(), if tail { &good } else { invalid }, if tail { invalid } else { &good });
            let e = probe_video(&tool, Path::new("screen.mp4")).expect_err(&format!("accepted case {i} tail={tail}: {invalid}"));
            assert_video_error(e.clone(), "precise");
            assert!(e.message.contains("retained"), "{}", e.message);
        }
    }
}

#[test]
fn precise_probe_matches_rationals_and_extradata_and_floors_fractional_cadence() {
    let _guard = FIXTURES.lock().unwrap();
    let good = HEADER.to_owned()+FIRST;
    let dir = TestDir::new();
    let tail = (HEADER.to_owned()+TAIL).replace("1/30", "2/60").replace("ab12cd34", "AB12CD34").replace("11420d96", "11420D96").replace("Lavf62.12.103", "Lavf61.7.100").replace('\n', "\r\n");
    assert_eq!(probe_video(&paired_tool(dir.path(), &good, &tail), Path::new("screen.mp4")).unwrap().duration_us, 3_033_333);
    for (first, tail, expected) in [
        (good.replace("1/30", "1001/30000"), (HEADER.to_owned()+TAIL).replace("1/30", "1001/30000"), 3_036_366),
        (good.replace("0/1", "1/1"), (HEADER.to_owned()+TAIL).replace("0/1", "2/2"), 3_033_333),
        (good.replace("1/30", "1/1"), (HEADER.to_owned()+"0, 86399, 86399, 1, 1, ab12cd34\n").replace("1/30", "1/1"), 86_400_000_000),
    ] {
        let dir = TestDir::new();
        assert_eq!(probe_video(&paired_tool(dir.path(), &first, &tail), Path::new("screen.mp4")).unwrap().duration_us, expected);
    }
    for tail in [
        good.replace("1/30", "1/60"), good.replace("100x50", "102x50"), good.replace("0/1", "1/1"),
        good.replace("44, 11420d96", "45, 11420d96"), good.replace("11420d96", "11420d97"),
        good.replace("#extradata 0, 44, 11420d96\n", ""),
    ] {
        let dir = TestDir::new();
        assert_video_error(probe_video(&paired_tool(dir.path(), &good, &tail), Path::new("screen.mp4")).unwrap_err(), "precise");
    }
    for first in [format!("{good}{FIRST}"), good.replace("-2, 0,", "-2, 1,")] {
        let dir = TestDir::new();
        assert_video_error(probe_video(&paired_tool(dir.path(), &first, &good), Path::new("screen.mp4")).unwrap_err(), "precise");
    }
    // One half-microsecond beyond 24h must fail even though flooring would hide it.
    let dir = TestDir::new();
    let first = good.replace("1/30", "1/2000000");
    let tail = (HEADER.to_owned()+"0, 172800000000, 172800000000, 1, 1, ab12cd34\n").replace("1/30", "1/2000000");
    assert_video_error(probe_video(&paired_tool(dir.path(), &first, &tail), Path::new("screen.mp4")).unwrap_err(), "24 hours");
}

#[test]
fn precise_evidence_line_header_and_row_bounds_are_explicit() {
    let _guard = FIXTURES.lock().unwrap();
    let first = HEADER.to_owned()+FIRST;
    let line_limit = HEADER.replace("#software: Lavf62.12.103", &format!("#software: {}", "a".repeat(1013)));
    for (tail, expected) in [
        (line_limit.clone()+FIRST, Ok(33_333)),
        (line_limit.replace(&"a".repeat(1013), &"a".repeat(1014))+FIRST, Err("1024-byte")),
        (HEADER.to_owned()+&"0,0,0,1,1,00000000\n".repeat(4096), Ok(33_333)),
        (HEADER.to_owned()+&"0,0,0,1,1,00000000\n".repeat(4097), Err("4096-packet")),
        ("#version: 2\n".repeat(64)+FIRST, Err("Duplicate")),
        ("#version: 2\n".repeat(65)+FIRST, Err("64-header")),
        (HEADER.to_owned(), Err("Missing video packets")),
    ] {
        let dir = TestDir::new();
        let result = probe_video(&paired_tool(dir.path(), &first, &tail), Path::new("screen.mp4"));
        match expected { Ok(duration) => assert_eq!(result.unwrap().duration_us,duration), Err(message) => assert_video_error(result.unwrap_err(),message) }
    }
}

#[test]
fn captured_pipe_limit_is_inclusive_and_both_pipes_are_bounded() {
    let _guard = FIXTURES.lock().unwrap();
    for pipe in ["", " >&2"] {
        for size in [96*1024, 96*1024+1] {
            let dir = TestDir::new();
            let tool = executable(dir.path(), &format!("head -c {size} /dev/zero{pipe}"));
            let result = focusflow_core::ffmpeg::run_probe(std::process::Command::new(tool), true);
            if size == 96*1024 {
                let (stdout, stderr) = result.unwrap();
                assert_eq!(if pipe.is_empty() { stdout.len() } else { stderr.len() }, size);
            } else { assert_video_error(result.unwrap_err(), "96 KiB"); }
        }
    }
}

#[test]
fn failed_tail_probe_retains_legacy_files_and_never_publishes_project() {
    use focusflow_core::{legacy::LegacyDefaults, store::ProjectStore};
    let _guard = FIXTURES.lock().unwrap();
    for failure in ["printf 'unsupported tail' >&2; exit 7", "exec yes x", "exec yes x >&2", "printf 'truncated'", "exit 0"] {
        let dir = TestDir::new();
        let project = dir.path().join("recording"); fs::create_dir(&project).unwrap();
        let source = project.join("screen.mp4"); fs::write(&source, b"retained fixture source").unwrap();
        let tool = paired_tool(dir.path(), &(HEADER.to_owned()+FIRST), &(HEADER.to_owned()+TAIL));
        let body = format!("echo $$ > \"$0.pid\"\ncase \" $* \" in *' -sseof -5 '*) {failure};; *) cat \"$0.first\";; esac");
        executable(dir.path(), &body);
        let defaults = LegacyDefaults { scale: 2.0, intro_us: 200_000, hold_us: 300_000, outro_us: 200_000, pan_transition_us: 200_000, settle_us: 100_000 };
        let error = ProjectStore::new(dir.path()).unwrap().open_or_import("recording", &tool, defaults).unwrap_err();
        assert_video_error(error, "retained");
        assert_reaped(&tool);
        assert!(!project.join("project.json").exists());
        assert_eq!(fs::read(source).unwrap(), b"retained fixture source");
    }
}
