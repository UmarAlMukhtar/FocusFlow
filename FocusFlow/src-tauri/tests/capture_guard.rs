#![cfg(unix)]

#[path = "../src/capture_guard.rs"]
#[allow(dead_code)]
mod capture_guard;

use std::{
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

const ENCODER: &str = r#"
import os, signal, sys, time
signal.alarm(25)
mode, receipt = sys.argv[1:3]
with open(receipt + '.pid', 'w') as f: f.write(str(os.getpid()))
if mode.startswith('exit'):
    sys.exit(int(mode[4:]))
if mode == 'args':
    with open(receipt, 'wb') as f:
        f.write(os.fsencode(sys.argv[3]) + b'\n' + os.fsencode(os.getcwd()) + b'\n'
                + os.fsencode(os.environ.get('FF_TEST_SET', 'missing')) + b'\n'
                + os.fsencode(os.environ.get('PATH', 'absent')))
if mode == 'ignore':
    time.sleep(24)
else:
    print('encoder output immediately after spawn', flush=True)
    while sys.stdin.buffer.read(1) not in (b'q', b''):
        pass
    if mode == 'stdout-tail':
        for _ in range(128): os.write(1, b's' * 8192)
        os.write(1, b'END-STATISTICS\n')
    if mode == 'held-stdout':
        child = os.fork()
        if child == 0:
            time.sleep(3)
            os._exit(0)
        with open(receipt + '.descendant', 'w') as f: f.write(str(child))
    if mode == 'no-readers':
        for _ in range(64):
            os.write(1, b'o' * 8192)
            os.write(2, b'e' * 8192)
    with open(receipt + '.stopped', 'w') as f: f.write('finalized')
"#;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    child: Child,
}

impl Fixture {
    fn start(mode: &str, extra: Option<OsString>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "focusflow-guard-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_FocusFlow"));
        command.args(["--focusflow-capture-guard", "--", "/usr/bin/python3", "-u", "-c", ENCODER, mode])
            .arg(root.join("receipt")).current_dir(&root)
            .env_remove("DISPLAY").env("FF_TEST_SET", "native override")
            .env_remove("PATH")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(arg) = extra { command.arg(arg); }
        Self { root, child: command.spawn().unwrap() }
    }

    fn ready(&mut self) -> (u32, BufReader<std::process::ChildStdout>) {
        let stdout = self.child.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let result = reader.read_line(&mut line);
            let _ = send.send((result, line, reader));
        });
        let (result, line, reader) = receive.recv_timeout(Duration::from_secs(4)).expect("bounded startup acknowledgement");
        result.unwrap();
        let pid = line.strip_prefix("FOCUSFLOW_CAPTURE_READY ").expect("encoder readiness header")
            .trim().parse::<u32>().unwrap();
        (pid, reader)
    }

    fn wait(&mut self) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() { return status; }
            assert!(start.elapsed() < Duration::from_secs(12), "guardian must exit in bounded time");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn reaped(&self, pid: u32) {
        // /proc absence verifies the encoder was reaped, not merely signalled.
        #[cfg(target_os = "linux")]
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists(), "encoder {pid} still exists");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.child.stdin.take();
        let start = Instant::now();
        while matches!(self.child.try_wait(), Ok(None)) && start.elapsed() < Duration::from_secs(12) {
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn q_stops_and_reaps_encoder_and_preserves_initial_output() {
    let mut fixture = Fixture::start("normal", None);
    let (pid, mut stdout) = fixture.ready();
    fixture.child.stdin.as_mut().unwrap().write_all(b"q\n").unwrap();
    assert!(fixture.wait().success());
    let mut output = String::new();
    stdout.read_to_string(&mut output).unwrap();
    assert!(output.contains("encoder output immediately after spawn"));
    assert!(fixture.root.join("receipt.stopped").exists());
    fixture.reaped(pid);
}

#[test]
fn eof_finalizes_even_after_parent_log_readers_disappear() {
    let mut fixture = Fixture::start("no-readers", None);
    let (pid, stdout) = fixture.ready();
    drop(stdout);
    fixture.child.stderr.take();
    fixture.child.stdin.take();
    assert!(fixture.wait().success());
    assert!(fixture.root.join("receipt.stopped").exists());
    fixture.reaped(pid);
}

#[test]
fn natural_exit_propagates_success_and_failure_without_waiting_for_control_eof() {
    for (mode, code) in [("exit0", 0), ("exit7", 7)] {
        let mut fixture = Fixture::start(mode, None);
        let (pid, _stdout) = fixture.ready();
        assert_eq!(fixture.wait().code(), Some(code));
        assert!(fixture.child.stdin.is_some());
        fixture.reaped(pid);
    }
}

#[test]
fn stop_ignoring_encoder_is_killed_and_reaped_after_deadline() {
    let mut fixture = Fixture::start("ignore", None);
    let (pid, _stdout) = fixture.ready();
    // Wait for fixture setup so this exercises an encoder actively ignoring stop.
    let start = Instant::now();
    while !fixture.root.join("receipt.pid").exists() {
        assert!(start.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(10));
    }
    fixture.child.stdin.take();
    let start = Instant::now();
    assert!(!fixture.wait().success());
    assert!(start.elapsed() >= Duration::from_secs(7));
    fixture.reaped(pid);
}

#[test]
fn missing_encoder_and_malformed_private_invocations_fail_headlessly() {
    for args in [
        vec!["--focusflow-capture-guard"],
        vec!["--focusflow-capture-guard", "--"],
        vec!["--focusflow-capture-guard", "wrong", "/usr/bin/true"],
        vec!["--focusflow-capture-guard", "--", "/nonexistent/focusflow-encoder"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_FocusFlow")).args(args)
            .env_remove("DISPLAY").output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("gtk"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn native_argument_bytes_directory_and_environment_reach_encoder() {
    let arg = OsString::from_vec(b"spaces ' quotes \" and \xff".to_vec());
    let mut fixture = Fixture::start("args", Some(arg));
    let (pid, _stdout) = fixture.ready();
    fixture.child.stdin.take();
    assert!(fixture.wait().success());
    let mut expected = b"spaces ' quotes \" and \xff\n".to_vec();
    expected.extend_from_slice(fixture.root.as_os_str().as_encoded_bytes());
    expected.extend_from_slice(b"\nnative override\nabsent");
    assert_eq!(fs::read(fixture.root.join("receipt")).unwrap(), expected);
    fixture.reaped(pid);
}

fn launch_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "focusflow-launch-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn encoder_command(root: &std::path::Path, mode: &str) -> Command {
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-u", "-c", ENCODER, mode]).arg(root.join("receipt"))
        .current_dir(root).env("FF_TEST_SET", "copied override").env_remove("PATH");
    command
}

#[test]
fn launch_scope_error_closes_control_and_reaps_both_processes() {
    for wait_ready in [false, true] {
        let root = launch_root();
        let command = encoder_command(&root, "normal");
        let mut launch = capture_guard::Launch::spawn(&command, std::path::Path::new(env!("CARGO_BIN_EXE_FocusFlow"))).unwrap();
        let guardian_pid = launch.child_mut().unwrap().id();
        if wait_ready { launch.wait_ready().unwrap(); }
        // Model an error before publication, including before the ack is consumed.
        drop(launch);
        let pid = fs::read_to_string(root.join("receipt.pid")).unwrap().parse::<u32>().unwrap();
        assert_eq!(fs::read_to_string(root.join("receipt.stopped")).unwrap(), "finalized");
        #[cfg(target_os = "linux")]
        for pid in [pid, guardian_pid] {
            assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn launch_copies_native_arguments_directory_and_explicit_environment() {
    let root = launch_root();
    let mut command = encoder_command(&root, "args");
    command.arg(OsString::from_vec(b"native ' \" \xff argument".to_vec()));
    let mut launch = capture_guard::Launch::spawn(&command, std::path::Path::new(env!("CARGO_BIN_EXE_FocusFlow"))).unwrap();
    launch.wait_ready().unwrap();
    let pid = launch.encoder_pid;
    assert_ne!(pid, launch.child_mut().unwrap().id());
    let mut stdout = launch.stdout.take().unwrap();
    drop(launch);
    let mut output = String::new();
    stdout.read_to_string(&mut output).unwrap();
    assert!(output.starts_with("encoder output immediately after spawn"));
    let mut expected = b"native ' \" \xff argument\n".to_vec();
    expected.extend_from_slice(root.as_os_str().as_encoded_bytes());
    expected.extend_from_slice(b"\ncopied override\nabsent");
    assert_eq!(fs::read(root.join("receipt")).unwrap(), expected);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn startup_header_is_bounded_in_size_and_time_and_errors_release_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    for (mode, expected) in [("delay", "3 second limit"), ("oversize", "512 bytes"), ("malformed", "Invalid encoder")] {
        let root = launch_root();
        let executable = root.join("guardian");
        fs::write(&executable, format!(r#"#!/usr/bin/python3
import sys
mode = {mode:?}
if mode == 'oversize':
    sys.stdout.write('x' * 513)
elif mode == 'malformed':
    sys.stdout.write('arbitrary output\n')
sys.stdout.flush()
sys.stdin.buffer.read()
with open('owner-released', 'w') as f: f.write('yes')
"#)).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let command = encoder_command(&root, "normal");
        let mut launch = capture_guard::Launch::spawn(&command, &executable).unwrap();
        let start = Instant::now();
        let error = launch.wait_ready().unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(start.elapsed() < Duration::from_secs(4));
        drop(launch);
        assert!(root.join("owner-released").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stdout_statistics_tail_completes_before_successful_owner_exit() {
    let mut fixture=Fixture::start("stdout-tail",None);
    let (pid,mut stdout)=fixture.ready();
    fixture.child.stdin.as_mut().unwrap().write_all(b"q\n").unwrap();
    let reader=thread::spawn(move || { thread::sleep(Duration::from_millis(400));let mut bytes=Vec::new();stdout.read_to_end(&mut bytes).unwrap();bytes });
    assert!(fixture.wait().success());
    let bytes=reader.join().unwrap();
    assert!(bytes.ends_with(b"END-STATISTICS\n"));
    assert_eq!(bytes.iter().filter(|&&b|b==b's').count(),128*8192+1);
    fixture.reaped(pid);
}

#[test]
fn retained_stdout_reports_incomplete_relay_within_owner_deadline() {
    let mut fixture=Fixture::start("held-stdout",None);
    let (pid,_stdout)=fixture.ready();
    fixture.child.stdin.take();
    let start=Instant::now();
    assert!(!fixture.wait().success());
    assert!(start.elapsed()<Duration::from_secs(3));
    fixture.reaped(pid);
    let descendant=fs::read_to_string(fixture.root.join("receipt.descendant")).unwrap();
    let _=Command::new("kill").arg(descendant.trim()).status();
}
