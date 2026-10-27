use std::{path::Path, process::{Child, Command, Stdio}, time::{Duration, Instant}};
use crate::{CoreResult, CoreError, ErrorCode, project::{validate_video, MAX_DURATION_US}};

mod cfr_grid;
pub use cfr_grid::{probe_cfr_grid, SavedCfrGrid};

#[derive(Debug, Clone, Copy)]
pub struct ProbedVideo { pub width: u32, pub height: u32, pub duration_us: u64 }

/// Exact packet evidence from the same bounded first/tail probe used by legacy imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketTime { pub ticks: u64, pub numerator: u32, pub denominator: u32 }
#[derive(Debug, Clone, Copy)]
pub struct ProbedVideoEvidence {
    pub video: ProbedVideo,
    pub endpoint: PacketTime,
    pub first_packet_duration: PacketTime,
}
pub fn probe_video_evidence(ffmpeg: &Path, input: &Path) -> CoreResult<ProbedVideoEvidence> {
    probe_video_until(ffmpeg, input, Instant::now() + Duration::from_secs(15))
        .map_err(|error| precise_error(error.message))
}

pub fn probe_video(ffmpeg: &Path, input: &Path) -> CoreResult<ProbedVideo> {
    probe_video_evidence(ffmpeg, input).map(|evidence| evidence.video)
}

fn probe_video_until(ffmpeg: &Path, input: &Path, deadline: Instant) -> CoreResult<ProbedVideoEvidence> {
    let mut first = None;
    for tail in [false, true] {
        let mut command = Command::new(ffmpeg);
        command.args(["-v", "error", "-copyts"]);
        if tail { command.args(["-sseof", "-5"]); }
        command.args(input_args(input, true));
        if !tail { command.args(["-frames:v", "1"]); }
        command.args(["-map", "0:v:0", "-an", "-c:v", "copy", "-f", "framehash", "-hash", "adler32", "pipe:1"]);
        let (bytes, _) = run_probe_until(command, true, deadline)?;
        let packets = parse_framehash(&bytes)?;
        if let Some(first) = first {
            return precise_video(first, packets);
        }
        if packets.rows != 1 || packets.first_pts != 0 {
            return Err(probe_error("Expected exactly one first video packet with presentation timestamp zero"));
        }
        first = Some(packets);
    }
    Err(probe_error("Missing video tail evidence"))
}

fn precise_error(message: impl std::fmt::Display) -> CoreError {
    probe_error(format!("Unsupported precise video evidence: {message}. Source recording was retained. Use a finalized zero-start H.264 recorder MP4 within the probe limits."))
}

/// Bounded metadata/capability subprocess; stdout is captured only when requested.
/// Both readers and the owned process must complete within the same 15-second deadline.
pub fn run_probe(command: Command, capture_stdout: bool) -> CoreResult<(Vec<u8>, Vec<u8>)> {
    run_probe_until(command, capture_stdout, Instant::now() + Duration::from_secs(15))
}

fn run_probe_until(mut command: Command, capture_stdout: bool, deadline: Instant) -> CoreResult<(Vec<u8>, Vec<u8>)> {
    if Instant::now() >= deadline { return Err(probe_error("FFmpeg metadata probe timed out before starting")); }
    let mut child = OwnedChild(command.stdin(Stdio::null())
        .stdout(if capture_stdout { Stdio::piped() } else { Stdio::null() }).stderr(Stdio::piped()).spawn()
        .map_err(|error| probe_error(format!("Could not start FFmpeg metadata probe: {error}")))?);
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut pipes: Vec<(usize, Box<dyn std::io::Read + Send>)> = vec![(1, Box::new(child.0.stderr.take()
        .ok_or_else(|| probe_error("FFmpeg metadata pipe unavailable"))?))];
    if capture_stdout { pipes.push((0, Box::new(child.0.stdout.take().ok_or_else(|| probe_error("FFmpeg output pipe unavailable"))?))); }
    for (index, pipe) in pipes {
        let sender = sender.clone();
        // shortcut: descendants can retain pipes; never join readers past the shared deadline.
        std::thread::Builder::new().name("metadata-reader".into()).spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            let result = pipe.take((DIAGNOSTIC_LIMIT + 1) as u64).read_to_end(&mut bytes).map(|_| bytes);
            let _ = sender.send((index, result));
        }).map_err(|error| probe_error(format!("Could not read FFmpeg metadata: {error}")))?;
    }
    drop(sender);
    let mut outputs = [if capture_stdout { None } else { Some(Vec::new()) }, None];
    let mut status = None;
    loop {
        if Instant::now() >= deadline {
            return Err(probe_error("FFmpeg metadata probe timed out after 15 seconds. Check the recording and FFmpeg, then retry."));
        }
        while outputs.iter().any(Option::is_none) {
            match receiver.try_recv() {
                Ok((index, result)) => {
                    let bytes = result.map_err(|error| probe_error(format!("Could not read FFmpeg metadata: {error}")))?;
                    if bytes.len() > DIAGNOSTIC_LIMIT { return Err(probe_error("FFmpeg metadata exceeds the 96 KiB limit. Check the recording and FFmpeg, then retry.")); }
                    outputs[index] = Some(bytes);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Err(probe_error("FFmpeg metadata reader stopped unexpectedly. Retry the import.")),
            }
        }
        if status.is_none() { status = child.0.try_wait().map_err(|error| probe_error(format!("Could not wait for FFmpeg metadata probe: {error}")))?; }
        if let Some(status) = status {
            if outputs.iter().all(Option::is_some) {
                let diagnostics = outputs[1].take().unwrap_or_default();
                if !status.success() { return Err(probe_error(format!("FFmpeg metadata probe failed ({status}): {}", String::from_utf8_lossy(&diagnostics).trim()))); }
                return Ok((outputs[0].take().unwrap_or_default(), diagnostics));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn probe_error(message: impl Into<String>) -> CoreError {
    CoreError { code: ErrorCode::EncoderFailed, message: message.into(), field: Some("video".into()) }
}

// Only the bounded recorder framehash format is supported; this is not a whole-file integrity check.
struct Framehash {
    dimensions: (u32, u32),
    timebase: (u32, u32),
    sar: (u32, u32),
    extradata: Option<(i64, u32)>,
    first_pts: i64,
    first_duration: i64,
    max_end: i64,
    rows: usize,
}

fn packet_integer(value: &str) -> CoreResult<i64> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0')) || value == "-0" {
        return Err(probe_error("Noncanonical packet integer"));
    }
    value.parse::<i64>().ok().filter(|n| *n != i64::MIN && *n != i64::MAX)
        .ok_or_else(|| probe_error("Packet integer overflow or sentinel"))
}

fn positive_integer(value: &str, maximum: i64) -> CoreResult<i64> {
    packet_integer(value).ok().filter(|n| (1..=maximum).contains(n))
        .ok_or_else(|| probe_error("Expected a bounded positive integer"))
}

fn rational(value: &str, unspecified_sar: bool) -> CoreResult<(u32, u32)> {
    if unspecified_sar && value == "0/1" { return Ok((0, 1)); }
    let (a, b) = value.split_once('/').ok_or_else(|| probe_error("Invalid packet rational"))?;
    Ok((positive_integer(a, i32::MAX as i64)? as u32, positive_integer(b, i32::MAX as i64)? as u32))
}

fn checksum(value: &str) -> CoreResult<u32> {
    if value.len() != 8 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(probe_error("Expected an eight-hex-digit adler32 checksum"));
    }
    u32::from_str_radix(value, 16).map_err(|_| probe_error("Invalid adler32 checksum"))
}

#[derive(Default)]
struct FramehashHeaders {
    seen: u16,
    dimensions: Option<(u32, u32)>,
    timebase: Option<(u32, u32)>,
    sar: Option<(u32, u32)>,
    extradata: Option<(i64, u32)>,
}
impl FramehashHeaders {
    fn parse(&mut self, line: &str) -> CoreResult<()> {
        let header = if line == "#format: frame checksums" { 0 }
            else if line == "#version: 2" { 1 }
            else if line == "#hash: adler32" { 2 }
            else if let Some(value) = line.strip_prefix("#tb 0: ") { self.timebase = Some(rational(value, false)?); 3 }
            else if line == "#media_type 0: video" { 4 }
            else if line == "#codec_id 0: h264" { 5 }
            else if let Some(value) = line.strip_prefix("#dimensions 0: ") {
                let (w, h) = value.split_once('x').ok_or_else(|| probe_error("Invalid video dimensions"))?;
                self.dimensions = Some((positive_integer(w, 16384)? as u32, positive_integer(h, 16384)? as u32)); 6
            }
            else if let Some(value) = line.strip_prefix("#sar 0: ") { self.sar = Some(rational(value, true)?); 7 }
            else if line.split(',').map(str::trim).eq(["#stream#", "dts", "pts", "duration", "size", "hash"]) { 8 }
            else if let Some(value) = line.strip_prefix("#software: ") {
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b)) {
                    return Err(probe_error("Unsupported framehash software header"));
                }
                9
            }
            else if let Some(value) = line.strip_prefix("#extradata ") {
                let fields: Vec<_> = value.split(',').map(str::trim).collect();
                if fields.len() != 3 || fields[0] != "0" { return Err(probe_error("Unsupported framehash extradata header")); }
                self.extradata = Some((positive_integer(fields[1], i32::MAX as i64)?, checksum(fields[2])?)); 10
            }
            else { return Err(probe_error("Unsupported framehash header, codec or stream (expected version 2/adler32 H.264 video)")); };
        let bit = 1 << header;
        if self.seen & bit != 0 { return Err(probe_error("Duplicate framehash header")); }
        self.seen |= bit;
        Ok(())
    }
    fn require_complete(&self) -> CoreResult<()> {
        if self.seen & 0x1ff != 0x1ff { return Err(probe_error("Missing required framehash headers")); }
        Ok(())
    }
}

struct FramehashPacket { dts: i64, pts: i64, duration: i64, end: i64 }
fn parse_packet(line: &str) -> CoreResult<FramehashPacket> {
    let mut fields = line.split(',').map(str::trim);
    let values: [Option<&str>; 6] = std::array::from_fn(|_| fields.next());
    let [Some(stream), Some(dts), Some(pts), Some(duration), Some(size), Some(hash)] = values else {
        return Err(probe_error("Unsupported video packet fields or stream"));
    };
    if fields.next().is_some() || stream != "0" { return Err(probe_error("Unsupported video packet fields or stream")); }
    let dts = packet_integer(dts)?;
    let pts = packet_integer(pts)?;
    if pts < 0 { return Err(probe_error("Negative video presentation timestamp")); }
    let duration = positive_integer(duration, i64::MAX - 1)?;
    positive_integer(size, i64::MAX - 1)?;
    checksum(hash)?;
    let end = pts.checked_add(duration).ok_or_else(|| probe_error("Video endpoint overflow"))?;
    Ok(FramehashPacket { dts, pts, duration, end })
}

fn parse_framehash(bytes: &[u8]) -> CoreResult<Framehash> {
    if bytes.len() > DIAGNOSTIC_LIMIT { return Err(probe_error("Framehash exceeds the 96 KiB limit")); }
    if bytes.is_empty() || !bytes.ends_with(b"\n") { return Err(probe_error("Missing or truncated framehash output")); }
    let text = std::str::from_utf8(bytes).map_err(|_| probe_error("Invalid framehash text"))?;
    if text.lines().filter(|line| line.starts_with('#')).count() > 64 { return Err(probe_error("Framehash exceeds the 64-header limit")); }
    let mut headers = FramehashHeaders::default();
    let (mut rows, mut first_pts, mut first_duration, mut max_end) = (0, 0, 0, 0);
    for raw in text.split_terminator('\n') {
        if raw.len() > 1024 { return Err(probe_error("Framehash exceeds the 1024-byte line limit")); }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() || !line.bytes().all(|b| (b' '..=b'~').contains(&b)) { return Err(probe_error("Invalid framehash characters")); }
        if line.starts_with('#') {
            if rows != 0 { return Err(probe_error("Framehash header after packet data")); }
            headers.parse(line)?;
        } else {
            headers.require_complete()?;
            rows += 1;
            if rows > 4096 { return Err(probe_error("Framehash exceeds the 4096-packet limit")); }
            let FramehashPacket { pts, duration, end, .. } = parse_packet(line)?;
            if rows == 1 { first_pts = pts; first_duration = duration; }
            max_end = max_end.max(end);
        }
    }
    if rows == 0 { return Err(probe_error("Missing video packets")); }
    Ok(Framehash {
        dimensions: headers.dimensions.ok_or_else(|| probe_error("Missing dimensions"))?,
        timebase: headers.timebase.ok_or_else(|| probe_error("Missing timebase"))?,
        sar: headers.sar.ok_or_else(|| probe_error("Missing sample aspect ratio"))?,
        extradata: headers.extradata, first_pts, first_duration, max_end, rows,
    })
}

fn same_rational(a: (u32, u32), b: (u32, u32)) -> bool {
    // Each component is bounded to signed 32-bit by the parser.
    u64::from(a.0) * u64::from(b.1) == u64::from(b.0) * u64::from(a.1)
}

fn precise_video(first: Framehash, tail: Framehash) -> CoreResult<ProbedVideoEvidence> {
    if first.dimensions != tail.dimensions || !same_rational(first.timebase, tail.timebase)
        || !same_rational(first.sar, tail.sar) || first.extradata != tail.extradata {
        return Err(probe_error("First and tail video headers do not agree"));
    }
    let endpoint = |packets: &Framehash| u128::try_from(packets.max_end).ok()
        .and_then(|end| end.checked_mul(u128::from(packets.timebase.0)))
        .ok_or_else(|| probe_error("Video endpoint overflow"));
    let first_end = endpoint(&first)?;
    let tail_end = endpoint(&tail)?;
    if tail_end.checked_mul(u128::from(first.timebase.1)).ok_or_else(|| probe_error("Video endpoint overflow"))?
        < first_end.checked_mul(u128::from(tail.timebase.1)).ok_or_else(|| probe_error("Video endpoint overflow"))? {
        return Err(probe_error("Video tail ends before the first packet"));
    }
    let micros = tail_end.checked_mul(1_000_000).ok_or_else(|| probe_error("Video endpoint overflow"))?;
    let denominator = u128::from(tail.timebase.1);
    if micros > u128::from(MAX_DURATION_US) * denominator { return Err(probe_error("Video endpoint exceeds 24 hours")); }
    let duration_us = u64::try_from(micros / denominator).map_err(|_| probe_error("Video duration overflow"))?;
    let (width, height) = tail.dimensions;
    validate_video(width, height, duration_us).map_err(|error| probe_error(error.message))?;
    Ok(ProbedVideoEvidence {
        video: ProbedVideo { width, height, duration_us },
        endpoint: PacketTime { ticks: tail.max_end as u64, numerator: tail.timebase.0, denominator: tail.timebase.1 },
        first_packet_duration: PacketTime { ticks: first.first_duration as u64, numerator: first.timebase.0, denominator: first.timebase.1 },
    })
}

pub const FILTER_SCRIPT: &str = "render.filter_complex.txt";
pub const RENDER_OUTPUT: &str = "output.partial.mp4";

/// Return argv only: the caller writes FILTER_SCRIPT and launches the chosen executable directly.
pub fn render_args(snapshot: &crate::store::RenderSnapshot, plan: &crate::render::RenderPlan,
    work_dir: &Path, use_audio: bool) -> CoreResult<Vec<std::ffi::OsString>> {
    crate::render::validate_plan(snapshot, plan)?;
    let video = crate::paths::asset_path(&snapshot.project_dir, &snapshot.document.video.path)?;
    let audio = crate::audio::render_audio(snapshot, use_audio)?;
    let mut args: Vec<std::ffi::OsString> = ["-hide_banner", "-nostats", "-progress", "pipe:1", "-y"].map(Into::into).into();
    args.extend(input_args(&video, true));
    if let Some(input) = &audio { args.extend(input_args(&input.path, false)); }
    args.push("-filter_complex_script".into()); args.push(work_dir.join(FILTER_SCRIPT).into_os_string());
    args.extend(["-map", "[v]"].map(Into::into));
    if audio.is_some() { args.extend(["-map", "[a]", "-c:a", "aac", "-b:a", "128k"].map(Into::into)); }
    else { args.push("-an".into()); }
    args.extend(["-r".into(), plan.preset.fps().to_string().into(), "-c:v".into(), "libx264".into(),
        "-preset".into(), "veryfast".into(), "-crf".into(), plan.preset.crf().to_string().into(),
        "-pix_fmt".into(), "yuv420p".into(), "-movflags".into(), "+faststart".into()]);
    args.push(work_dir.join(RENDER_OUTPUT).into_os_string());
    Ok(args)
}

fn input_args(input: &Path, video: bool) -> Vec<std::ffi::OsString> {
    // Recorder containers only: auto-probed playlists and external MOV tracks can escape asset containment.
    let format = if video { "mov" } else { "wav" };
    let mut args: Vec<std::ffi::OsString> = ["-protocol_whitelist", "file,pipe", "-f", format].map(Into::into).into();
    if video { args.extend(["-enable_drefs", "0", "-use_absolute_path", "0"].map(Into::into)); }
    args.push("-i".into());
    args.push(input.as_os_str().into());
    args
}

pub(crate) const DIAGNOSTIC_LIMIT: usize = 96 * 1024;

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

/// Own and reap exactly the child we launched; persisted process IDs are never used.
pub(crate) fn run_encoder(mut command: Command, cancelled: impl Fn() -> CoreResult<bool>,
    mut progress: impl FnMut(u64) -> CoreResult<()>) -> CoreResult<()> {
    use std::{io::Read, sync::{Arc, atomic::{AtomicU64, Ordering}}};
    if cancelled()? { return Err(cancel_error()); }
    let mut child = OwnedChild(command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| runner_error(format!("Could not start encoder: {e}")))?);
    let stdout = child.0.stdout.take().ok_or_else(|| runner_error("Encoder progress pipe unavailable"))?;
    let mut stderr = child.0.stderr.take().ok_or_else(|| runner_error("Encoder diagnostic pipe unavailable"))?;
    let time = Arc::new(AtomicU64::new(0));
    let reader_time = time.clone();
    let output_reader = std::thread::Builder::new().name("export-progress".into()).spawn(move || {
        read_progress(stdout, &reader_time)
    }).map_err(|e| runner_error(format!("Could not drain encoder progress: {e}")))?;
    let error_reader = match std::thread::Builder::new().name("export-diagnostics".into()).spawn(move || {
        let mut tail = std::collections::VecDeque::with_capacity(DIAGNOSTIC_LIMIT);
        let mut buf = [0; 4096];
        loop {
            let count = stderr.read(&mut buf)?;
            if count == 0 { break; }
            let excess = (tail.len() + count).saturating_sub(DIAGNOSTIC_LIMIT);
            tail.drain(..excess);
            tail.extend(&buf[..count]);
        }
        Ok::<_, std::io::Error>(tail.into_iter().collect::<Vec<u8>>())
    }) {
        Ok(reader) => reader,
        Err(error) => {
            drop(child);
            let _ = output_reader.join();
            return Err(runner_error(format!("Could not drain encoder diagnostics: {error}")));
        }
    };
    let outcome = (|| loop {
        if cancelled()? { return Err(cancel_error()); }
        progress(time.load(Ordering::Relaxed))?;
        if let Some(status) = child.0.try_wait().map_err(|e| runner_error(format!("Could not wait for encoder: {e}")))? {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(25));
    })();
    // Kill before joining pipe readers on any error, including a progress-persistence failure.
    drop(child);
    let progress_read = output_reader.join().map_err(|_| runner_error("Encoder progress reader stopped unexpectedly"))?;
    let diagnostics = error_reader.join().map_err(|_| runner_error("Encoder diagnostic reader stopped unexpectedly"))?
        .map_err(|e| runner_error(format!("Could not read encoder diagnostics: {e}")))?;
    let status = outcome?;
    progress_read.map_err(|e| runner_error(format!("Could not read encoder progress: {e}")))?;
    if !status.success() {
        let mut text = String::from_utf8_lossy(&diagnostics).into_owned();
        let mut end = text.len().min(DIAGNOSTIC_LIMIT);
        while !text.is_char_boundary(end) { end -= 1; }
        text.truncate(end);
        return Err(runner_error(format!("Encoder exited with {status}: {text}")));
    }
    Ok(())
}

fn read_progress(mut input: impl std::io::Read, time: &std::sync::atomic::AtomicU64) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    let mut buf = [0; 4096];
    let mut line = Vec::with_capacity(128);
    let mut oversized = false;
    loop {
        let count = input.read(&mut buf)?;
        if count == 0 { return Ok(()); }
        for byte in &buf[..count] {
            if *byte == b'\n' {
                if !oversized {
                    if let Ok(text) = std::str::from_utf8(&line) {
                        if let Some(value) = text.trim_end_matches('\r').strip_prefix("out_time_us=") {
                            if let Ok(value) = value.parse::<u64>() { time.fetch_max(value, Ordering::Relaxed); }
                        }
                    }
                }
                line.clear(); oversized = false;
            } else if line.len() < 128 && !oversized { line.push(*byte); }
            else { oversized = true; }
        }
    }
}
fn runner_error(message: impl Into<String>) -> CoreError {
    CoreError { code: ErrorCode::EncoderFailed, message: message.into(), field: None }
}
pub(crate) fn cancel_error() -> CoreError {
    CoreError { code: ErrorCode::Cancelled, message: "Export cancelled".into(), field: None }
}

/// Seek participating branches directly; preceding keyframes may still need decoding for long GOPs.
pub fn preview_args(snapshot: &crate::store::RenderSnapshot, plan: &crate::render::RenderPlan,
    selection: &crate::preview::PreviewSelection, work_dir: &Path, use_audio: bool, still: bool) -> CoreResult<Vec<std::ffi::OsString>> {
    crate::render::validate_plan(snapshot, plan)?;
    crate::project::require(!still || selection.frame_count == 1, "preview", "A still must select exactly one frame")?;
    let selected = crate::preview::inputs(snapshot, plan, selection, use_audio && !still)?;
    let video = crate::paths::asset_path(&snapshot.project_dir, &snapshot.document.video.path)?;
    let mut args: Vec<std::ffi::OsString> = ["-hide_banner", "-nostats", "-progress", "pipe:1", "-y", "-copyts", "-start_at_zero"].map(Into::into).into();
    for span in &selected.video {
        let seek_us = (span.source_first * 1_000_000 / selection.fps as u64).saturating_sub(100_000);
        args.extend(["-ss".into(), format!("{}.{:06}",seek_us/1_000_000,seek_us%1_000_000).into(), "-noaccurate_seek".into(), "-threads".into(), "1".into()]);
        args.extend(input_args(&video, true));
    }
    if let Some(wav) = &selected.audio {
        for span in &selected.audio_spans {
            if span.input.is_none() { continue; }
            let seek_us = (span.low * 1_000_000 / wav.sample_rate as u64).saturating_sub(100_000);
            args.extend(["-ss".into(), format!("{}.{:06}",seek_us/1_000_000,seek_us%1_000_000).into(), "-threads".into(), "1".into()]);
            args.extend(input_args(&wav.path, false));
        }
    }
    args.push("-filter_complex_script".into()); args.push(work_dir.join(FILTER_SCRIPT).into_os_string());
    args.extend(["-map", "[v]", "-frames:v"].map(Into::into)); args.push(selection.frame_count.to_string().into());
    if still {
        args.extend(["-an", "-c:v", "png", "-threads", "1", "-update", "1"].map(Into::into));
        args.push(work_dir.join("output.partial.png").into_os_string());
    } else {
        if selected.audio.is_some() { args.extend(["-map", "[a]", "-c:a", "aac", "-b:a", "128k"].map(Into::into)); }
        else { args.push("-an".into()); }
        args.extend(["-r", "30", "-c:v", "libx264", "-preset", "veryfast", "-crf", "25", "-pix_fmt", "yuv420p", "-movflags", "+faststart"].map(Into::into));
        args.push(work_dir.join(RENDER_OUTPUT).into_os_string());
    }
    Ok(args)
}

pub(crate) fn cursor_window_args(snapshot:&crate::store::RenderSnapshot,plan:&crate::render::RenderPlan,
 window:&crate::render::cursor_window::CursorWindow,dir:&Path,still:bool)->CoreResult<Vec<std::ffi::OsString>> {
 let fps=plan.preset.fps();let video=crate::paths::asset_path(&snapshot.project_dir,&snapshot.document.video.path)?;
 let seek=(window.source_first*1_000_000/fps as u64).saturating_sub(100_000);
 let mut args:Vec<std::ffi::OsString>=["-hide_banner","-nostats","-progress","pipe:1","-y","-copyts","-start_at_zero","-ss"].map(Into::into).into();
 args.extend([format!("{}.{:06}",seek/1_000_000,seek%1_000_000).into(),"-noaccurate_seek".into(),"-threads".into(),"1".into()]);
 debug_assert_eq!(args.iter().filter(|a|*a=="-i").count(),crate::render::cursor_window::VIDEO_INPUT);
 args.extend(input_args(&video,true));
 debug_assert_eq!(args.iter().filter(|a|*a=="-i").count(),crate::render::cursor_window::ATLAS_INPUT);
 if window.atlas.is_some() {
 args.extend(["-f","image2","-framerate"].map(Into::into));args.push(fps.to_string().into());
 args.extend(["-c:v","pam","-i"].map(Into::into));args.push(dir.join("cursor-atlas.pam").into_os_string());
 }
 args.extend(["-filter_complex_threads","1","-filter_complex_script"].map(Into::into));args.push(dir.join(FILTER_SCRIPT).into_os_string());
 args.extend(["-map","[v]","-an","-frames:v"].map(Into::into));args.push(window.count.to_string().into());
 if still {args.extend(["-c:v","png","-threads","1","-update","1"].map(Into::into));args.push(dir.join("output.partial.png").into_os_string());}
 else {
  args.extend(["-c:v","libx264","-preset","veryfast","-crf"].map(Into::into));args.push(plan.preset.crf().to_string().into());
  args.extend(["-pix_fmt","yuv420p","-bf","0","-flags","+cgop","-video_track_timescale","90000","-r"].map(Into::into));args.push(fps.to_string().into());
  args.push(dir.join("cursor-segment.mp4").into_os_string());
 }
 Ok(args)
}
pub(crate) fn cursor_concat_args(dir:&Path,fps:u32)->Vec<std::ffi::OsString>{
 let mut args:Vec<std::ffi::OsString>=["-hide_banner","-nostats","-progress","pipe:1","-y","-f","concat","-safe","1","-i"].map(Into::into).into();
 args.push(dir.join("cursor-concat.txt").into_os_string());
 args.extend(["-map","0:v","-an","-c:v","copy","-bsf:v"].map(Into::into));
 args.push(format!("setts=pts=N*{}:dts=N*{}:duration={}:time_base=1/90000",90000/fps,90000/fps,90000/fps).into());
 args.extend(["-video_track_timescale","90000","-movflags","+faststart"].map(Into::into));args.push(dir.join("cursor-video.mp4").into_os_string());args
}
pub(crate) fn cursor_audio_args(snapshot:&crate::store::RenderSnapshot,plan:&crate::render::RenderPlan,
 selection:Option<&crate::preview::PreviewSelection>,dir:&Path)->CoreResult<Vec<std::ffi::OsString>>{
 // Keep the established audio input map (including unused video inputs), then add completed video.
 let mut args=match selection {Some(s)=>preview_args(snapshot,plan,s,dir,true,false)?,None=>render_args(snapshot,plan,dir,true)?};
 let cut=args.iter().position(|a|a=="-filter_complex_script").unwrap();args.truncate(cut);
 let index=args.iter().filter(|a|*a=="-i").count();
 args.extend(input_args(&dir.join("cursor-video.mp4"),true));
 args.extend(["-filter_complex_script".into(),dir.join(FILTER_SCRIPT).into_os_string(),"-map".into(),format!("{index}:v").into(),"-map".into(),"[a]".into(),"-c:v".into(),"copy".into(),"-c:a".into(),"aac".into(),"-b:a".into(),"128k".into(),"-video_track_timescale".into(),"90000".into(),"-movflags".into(),"+faststart".into(),dir.join(RENDER_OUTPUT).into_os_string()]);Ok(args)
}

#[cfg(all(test, unix))]
mod precise_deadline_tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(pid) = fs::read_to_string(self.0.join("tool.descendant")) {
                let _ = Command::new("kill").arg(pid.trim()).status();
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn precise_pair_shares_deadline_reaps_children_and_does_not_wait_for_inherited_pipes() {
        let hash = "#format: frame checksums\n#version: 2\n#hash: adler32\n#tb 0: 1/30\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 100x50\n#sar 0: 0/1\n#stream#, dts, pts, duration, size, hash\n0,-2,0,1,1,00000000\n";
        for (index, body) in [
            "echo $$ > \"$0.pid\"\nexec sleep 5",
            "echo $$ > \"$0.pid\"\ncase \" $* \" in *' -sseof -5 '*) echo tail > \"$0.phase\"; sleep 0.25;; *) sleep 0.15;; esac\ncat \"$0.hash\"",
            "echo $$ > \"$0.pid\"\ncase \" $* \" in *' -sseof -5 '*) echo tail > \"$0.phase\"; sleep 5 & echo $! > \"$0.descendant\"; exit 0;; *) cat \"$0.hash\";; esac",
        ].into_iter().enumerate() {
            let fixture = Fixture(std::env::temp_dir().join(format!("focusflow-precise-deadline-{}-{index}", std::process::id())));
            fs::create_dir(&fixture.0).unwrap();
            let tool = fixture.0.join("tool");
            fs::write(&tool, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(tool.with_extension("hash"), hash).unwrap();
            // An expired budget must not spawn even the first phase.
            assert!(probe_video_until(&tool, Path::new("screen.mp4"), Instant::now()).unwrap_err().message.contains("timed out"));
            assert!(!tool.with_extension("pid").exists());
            let started = Instant::now();
            let result = probe_video_until(&tool, Path::new("screen.mp4"), started + Duration::from_millis(300));
            assert!(result.unwrap_err().message.contains("timed out"));
            assert!(started.elapsed() < Duration::from_millis(650));
            if index > 0 { assert_eq!(fs::read_to_string(tool.with_extension("phase")).unwrap().trim(), "tail"); }
            let pid = fs::read_to_string(tool.with_extension("pid")).unwrap();
            assert!(!Command::new("kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status().unwrap().success());
        }
    }
}

#[cfg(test)]
mod precise_packet_tests {
    use super::*;

    #[test]
    fn native_b_frames_and_submicrosecond_endpoints_use_checked_presentation_time() {
        let headers = "#format: frame checksums\n#version: 2\n#hash: adler32\n#tb 0: 1/15360\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 1280x800\n#sar 0: 0/1\n#stream#, dts, pts, duration, size, hash\n";
        let first = headers.to_owned()+"0,-1024,0,512,19741,9616ce88\n";
        let tail = headers.to_owned()+"0,45056,46592,512,713,9616ce88\n0,45568,46080,512,538,9616ce88\n";
        let evidence = precise_video(parse_framehash(first.as_bytes()).unwrap(), parse_framehash(tail.as_bytes()).unwrap()).unwrap();
        assert_eq!((evidence.video.width, evidence.video.height, evidence.video.duration_us), (1280, 800, 3_066_666));
        let tiny = (headers.to_owned()+"0,0,0,1,1,00000000\n").replace("1/15360", "1/2000000");
        assert!(precise_video(parse_framehash(tiny.as_bytes()).unwrap(), parse_framehash(tiny.as_bytes()).unwrap()).is_err());
        let one_us = tiny.replace("1/2000000", "1/1000000");
        assert_eq!(precise_video(parse_framehash(one_us.as_bytes()).unwrap(), parse_framehash(one_us.as_bytes()).unwrap()).unwrap().video.duration_us, 1);
        let before_first = first.replace(",512,", ",511,");
        assert!(precise_video(parse_framehash(first.as_bytes()).unwrap(), parse_framehash(before_first.as_bytes()).unwrap()).is_err());
        let overflow = headers.to_owned()+"0,0,9223372036854775806,2,1,00000000\n";
        assert!(parse_framehash(overflow.as_bytes()).is_err());
    }
}
