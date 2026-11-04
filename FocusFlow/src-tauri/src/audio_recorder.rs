//! Microphone audio recording backend for FocusFlow.
//!
//! Captures audio from the platform microphone backend using [`cpal`] and writes
//! 16-bit PCM samples to `mic.wav` in the active session folder using [`hound`].
//!
//! # Threading model
//!
//! ```text
//!  CPAL callback thread
//!  (realtime priority, short deadline)
//!       │
//!       │  Vec<i16>  (bounded, nonblocking channel)
//!       ▼
//!  focusflow-audio-owner thread
//!  (normal priority, blocking I/O allowed)
//!       │
//!       ▼  hound::WavWriter
//!    mic.wav.partial → mic.wav only after clean finalization
//! ```
//!
//! The callback caps each batch, converts to `i16`, and tries to enqueue it.
//! Overflow or a stream error permanently fails this track, preventing later
//! samples from concealing a gap. The owner drops the stream, drains accepted
//! batches, and finalizes away from the callback. Failed audio stays in an
//! exclusively created recovery file; only clean, usable audio is published.

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat,
};
use hound::{WavSpec, WavWriter};
use serde::Serialize;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{mpsc::{self, Sender, SyncSender}, atomic::{AtomicU8, Ordering}, Arc},
    thread::{self, JoinHandle},
};

// ── Public result type ─────────────────────────────────────────────────────

pub type AudioResult<T> = Result<T, AudioError>;

// ── AudioError ─────────────────────────────────────────────────────────────

/// A recoverable or non-recoverable error from the audio recording backend.
///
/// Error codes:
/// * `host_unavailable`        — the cpal audio host could not be queried.
/// * `no_input_device`         — no microphone was found on the system.
/// * `device_unavailable`      — the named device is not currently connected.
/// * `stream_build_failed`     — cpal could not create the input stream.
/// * `stream_play_failed`      — cpal could not start streaming.
/// * `wav_writer_failed`       — hound could not write or finalize mic.wav.
/// * `writer_thread_failed`    — the OS rejected the writer thread spawn.
/// * `writer_thread_panicked`  — the writer thread panicked before finalizing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl AudioError {
    fn new(code: impl Into<String>, message: impl Into<String>, recoverable: bool) -> Self {
        AudioError {
            code: code.into(),
            message: message.into(),
            recoverable,
        }
    }
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AudioError {}

// ── AudioDevice ────────────────────────────────────────────────────────────

/// A discoverable microphone input device.
///
/// The `id` is the platform device name string.  It is used as the selector
/// when calling [`AudioRecorder::start`].  On Windows, built-in device names
/// are stable across reboots; USB device names may change when re-plugged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    /// Opaque selector — currently the platform device name.
    pub id: String,
    /// Human-readable label shown in the UI microphone dropdown.
    pub name: String,
}

// ── Tauri command ──────────────────────────────────────────────────────────

/// Returns all available microphone input devices from the platform audio host.
///
/// Devices that fail to return a name are silently skipped so a single
/// broken device cannot prevent the list from loading.
///
/// Returns an **empty list** (not an error) when no microphone is connected.
/// Returns an error only if the audio host itself is unavailable.
#[tauri::command]
pub fn list_audio_input_devices() -> AudioResult<Vec<AudioDevice>> {
    let host = cpal::default_host();

    let devices = host.input_devices().map_err(|error| {
        AudioError::new(
            "host_unavailable",
            format!("Could not enumerate audio input devices: {error}"),
            true,
        )
    })?;

    let mut result = Vec::new();

    for device in devices {
        match device.name() {
            Ok(name) => {
                result.push(AudioDevice {
                    id: name.clone(),
                    name,
                });
            }
            Err(error) => {
                eprintln!("[FocusFlow audio] Skipping unnamed device: {error}");
            }
        }
    }

    Ok(result)
}

// Bound queued PCM to 256 KiB, plus one owner-held and one callback-held batch.
const SAMPLE_QUEUE_BATCHES: usize = 8;
const MAX_CALLBACK_SAMPLES: usize = 16_384;
const QUEUE_OVERFLOW: u8 = 1;
const BATCH_TOO_LARGE: u8 = 2;
const STREAM_FAILED: u8 = 3;
const WRITER_DISCONNECTED: u8 = 4;

#[derive(Default)]
struct CaptureStatus(AtomicU8);
impl CaptureStatus {
    fn fail(&self, reason: u8) {
        let _ = self.0.compare_exchange(0, reason, Ordering::AcqRel, Ordering::Acquire);
    }
    fn failed(&self) -> u8 { self.0.load(Ordering::Acquire) }
    fn error(&self) -> Option<AudioError> {
        let (code, message) = match self.failed() {
            0 => return None,
            QUEUE_OVERFLOW => ("mic_queue_overflow", "Microphone writing could not keep up with capture."),
            BATCH_TOO_LARGE => ("mic_batch_too_large", "The microphone delivered an unsupported sample batch size."),
            STREAM_FAILED => ("mic_stream_failed", "The microphone stream failed. Check the device connection and microphone access before recording again."),
            _ => ("mic_writer_disconnected", "The microphone writer stopped accepting samples."),
        };
        Some(AudioError::new(code, message, true))
    }
}
#[derive(Clone)]
struct CallbackQueue {
    tx: SyncSender<Vec<i16>>,
    status: Arc<CaptureStatus>,
}
impl CallbackQueue {
    fn new() -> (Self, mpsc::Receiver<Vec<i16>>) {
        let (tx, rx) = mpsc::sync_channel(SAMPLE_QUEUE_BATCHES);
        (Self { tx, status: Arc::new(CaptureStatus::default()) }, rx)
    }
    fn stream_error(&self, _error: cpal::StreamError) { self.status.fail(STREAM_FAILED); }
    fn error_handler(&self) -> impl FnMut(cpal::StreamError) + Send + 'static {
        // EOF must cover both callbacks, even if the backend drops them separately.
        let queue = self.clone();
        move |error| queue.stream_error(error)
    }
    fn push<T: Copy>(&self, data: &[T], convert: fn(T) -> i16) {
        if self.status.failed() != 0 || data.is_empty() { return; }
        if data.len() > MAX_CALLBACK_SAMPLES { self.status.fail(BATCH_TOO_LARGE); return; }
        let samples = data.iter().copied().map(convert).collect();
        if self.status.failed() != 0 { return; }
        match self.tx.try_send(samples) {
            Ok(()) => {},
            Err(mpsc::TrySendError::Full(_)) => self.status.fail(QUEUE_OVERFLOW),
            Err(mpsc::TrySendError::Disconnected(_)) => self.status.fail(WRITER_DISCONNECTED),
        }
    }
}

type WavOutput = WavWriter<std::io::BufWriter<std::fs::File>>;
struct PendingWav {
    writer: WavOutput,
    partial: PathBuf,
    output: PathBuf,
}
impl PendingWav {
    fn finish(self, capture_result: AudioResult<()>) -> AudioResult<()> {
        let finalized = self.writer.finalize().map_err(wav_error);
        // Always finalize for recovery, but never publish a discontinuous track.
        capture_result?;
        finalized?;
        if !focusflow_core::audio::is_usable_wav(&self.partial) {
            return Err(AudioError::new("mic_audio_unusable", "Microphone audio was empty or unreadable; the partial recording was retained for recovery.", true));
        }
        // hard_link publishes without replacing an existing file, including a dangling symlink.
        std::fs::hard_link(&self.partial, &self.output).map_err(|error| AudioError::new("mic_publish_failed", format!("Could not publish microphone audio: {error}; partial recording retained for recovery."), true))?;
        if let Err(error) = std::fs::remove_file(&self.partial) {
            eprintln!("[FocusFlow audio] Published microphone audio; could not remove owned partial: {error}");
        }
        Ok(())
    }
}
fn write_samples(writer: &mut WavOutput, batch: Vec<i16>) -> AudioResult<()> {
    for sample in batch { writer.write_sample(sample).map_err(wav_error)?; }
    Ok(())
}
fn finish_samples(samples: mpsc::Receiver<Vec<i16>>, mut wav: PendingWav, capture_result: AudioResult<()>, status: &CaptureStatus) -> AudioResult<()> {
    let mut write_result = Ok(());
    for batch in samples {
        if write_result.is_ok() { write_result = write_samples(&mut wav.writer, batch); }
    }
    // All callback senders are gone; include errors raised during the drain.
    let capture_result = status.error().map_or(capture_result, Err);
    wav.finish(capture_result.and(write_result))
}

// ── AudioRecorder ──────────────────────────────────────────────────────────

/// The CPAL stream is created, played, and dropped only on its owning thread.
/// Dropping this handle also stops and finalizes the recording.
pub struct AudioRecorder {
    stop: Option<Sender<()>>,
    owner: Option<JoinHandle<AudioResult<()>>>,
}

impl AudioRecorder {
    pub fn start(device_name: Option<&str>, output_path: &Path) -> AudioResult<Self> {
        let device_name = device_name.map(str::to_owned);
        let output_path = output_path.to_path_buf();
        let (stop, stop_rx) = mpsc::channel();
        let (ready, ready_rx) = mpsc::sync_channel(1);
        let owner = thread::Builder::new().name("focusflow-audio-owner".into()).spawn(move || {
            let initialized = initialize_audio(device_name.as_deref(), &output_path);
            let (stream, samples, mut wav, status) = match initialized {
                Ok(value) => value,
                Err(error) => { let _ = ready.send(Err(error.clone())); return Err(error); }
            };
            if ready.send(Ok(())).is_err() {
                drop(stream);
                return finish_samples(samples, wav, Err(AudioError::new("audio_start_cancelled", "Microphone startup was cancelled.", true)), &status);
            }
            let mut result = Ok(());
            loop {
                if let Some(error) = status.error() { result = Err(error); break; }
                match stop_rx.try_recv() {
                    Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                match samples.recv_timeout(std::time::Duration::from_millis(20)) {
                    Ok(batch) => {
                        if let Err(error) = write_samples(&mut wav.writer, batch) { result = Err(error); break; }
                    },
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        result = Err(AudioError::new("mic_stream_failed", "The microphone stream ended unexpectedly.", true));
                        break;
                    },
                }
            }
            drop(stream);
            finish_samples(samples, wav, result, &status)
        }).map_err(|error| AudioError::new("audio_thread_failed", error.to_string(), true))?;
        let startup_error = match ready_rx.recv() {
            Ok(Ok(())) => return Ok(Self { stop: Some(stop), owner: Some(owner) }),
            Ok(Err(error)) => error,
            Err(error) => AudioError::new("audio_start_failed", error.to_string(), true),
        };
        drop(stop);
        let _ = owner.join();
        Err(startup_error)
    }
    pub fn stop(mut self) -> AudioResult<()> { self.finish() }
    pub(crate) fn request_stop(&mut self) {
        if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
    }
    fn finish(&mut self) -> AudioResult<()> {
        self.request_stop();
        match self.owner.take() {
            Some(owner) => owner.join().map_err(|_| AudioError::new("audio_thread_panicked", "Audio owner stopped unexpectedly", true))?,
            None => Ok(()),
        }
    }
}
impl Drop for AudioRecorder {
    fn drop(&mut self) { if let Err(error) = self.finish() { eprintln!("[FocusFlow audio] {error}"); } }
}
fn wav_error(error: hound::Error) -> AudioError { AudioError::new("wav_writer_failed", error.to_string(), true) }
fn create_wav_writer(output_path: &Path, spec: WavSpec) -> AudioResult<PendingWav> {
    match std::fs::symlink_metadata(output_path) {
        Ok(_) => return Err(AudioError::new("mic_output_exists", "Microphone output already exists; existing recording was preserved.", true)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(AudioError::new("wav_writer_failed", error.to_string(), true)),
    }
    let partial = output_path.with_file_name("mic.wav.partial");
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&partial)
        .map_err(|error| AudioError::new("wav_writer_failed", format!("Could not create microphone recovery file: {error}"), true))?;
    let writer = WavWriter::new(std::io::BufWriter::new(file), spec).map_err(wav_error)?;
    Ok(PendingWav { writer, partial, output: output_path.to_owned() })
}
fn initialize_audio(device_name: Option<&str>, output_path: &Path) -> AudioResult<(cpal::Stream, mpsc::Receiver<Vec<i16>>, PendingWav, Arc<CaptureStatus>)> {
        let host = cpal::default_host();

        let device = match device_name {
            Some(name) => {
                // Find the device whose WASAPI name matches exactly.
                let devices = host.input_devices().map_err(|error| {
                    AudioError::new(
                        "host_unavailable",
                        format!("Could not enumerate audio devices: {error}"),
                        true,
                    )
                })?;

                devices
                    .filter_map(|d| d.name().ok().map(|n| (n, d)))
                    .find(|(n, _)| n == name)
                    .map(|(_, d)| d)
                    .ok_or_else(|| {
                        AudioError::new(
                            "device_unavailable",
                            format!(
                                "Microphone '{name}' is not available. \
                                 It may have been disconnected. \
                                 Refresh the device list and choose again."
                            ),
                            true,
                        )
                    })?
            }
            None => host.default_input_device().ok_or_else(|| {
                AudioError::new(
                    "no_input_device",
                    "No microphone input device was found. \
                     Check that a microphone is connected and that \
                     microphone access is enabled in your system settings.",
                    true,
                )
            })?,
        };

        // Query the device's preferred input configuration (sample rate,
        // channel count, sample format).  This is what WASAPI reports as the
        // device's native mix format.
        let supported_config = device.default_input_config().map_err(|error| {
            AudioError::new(
                "stream_build_failed",
                format!(
                    "Could not read microphone configuration: {error}. \
                     The device may have been disconnected."
                ),
                true,
            )
        })?;

        let channels = supported_config.channels();
        let sample_rate = supported_config.sample_rate().0;
        let sample_format = supported_config.sample_format();
        // Convert to StreamConfig (drops the supported-range metadata).
        let stream_config: cpal::StreamConfig = supported_config.into();

        // WAV output spec: always i16 PCM regardless of device format.
        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

    // File creation is part of the readiness handshake, before video may start.
    let writer = create_wav_writer(output_path, spec)?;
    let (tx, rx) = CallbackQueue::new();
    let status = Arc::clone(&tx.status);
    let stream = build_input_stream(&device, &stream_config, sample_format, tx)?;
    stream.play().map_err(|error| AudioError::new("stream_play_failed", error.to_string(), true))?;
    if let Some(error) = status.error() { return Err(error); }
    Ok((stream, rx, writer, status))
}

// ── Stream construction ────────────────────────────────────────────────────

/// Builds a typed cpal input stream for the given `sample_format`, converting
/// all samples to `i16` before forwarding them through `tx`.
///
/// Each sample format dispatches to a separate [`build_typed_input_stream`]
/// call so that `tx` is moved into exactly one closure (the one that matches
/// the actual device format).  Rust's borrow checker permits moving a value
/// into different match arms because only one arm executes.
fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: SampleFormat,
    tx: CallbackQueue,
) -> AudioResult<cpal::Stream> {
    let stream = match sample_format {
        // Float formats (most common on modern WASAPI devices)
        SampleFormat::F32 => build_typed_input_stream::<f32>(device, config, tx, f32_to_i16),
        SampleFormat::F64 => build_typed_input_stream::<f64>(device, config, tx, f64_to_i16),

        // Signed integer formats
        SampleFormat::I8 => build_typed_input_stream::<i8>(device, config, tx, i8_to_i16),
        SampleFormat::I16 => build_typed_input_stream::<i16>(device, config, tx, i16_identity),
        SampleFormat::I32 => build_typed_input_stream::<i32>(device, config, tx, i32_to_i16),
        SampleFormat::I64 => build_typed_input_stream::<i64>(device, config, tx, i64_to_i16),

        // Unsigned integer formats
        SampleFormat::U8 => build_typed_input_stream::<u8>(device, config, tx, u8_to_i16),
        SampleFormat::U16 => build_typed_input_stream::<u16>(device, config, tx, u16_to_i16),
        SampleFormat::U32 => build_typed_input_stream::<u32>(device, config, tx, u32_to_i16),
        SampleFormat::U64 => build_typed_input_stream::<u64>(device, config, tx, u64_to_i16),

        // Unknown future format (SampleFormat is #[non_exhaustive] in cpal 0.15)
        other => {
            return Err(AudioError::new(
                "stream_build_failed",
                format!(
                    "Unsupported microphone sample format: {other:?}. \
                     FocusFlow supports F32, F64, I8/16/32/64, and U8/16/32/64."
                ),
                false,
            ));
        }
    };

    stream.map_err(|error| {
        AudioError::new(
            "stream_build_failed",
            format!(
                "Could not build microphone input stream ({sample_format:?}): {error}. \
                 Check that the microphone is not in use by another application."
            ),
            true,
        )
    })
}

/// Creates a cpal input stream typed to `T`, converts each sample to `i16`
/// using `convert`, and sends batches through `tx`.
///
/// `tx` is **moved** into the callback closure.  When the stream is dropped
/// the closure is dropped and `tx` is dropped, closing the channel.
///
/// Both callbacks latch the first failure. The owner stops the stream; later
/// callback data is rejected so a device interruption cannot become a hidden gap.
fn build_typed_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: CallbackQueue,
    convert: fn(T) -> i16,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + Send + 'static,
{
    let on_error = tx.error_handler();
    device.build_input_stream(
        config,
        move |data: &[T], _info: &cpal::InputCallbackInfo| {
            tx.push(data, convert);
        },
        on_error,
        None, // no timeout
    )
}

// ── Sample conversion functions ────────────────────────────────────────────
//
// All conversions map the full range of the source type to [-32768, 32767].
// These are plain function items (not closures) so they can be passed as
// `fn(T) -> i16` function-pointer arguments without capturing any state.

/// F32 in [-1.0, 1.0] → I16
#[inline]
fn f32_to_i16(sample: f32) -> i16 {
    // clamp first to guard against WASAPI delivering samples slightly outside
    // the nominal range on some drivers.
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// F64 in [-1.0, 1.0] → I16
#[inline]
fn f64_to_i16(sample: f64) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f64) as i16
}

/// I16 → I16 (no-op identity)
#[inline]
fn i16_identity(sample: i16) -> i16 {
    sample
}

/// I8 → I16: scale by left-shifting 8 bits
#[inline]
fn i8_to_i16(sample: i8) -> i16 {
    (sample as i16) << 8
}

/// I32 → I16: take the 16 most-significant bits
#[inline]
fn i32_to_i16(sample: i32) -> i16 {
    (sample >> 16) as i16
}

/// I64 → I16: take the 16 most-significant bits
#[inline]
fn i64_to_i16(sample: i64) -> i16 {
    (sample >> 48) as i16
}

/// U8 → I16: re-centre around 0, then scale
///
/// U8 mid-point (128) maps to 0; 0 maps to i16::MIN; 255 maps to ~i16::MAX.
#[inline]
fn u8_to_i16(sample: u8) -> i16 {
    // (sample - 128) gives [-128, 127]; shift left 8 to fill 16 bits.
    ((sample as i16) - 128) << 8
}

/// U16 → I16: re-centre by subtracting 2^15
///
/// U16 mid-point (32768) maps to 0; 0 maps to i16::MIN; 65535 maps to i16::MAX.
#[inline]
fn u16_to_i16(sample: u16) -> i16 {
    // wrapping_sub avoids UB; 0u16.wrapping_sub(0x8000) == 0x8000 == -32768i16
    sample.wrapping_sub(0x8000) as i16
}

/// U32 → I16: re-centre by subtracting 2^31, then take the high 16 bits
#[inline]
fn u32_to_i16(sample: u32) -> i16 {
    (sample.wrapping_sub(0x8000_0000_u32) >> 16) as i16
}

/// U64 → I16: re-centre by subtracting 2^63, then take the high 16 bits
#[inline]
fn u64_to_i16(sample: u64) -> i16 {
    (sample.wrapping_sub(0x8000_0000_0000_0000_u64) >> 48) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_dir() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("focusflow-bounded-mic-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        path
    }
    fn test_spec() -> WavSpec {
        WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }
    }
    #[test]
    fn wav_writer_never_overwrites_existing_output() {
        let dir = test_dir();
        let path = dir.join("mic.wav");
        std::fs::write(&path, b"preexisting recording").unwrap();
        let result = create_wav_writer(&path, test_spec());
        let rejected = result.is_err();
        drop(result);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(rejected, "must reject an existing output before opening it");
        assert_eq!(bytes, b"preexisting recording");
    }
    #[test]
    fn queue_overflow_is_nonblocking_and_stops_all_later_batches() {
        let (queue, rx) = CallbackQueue::new();
        let status = Arc::clone(&queue.status);
        let (done, completed) = mpsc::channel();
        let producer = thread::spawn(move || {
            for _ in 0..SAMPLE_QUEUE_BATCHES + 1 { queue.push(&[7i16], i16_identity); }
            let _ = done.send(());
            queue
        });
        let returned = completed.recv_timeout(std::time::Duration::from_secs(1)).is_ok();
        // Release a wrongly blocking producer before asserting, so failures cannot hang the suite.
        if !returned { while !producer.is_finished() { let _ = rx.recv_timeout(std::time::Duration::from_millis(20)); } }
        let queue = producer.join().unwrap();
        assert!(returned, "the real-time callback must not wait for queue capacity");
        assert_eq!(status.failed(), QUEUE_OVERFLOW);
        assert_eq!(rx.try_iter().count(), 8, "memory must be bounded even when the owner stalls");
        queue.push(&[99i16], i16_identity);
        assert!(rx.try_recv().is_err(), "never concatenate resumed audio after a dropped batch");
    }
    #[test]
    fn oversized_batch_is_rejected_before_conversion_or_allocation() {
        fn must_not_convert(_: i16) -> i16 { panic!("oversized batch was converted"); }
        let (queue, rx) = CallbackQueue::new();
        queue.push(&vec![0i16; MAX_CALLBACK_SAMPLES + 1], must_not_convert);
        assert_eq!(queue.status.failed(), BATCH_TOO_LARGE);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn first_stream_error_stops_batches_and_cannot_be_replaced() {
        let (queue, rx) = CallbackQueue::new();
        queue.push(&[1i16, 2], i16_identity);
        queue.status.fail(STREAM_FAILED);
        queue.push(&[3i16, 4], i16_identity);
        queue.status.fail(QUEUE_OVERFLOW);
        assert_eq!(queue.status.failed(), STREAM_FAILED);
        assert_eq!(rx.try_recv().unwrap(), vec![1, 2]);
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn failed_stream_retains_only_accepted_prefix_and_never_publishes_mic_wav() {
        let dir = test_dir();
        let output = dir.join("mic.wav");
        let wav = create_wav_writer(&output, test_spec()).unwrap();
        let (queue, rx) = CallbackQueue::new();
        queue.push(&[11i16, 22], i16_identity);
        queue.status.fail(STREAM_FAILED);
        queue.push(&[33i16, 44], i16_identity);
        let error = queue.status.error().unwrap();
        let status = Arc::clone(&queue.status);
        drop(queue);
        let result = finish_samples(rx, wav, Err(error), &status);
        let published = output.exists();
        let prefix = hound::WavReader::open(dir.join("mic.wav.partial")).unwrap().samples::<i16>().collect::<Result<Vec<_>, _>>().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(result.unwrap_err().code, "mic_stream_failed");
        assert!(!published, "legacy importer must never see the failed track");
        assert_eq!(prefix, vec![11, 22], "recovery keeps accepted audio without post-error samples");
    }
    #[test]
    fn error_handler_keeps_receiver_alive_until_device_error_is_reported() {
        let (queue, rx) = CallbackQueue::new();
        let status = Arc::clone(&queue.status);
        let mut on_error = queue.error_handler();
        drop(queue);
        let before_error = rx.try_recv();
        on_error(cpal::StreamError::DeviceNotAvailable);
        drop(on_error);
        let after_error = rx.try_recv();
        assert!(matches!(before_error, Err(mpsc::TryRecvError::Empty)), "draining must wait for the error callback as well as the data callback");
        assert!(matches!(after_error, Err(mpsc::TryRecvError::Disconnected)));
        assert_eq!(status.failed(), STREAM_FAILED);
    }
    #[test]
    fn drain_rechecks_error_latched_after_stop_result() {
        let dir = test_dir();
        let output = dir.join("mic.wav");
        let wav = create_wav_writer(&output, test_spec()).unwrap();
        let (queue, rx) = CallbackQueue::new();
        queue.push(&[11i16, 22], i16_identity);
        let status = Arc::clone(&queue.status);
        let initial_stop_result = Ok(());
        // A final callback may fail after the owner's initial stop/status check.
        status.fail(STREAM_FAILED);
        drop(queue);
        let result = finish_samples(rx, wav, initial_stop_result, &status);
        let published = output.exists();
        let retained = dir.join("mic.wav.partial").exists();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(result.is_err(), "a late error must prevent publication");
        assert!(!published);
        assert!(retained);
    }
    #[test]
    fn clean_audio_is_published_only_after_finalize_and_validation() {
        let dir = test_dir();
        let output = dir.join("mic.wav");
        let mut wav = create_wav_writer(&output, test_spec()).unwrap();
        assert!(!output.exists());
        assert!(dir.join("mic.wav.partial").exists());
        write_samples(&mut wav.writer, vec![12, 34]).unwrap();
        wav.finish(Ok(())).unwrap();
        assert!(focusflow_core::audio::is_usable_wav(&output));
        assert!(!dir.join("mic.wav.partial").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn empty_audio_remains_partial_and_is_not_published() {
        let dir = test_dir();
        let output = dir.join("mic.wav");
        let wav = create_wav_writer(&output, test_spec()).unwrap();
        let result = wav.finish(Ok(()));
        let published = output.exists();
        let retained = dir.join("mic.wav.partial").exists();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(result.unwrap_err().code, "mic_audio_unusable");
        assert!(!published);
        assert!(retained);
    }
    #[test]
    fn existing_recovery_file_is_preserved() {
        let dir = test_dir();
        let output = dir.join("mic.wav");
        let partial = dir.join("mic.wav.partial");
        std::fs::write(&partial, b"earlier recovery").unwrap();
        let rejected = create_wav_writer(&output, test_spec()).is_err();
        let bytes = std::fs::read(&partial).unwrap();
        let published = output.exists();
        std::fs::remove_dir_all(dir).unwrap();
        assert!(rejected);
        assert_eq!(bytes, b"earlier recovery");
        assert!(!published);
    }
    #[test]
    fn output_created_during_capture_is_never_replaced() {
        let dir = test_dir();
        let output = dir.join("mic.wav");
        let mut wav = create_wav_writer(&output, test_spec()).unwrap();
        write_samples(&mut wav.writer, vec![1, 2]).unwrap();
        std::fs::write(&output, b"another recording").unwrap();
        let result = wav.finish(Ok(()));
        let bytes = std::fs::read(&output).unwrap();
        let retained = focusflow_core::audio::is_usable_wav(&dir.join("mic.wav.partial"));
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(result.unwrap_err().code, "mic_publish_failed");
        assert_eq!(bytes, b"another recording");
        assert!(retained);
    }
    #[test]
    fn owner_handle_is_send_sync_without_unsafe_impls() {
        fn check<T: Send + Sync>() {}
        check::<AudioRecorder>();
    }
    #[test]
    fn stop_signal_reaches_owner_while_finalization_is_still_pending() {
        let (stop, stop_rx) = mpsc::channel();
        let (ack, ack_rx) = mpsc::channel();
        let (finish, finish_rx) = mpsc::channel();
        let owner = thread::spawn(move || {
            stop_rx.recv().unwrap();
            ack.send(()).unwrap();
            finish_rx.recv().unwrap();
            assert!(matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
            Ok(())
        });
        let mut recorder = AudioRecorder { stop: Some(stop), owner: Some(owner) };

        recorder.request_stop();
        let acknowledged = ack_rx.recv_timeout(std::time::Duration::from_secs(1));
        let finalized_before_release = recorder.owner.as_ref().unwrap().is_finished();
        recorder.request_stop();
        let signal_removed = recorder.stop.is_none();
        drop(recorder.stop.take());
        let released = finish.send(());
        let stopped = recorder.stop();

        acknowledged.unwrap();
        assert!(!finalized_before_release);
        assert!(signal_removed);
        released.unwrap();
        stopped.unwrap();
    }

    #[test]
    fn unavailable_device_fails_before_returning_a_recording() {
        let path = std::env::temp_dir().join("focusflow-nonexistent-device.wav");
        let result = AudioRecorder::start(Some("focusflow-test-device-that-does-not-exist"), &path);
        assert!(result.is_err());
        assert!(!path.exists());
    }
}
