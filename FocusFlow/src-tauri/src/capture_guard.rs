//! A headless encoder owner. Only the app holds the control pipe's writing end.
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::Path,
    process::{Child, ChildStdout, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MODE: &str = "--focusflow-capture-guard";
const READY: &str = "FOCUSFLOW_CAPTURE_READY ";
const ENCODER_STOP_TIMEOUT: Duration = Duration::from_secs(8);
const OWNER_WAIT_TIMEOUT: Duration = Duration::from_secs(12);
const START_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(25);

/// Called before Tauri initialization; unrelated arguments keep the normal launch.
pub fn dispatch(mut args: impl Iterator<Item = OsString>) -> Option<i32> {
    if args.next().as_deref() != Some(std::ffi::OsStr::new(MODE)) {
        return None;
    }
    let result = if args.next().as_deref() == Some(std::ffi::OsStr::new("--")) {
        match args.next() {
            Some(program) if !program.is_empty() => run(Command::new(program).args(args)),
            _ => Err(io::Error::other("Capture guardian requires an encoder executable")),
        }
    } else {
        Err(io::Error::other("Capture guardian requires -- before the encoder executable"))
    };
    Some(match result {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            diagnostic(&error.to_string());
            1
        }
    })
}

fn no_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW, with explicit pipe handles.
    }
    #[cfg(not(windows))]
    let _ = command;
}

fn diagnostic(message: &str) {
    let _ = writeln!(io::stderr(), "[FocusFlow capture guardian] {message}");
}

fn relay(mut source: impl Read, mut destination: impl Write) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];
    let mut forwarding = true;
    loop {
        match source.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(size) => {
                if forwarding {
                    forwarding = destination.write_all(&buffer[..size])
                        .and_then(|_| destination.flush()).is_ok();
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn run(command: &mut Command) -> io::Result<ExitStatus> {
    no_console(command);
    let mut encoder = command.stdin(Stdio::piped()).stdout(Stdio::piped())
        .stderr(Stdio::piped()).spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("Could not start FFmpeg encoder: {e}")))?;

    // Every fallible startup operation remains inside this cleanup boundary.
    let startup = (|| -> io::Result<(mpsc::Receiver<()>, mpsc::Receiver<io::Result<()>>)> {
        {
            let stdout = io::stdout();
            let mut out = stdout.lock();
            writeln!(out, "{READY}{}", encoder.id())?;
            out.flush()?;
        }
        let (stdout_send, stdout_done) = mpsc::sync_channel(1);
        if let Some(stdout) = encoder.stdout.take() {
            thread::Builder::new().name("encoder-stdout".into())
                .spawn(move || { let _ = stdout_send.send(relay(stdout, io::stdout())); })?;
        }
        if let Some(stderr) = encoder.stderr.take() {
            thread::Builder::new().name("encoder-stderr".into())
                .spawn(move || relay(stderr, io::stderr()))?;
        }
        let (send, receive) = mpsc::channel();
        thread::Builder::new().name("capture-control".into()).spawn(move || {
            let input = io::stdin();
            let mut input = input.lock();
            let mut byte = [0];
            loop {
                match input.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) if byte[0] == b'q' => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = send.send(());
        })?;
        Ok((receive, stdout_done))
    })();
    let (control, stdout_done) = match startup {
        Ok(channels) => channels,
        Err(error) => {
            let _ = stop_encoder(&mut encoder);
            return Err(error);
        }
    };

    let status = loop {
        match encoder.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = stop_encoder(&mut encoder);
                return Err(error);
            }
        }
        match control.recv_timeout(POLL) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => break stop_encoder(&mut encoder)?,
        }
    };
    // Statistics must reach EOF before owner exit; keep 8s inner + 1s relay below 12s outer.
    stdout_done.recv_timeout(Duration::from_secs(1))
        .map_err(|_| io::Error::other("Encoder statistics relay did not complete within 1 second"))??;
    Ok(status)
}

fn stop_encoder(encoder: &mut Child) -> io::Result<ExitStatus> {
    if let Some(mut stdin) = encoder.stdin.take() {
        // Closing stdin also requests stop when q cannot be delivered.
        let _ = stdin.write_all(b"q\n").and_then(|_| stdin.flush());
    }
    let start = Instant::now();
    loop {
        match encoder.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if start.elapsed() < ENCODER_STOP_TIMEOUT => thread::sleep(POLL),
            status => {
                let reason = match status {
                    Err(error) => format!("Could not query encoder status: {error}"),
                    _ => "Encoder did not stop within 8 seconds; forcing termination".into(),
                };
                diagnostic(&reason);
                if let Err(error) = encoder.kill() {
                    diagnostic(&format!("Could not kill encoder: {error}"));
                }
                return encoder.wait().map_err(|error| {
                    io::Error::new(error.kind(), format!("Could not reap encoder: {error}"))
                });
            }
        }
    }
}

/// Owns cleanup until the recorder has passed both readiness and stability checks.
pub(crate) struct Launch {
    child: Option<Child>,
    pub(crate) encoder_pid: u32,
    pub(crate) stdout: Option<ChildStdout>,
}

impl Launch {
    pub(crate) fn spawn(command: &Command, guardian: &Path) -> io::Result<Self> {
        let mut owner = Command::new(guardian);
        owner.arg(MODE).arg("--").arg(command.get_program()).args(command.get_args());
        if let Some(directory) = command.get_current_dir() {
            owner.current_dir(directory);
        }
        for (key, value) in command.get_envs() {
            if let Some(value) = value { owner.env(key, value); }
            else { owner.env_remove(key); }
        }
        no_console(&mut owner);
        let child = owner.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| io::Error::new(e.kind(), format!("Could not start capture guardian: {e}")))?;
        Ok(Self { child: Some(child), encoder_pid: 0, stdout: None })
    }

    pub(crate) fn wait_ready(&mut self) -> io::Result<()> {
        let stdout = self.child_mut()?.stdout.take()
            .ok_or_else(|| io::Error::other("Capture guardian stdout was not piped"))?;
        let (send, receive) = mpsc::channel();
        thread::Builder::new().name("capture-ready".into()).spawn(move || {
            let _ = send.send(read_ready(stdout));
        })?;
        let (pid, stdout) = receive.recv_timeout(START_TIMEOUT)
            .map_err(|e| io::Error::other(format!("Encoder startup acknowledgement failed (3 second limit): {e}")))??;
        self.encoder_pid = pid;
        self.stdout = Some(stdout);
        Ok(())
    }

    pub(crate) fn child_mut(&mut self) -> io::Result<&mut Child> {
        self.child.as_mut().ok_or_else(|| io::Error::other("Capture ownership already transferred"))
    }

    pub(crate) fn publish(mut self) -> io::Result<Child> {
        self.child.take().ok_or_else(|| io::Error::other("Capture ownership already transferred"))
    }
}

fn read_ready(mut stdout: ChildStdout) -> io::Result<(u32, ChildStdout)> {
    let mut header = Vec::new();
    let mut byte = [0];
    // Read only the header, leaving all encoder output for the existing log drain.
    while header.len() < 512 {
        stdout.read_exact(&mut byte).map_err(|e| io::Error::new(e.kind(),
            format!("FFmpeg encoder did not acknowledge startup: {e}")))?;
        if byte[0] == b'\n' {
            let pid = std::str::from_utf8(&header).ok().and_then(|line| line.strip_prefix(READY))
                .and_then(|pid| pid.parse::<u32>().ok()).filter(|pid| *pid > 0)
                .ok_or_else(|| io::Error::other("Invalid encoder startup acknowledgement"))?;
            return Ok((pid, stdout));
        }
        header.push(byte[0]);
    }
    Err(io::Error::other("Encoder startup acknowledgement exceeds 512 bytes"))
}

impl Drop for Launch {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            close_and_reap(child);
        }
    }
}

/// On an app-side error, EOF asks the guardian to clean up; never kill its owner.
pub(crate) fn close_and_reap(mut child: Child) {
    child.stdin.take();
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if start.elapsed() < OWNER_WAIT_TIMEOUT => thread::sleep(POLL),
            _ => {
                reap_in_background(child);
                return;
            }
        }
    }
}

pub(crate) fn reap_in_background(mut child: Child) {
    child.stdin.take();
    // A separately hung guardian cannot be recovered portably; retain a reaper.
    let _ = thread::Builder::new().name("capture-reap".into()).spawn(move || {
        let _ = child.wait();
    });
}
