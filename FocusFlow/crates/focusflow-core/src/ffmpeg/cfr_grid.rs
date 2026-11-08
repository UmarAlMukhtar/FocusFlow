//! Whole-file, fixed-profile evidence; no decoded-pixel or physical timing claim.
use super::*;
use std::{io::{self, Read}, sync::atomic::{AtomicBool, Ordering}};

const MAX_FRAMES: u64 = 2_592_000;
const LINE_LIMIT: usize = 1024;

#[derive(Debug, Clone, Copy)]
pub struct SavedCfrGrid {
    pub video: ProbedVideoEvidence,
    pub completed_frames: u64,
}

/// Validate every saved H.264 packet on a zero-start, B0, 30 fps grid.
/// Both pipes must reach EOF and the one owned process must exit successfully.
/// The caller deadline is capped at ten minutes; failure cleanup has its own two-second bound.
pub fn probe_cfr_grid(ffmpeg: &Path, input: &Path, expected_frames: u64,
    cancelled: &AtomicBool, deadline: Instant) -> CoreResult<SavedCfrGrid> {
    let deadline = deadline.min(Instant::now() + Duration::from_secs(600));
    check_running(cancelled, deadline)?;
    let grid = Grid::new(expected_frames)?;
    let mut command = Command::new(ffmpeg);
    command.args(["-v", "error", "-copyts"]).args(input_args(input, true))
        .args(["-map", "0", "-c", "copy", "-f", "framehash", "-hash", "adler32", "pipe:1"]);
    run(command, grid, cancelled, deadline)
}

fn check_running(cancelled: &AtomicBool, deadline: Instant) -> CoreResult<()> {
    if cancelled.load(Ordering::Relaxed) { return Err(cancel_error()); }
    if Instant::now() >= deadline { return Err(probe_error("FFmpeg CFR validation timed out; source recording was retained")); }
    Ok(())
}

struct Grid {
    expected: u64,
    rows: u64,
    headers: FramehashHeaders,
    header_count: usize,
    line: [u8; LINE_LIMIT],
    line_len: usize,
    first_duration: u64,
    endpoint: u64,
}
impl Grid {
    fn new(expected: u64) -> CoreResult<Self> {
        if !(1..=MAX_FRAMES).contains(&expected) { return Err(probe_error("Expected CFR frame count must be between 1 and 2592000 (24 hours)")); }
        Ok(Self { expected, rows: 0, headers: FramehashHeaders::default(), header_count: 0,
            line: [0; LINE_LIMIT], line_len: 0, first_duration: 0, endpoint: 0 })
    }
    fn push(&mut self, bytes: &[u8]) -> CoreResult<()> {
        for &byte in bytes {
            if byte == b'\n' {
                self.complete_line()?;
                self.line_len = 0;
            } else {
                if self.line_len == LINE_LIMIT { return Err(probe_error("Framehash exceeds the 1024-byte line limit")); }
                self.line[self.line_len] = byte;
                self.line_len += 1;
            }
        }
        Ok(())
    }
    fn complete_line(&mut self) -> CoreResult<()> {
        let raw = std::str::from_utf8(&self.line[..self.line_len]).map_err(|_| probe_error("Invalid framehash text"))?;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() || !line.bytes().all(|b| (b' '..=b'~').contains(&b)) { return Err(probe_error("Invalid framehash characters")); }
        if line.starts_with('#') {
            if self.rows != 0 { return Err(probe_error("Framehash header after packet data")); }
            self.header_count += 1;
            if self.header_count > 64 { return Err(probe_error("Framehash exceeds the 64-header limit")); }
            return self.headers.parse(line);
        }
        self.headers.require_complete()?;
        if self.rows >= self.expected || self.rows >= MAX_FRAMES { return Err(probe_error("Framehash exceeds expected CFR packet count")); }
        let packet = parse_packet(line)?;
        let (numerator, denominator) = self.headers.timebase.ok_or_else(|| probe_error("Missing timebase"))?;
        // i64 packet ticks and i32 rational components fit these products in u128.
        let scale = u128::from(numerator) * 30;
        if packet.dts != packet.pts || packet.pts as u128 * scale != u128::from(self.rows) * u128::from(denominator)
            || packet.duration as u128 * scale != u128::from(denominator) {
            return Err(probe_error("Video packet is not on the zero-start B0 30 fps CFR grid"));
        }
        if self.rows == 0 { self.first_duration = packet.duration as u64; }
        self.endpoint = packet.end as u64;
        self.rows += 1;
        Ok(())
    }
    fn finish(self) -> CoreResult<SavedCfrGrid> {
        if self.line_len != 0 { return Err(probe_error("Missing or truncated framehash newline")); }
        if self.rows != self.expected { return Err(probe_error("Framehash ended before the expected CFR packet count")); }
        let (numerator, denominator) = self.headers.timebase.ok_or_else(|| probe_error("Missing timebase"))?;
        let sar = self.headers.sar.ok_or_else(|| probe_error("Missing sample aspect ratio"))?;
        if sar != (0, 1) && !same_rational(sar, (1, 1)) { return Err(probe_error("CFR video must have square or unspecified sample aspect ratio")); }
        let end = u128::from(self.endpoint) * u128::from(numerator);
        if end * 30 != u128::from(self.expected) * u128::from(denominator) { return Err(probe_error("Saved video endpoint differs from the expected CFR grid")); }
        let micros = end * 1_000_000;
        if micros > u128::from(MAX_DURATION_US) * u128::from(denominator) { return Err(probe_error("Video endpoint exceeds 24 hours")); }
        let duration_us = (micros / u128::from(denominator)) as u64;
        let (width, height) = self.headers.dimensions.ok_or_else(|| probe_error("Missing dimensions"))?;
        validate_video(width, height, duration_us).map_err(|error| probe_error(error.message))?;
        Ok(SavedCfrGrid { completed_frames: self.rows, video: ProbedVideoEvidence {
            video: ProbedVideo { width, height, duration_us },
            endpoint: PacketTime { ticks: self.endpoint, numerator, denominator },
            first_packet_duration: PacketTime { ticks: self.first_duration, numerator, denominator },
        } })
    }
}

// No reader workers or blocking wait: the guard owns only the exact child it spawned.
struct BoundedChild { child: Child, reaped: bool, cleanup_attempted: bool }
impl BoundedChild {
    fn cleanup(&mut self) -> CoreResult<()> {
        self.cleanup_attempted = true;
        if self.reaped { return Ok(()); }
        let deadline = Instant::now() + Duration::from_secs(2);
        let kill_error = self.child.kill().err();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => { self.reaped = true; return Ok(()); }
                Ok(None) => (),
                Err(error) => return Err(probe_error(format!("CFR child cleanup unconfirmed: {error}; kill: {kill_error:?}"))),
            }
            if Instant::now() >= deadline { return Err(probe_error(format!("CFR child cleanup unconfirmed after two seconds; kill: {kill_error:?}"))); }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for BoundedChild {
    fn drop(&mut self) {
        if !self.cleanup_attempted {
            if let Err(error) = self.cleanup() { eprintln!("{}", error.message); }
        }
    }
}

#[cfg(any(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")), windows))]
fn run(mut command: Command, mut grid: Grid, cancelled: &AtomicBool, deadline: Instant) -> CoreResult<SavedCfrGrid> {
    check_running(cancelled, deadline)?;
    let child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|error| probe_error(format!("Could not start FFmpeg CFR validation: {error}")))?;
    let mut owner = BoundedChild { child, reaped: false, cleanup_attempted: false };
    let result = (|| {
        let mut stdout = owner.child.stdout.take().ok_or_else(|| probe_error("CFR stdout unavailable"))?;
        let mut stderr = owner.child.stderr.take().ok_or_else(|| probe_error("CFR stderr unavailable"))?;
        pipe::prepare(&stdout).and_then(|_| pipe::prepare(&stderr)).map_err(|error| probe_error(format!("Could not prepare CFR pipes: {error}")))?;
        let (mut stdout_eof, mut stderr_eof) = (false, false);
        let mut status = None;
        let mut diagnostics = Vec::with_capacity(DIAGNOSTIC_LIMIT);
        let mut buffer = [0u8; 8192];
        loop {
            check_running(cancelled, deadline)?;
            let mut progressed = false;
            // One bounded read per pipe gives cancellation and the other pipe a turn even during flooding.
            if !stdout_eof {
                match pipe::read(&mut stdout, &mut buffer).map_err(|error| probe_error(format!("Could not read CFR stdout: {error}")))? {
                    Some(0) => stdout_eof = true,
                    Some(n) => { grid.push(&buffer[..n])?; progressed = true; }
                    None => (),
                }
            }
            check_running(cancelled, deadline)?;
            if !stderr_eof {
                match pipe::read(&mut stderr, &mut buffer).map_err(|error| probe_error(format!("Could not read CFR stderr: {error}")))? {
                    Some(0) => stderr_eof = true,
                    Some(n) => { let keep = n.min(DIAGNOSTIC_LIMIT - diagnostics.len()); diagnostics.extend_from_slice(&buffer[..keep]); progressed = true; }
                    None => (),
                }
            }
            if status.is_none() {
                status = owner.child.try_wait().map_err(|error| probe_error(format!("Could not poll CFR child: {error}")))?;
                owner.reaped = status.is_some();
            }
            if let Some(status) = status {
                if !status.success() { return Err(probe_error(format!("FFmpeg CFR validation failed ({status}): {}", String::from_utf8_lossy(&diagnostics).trim()))); }
                if stdout_eof && stderr_eof { check_running(cancelled, deadline)?; return grid.finish(); }
            }
            if !progressed { std::thread::sleep(Duration::from_millis(5)); }
        }
    })();
    match owner.cleanup() {
        Ok(()) => result,
        Err(cleanup) => Err(match result {
            Err(mut error) => { error.message.push_str(&format!("; {}", cleanup.message)); error }
            Ok(_) => cleanup,
        }),
    }
}

#[cfg(not(any(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")), windows)))]
fn run(_: Command, _: Grid, _: &AtomicBool, _: Instant) -> CoreResult<SavedCfrGrid> {
    Err(probe_error("Streaming CFR validation requires Linux x86/x64/ARM64 or Windows nonblocking pipe support"))
}

#[cfg(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
mod pipe {
    use super::*;
    use std::os::fd::AsRawFd;
    unsafe extern "C" { fn fcntl(fd: std::ffi::c_int, cmd: std::ffi::c_int, ...) -> std::ffi::c_int; }
    pub(super) fn prepare(pipe: &impl AsRawFd) -> io::Result<()> {
        // Linux fcntl ABI: valid borrowed std pipe fd, scalar arguments, no pointer or ownership transfer.
        let flags = unsafe { fcntl(pipe.as_raw_fd(), 3) }; // F_GETFL
        if flags < 0 { return Err(io::Error::last_os_error()); }
        // We are the sole reader; changing this read end does not affect the child's write end.
        if unsafe { fcntl(pipe.as_raw_fd(), 4, flags | 0x800) } < 0 { return Err(io::Error::last_os_error()); } // F_SETFL, O_NONBLOCK
        Ok(())
    }
    pub(super) fn read(pipe: &mut (impl Read + AsRawFd), buffer: &mut [u8]) -> io::Result<Option<usize>> {
        match pipe.read(buffer) {
            Ok(n) => Ok(Some(n)),
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(windows)]
mod pipe {
    use super::*;
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn PeekNamedPipe(handle: *mut c_void, buffer: *mut c_void, size: u32, read: *mut u32, available: *mut u32, left: *mut u32) -> i32;
    }
    pub(super) fn prepare(_: &impl AsRawHandle) -> io::Result<()> { Ok(()) }
    pub(super) fn read(pipe: &mut (impl Read + AsRawHandle), buffer: &mut [u8]) -> io::Result<Option<usize>> {
        let mut available = 0;
        // Valid std-owned anonymous pipe handle and live DWORD output; optional pointers are null.
        // This thread is the sole consumer, with no pending I/O on this handle, so Peek cannot queue
        // behind a blocking read. Read only the reported bytes; no other reader can steal them.
        let ok = unsafe { PeekNamedPipe(pipe.as_raw_handle(), std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut available, std::ptr::null_mut()) };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(109) { return Ok(Some(0)); } // ERROR_BROKEN_PIPE: all writers closed.
            return Err(error);
        }
        if available == 0 { return Ok(None); }
        let count = buffer.len().min(available as usize);
        pipe.read(&mut buffer[..count]).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &[u8] = b"#format: frame checksums\n#version: 2\n#hash: adler32\n#tb 0: 1/30\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 100x50\n#sar 0: 1/1\n#stream#, dts, pts, duration, size, hash\n";

    #[test]
    fn arbitrary_chunk_boundaries_preserve_complete_lines_and_exact_endpoint() {
        for chunk in 1..=97 {
            let mut grid = Grid::new(1).unwrap();
            for bytes in HEADER.chunks(chunk) { grid.push(bytes).unwrap(); }
            for bytes in b"0, 0, 0, 1, 123, abcdef01\n".chunks(chunk) { grid.push(bytes).unwrap(); }
            let result = grid.finish().unwrap();
            assert_eq!(result.video.video.duration_us, 33_333);
            assert_eq!(result.video.endpoint, PacketTime {ticks: 1, numerator: 1, denominator: 30});
        }
    }

    #[test]
    fn exact_line_limit_accepts_complete_header_but_rejects_next_byte_without_newline() {
        let mut grid = Grid::new(1).unwrap();
        let line = format!("#software: {}", "x".repeat(1013));
        assert_eq!(line.len(), 1024);
        grid.push(line.as_bytes()).unwrap();
        assert!(grid.push(b"x").is_err());
        let mut grid = Grid::new(1).unwrap();
        grid.push(line.as_bytes()).unwrap();grid.push(b"\n").unwrap();
        grid.push(HEADER).unwrap();grid.push(b"0, 0, 0, 1, 123, abcdef01\n").unwrap();
        assert!(grid.finish().is_ok());
    }

    #[test]
    fn wide_rationals_and_overflow_are_checked_before_grid_acceptance() {
        let header = std::str::from_utf8(HEADER).unwrap().replace("1/30", "1/2147483640");
        let mut grid = Grid::new(1).unwrap();grid.push(header.as_bytes()).unwrap();
        grid.push(b"0, 0, 0, 71582788, 123, abcdef01\n").unwrap();
        assert_eq!(grid.finish().unwrap().video.video.duration_us, 33_333);
        for row in [
            "0, 0, 0, 71582789, 123, abcdef01\n",
            "0, 0, 0, 9223372036854775807, 123, abcdef01\n",
            "0, 0, 0, 0, 123, abcdef01\n",
            "0, 9223372036854775806, 9223372036854775806, 2, 123, abcdef01\n",
        ] {
            let mut grid = Grid::new(1).unwrap();grid.push(header.as_bytes()).unwrap();assert!(grid.push(row.as_bytes()).is_err());
        }
    }

    #[test]
    fn twenty_four_hour_grid_streams_to_exact_inclusive_ceiling() {
        let mut grid = Grid::new(2_592_000).unwrap();grid.push(HEADER).unwrap();
        // One row at a time: neither input fixture nor validator retains packet history.
        for n in 0..2_592_000 { grid.push(format!("0, {n}, {n}, 1, 123, abcdef01\n").as_bytes()).unwrap(); }
        let result = grid.finish().unwrap();
        assert_eq!(result.completed_frames, 2_592_000);
        assert_eq!(result.video.video.duration_us, 86_400_000_000);
        assert_eq!(result.video.endpoint, PacketTime {ticks: 2_592_000, numerator: 1, denominator: 30});
    }
}
