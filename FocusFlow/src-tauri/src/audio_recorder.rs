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
//!       │  Vec<i16>  (mpsc channel)
//!       ▼
//!  focusflow-audio-owner thread
//!  (normal priority, blocking I/O allowed)
//!       │
//!       ▼  hound::WavWriter
//!    mic.wav
//! ```
//!
//! The audio callback converts every incoming sample batch to `i16` and sends
//! it through a channel.  All file I/O happens on the writer thread, keeping
//! the realtime callback free of blocking operations.
//!
//! When [`AudioRecorder::stop`] is called the WASAPI stream is dropped first.
//! That drops the `Sender` inside the callback closure, which closes the
//! channel.  The writer thread exits its `recv` loop and calls
//! `WavWriter::finalize()`, writing the WAV chunk-size header.

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat,
};
use hound::{WavSpec, WavWriter};
use serde::Serialize;
use std::{
    fmt,
    path::Path,
    sync::mpsc::{self, Sender},
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
            let (stream, samples, mut writer) = match initialized {
                Ok(value) => value,
                Err(error) => { let _ = ready.send(Err(error.clone())); return Err(error); }
            };
            if ready.send(Ok(())).is_err() { drop(stream); return Ok(()); }
            let write_samples = |writer: &mut WavWriter<std::io::BufWriter<std::fs::File>>, batch: Vec<i16>| -> AudioResult<()> {
                for sample in batch { writer.write_sample(sample).map_err(wav_error)?; }
                Ok(())
            };
            loop {
                match stop_rx.try_recv() {
                    Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                match samples.recv_timeout(std::time::Duration::from_millis(20)) {
                    Ok(batch) => write_samples(&mut writer, batch)?,
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            drop(stream);
            for batch in samples { write_samples(&mut writer, batch)?; }
            writer.finalize().map_err(wav_error)
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
    fn finish(&mut self) -> AudioResult<()> {
        if let Some(stop) = self.stop.take() { let _ = stop.send(()); }
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
fn initialize_audio(device_name: Option<&str>, output_path: &Path) -> AudioResult<(cpal::Stream, mpsc::Receiver<Vec<i16>>, WavWriter<std::io::BufWriter<std::fs::File>>)> {
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
    let writer = WavWriter::create(output_path, spec).map_err(wav_error)?;
    let (tx, rx) = mpsc::channel();
    let stream = build_input_stream(&device, &stream_config, sample_format, tx)?;
    stream.play().map_err(|error| AudioError::new("stream_play_failed", error.to_string(), true))?;
    Ok((stream, rx, writer))
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
    tx: Sender<Vec<i16>>,
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
/// The error callback logs the WASAPI error to stderr but does not terminate
/// the recording — transient device errors (e.g. brief glitches) are
/// recoverable and the stream will resume automatically.
fn build_typed_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: Sender<Vec<i16>>,
    convert: fn(T) -> i16,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + Send + 'static,
{
    device.build_input_stream(
        config,
        move |data: &[T], _info: &cpal::InputCallbackInfo| {
            let samples: Vec<i16> = data.iter().copied().map(convert).collect();
            // Silently discard if the writer thread has already exited.
            let _ = tx.send(samples);
        },
        on_stream_error,
        None, // no timeout
    )
}

/// Platform stream error handler.
///
/// Logs the error to stderr.  Returning from this function allows cpal to
/// continue; transient errors will self-recover.  Fatal errors (e.g. device
/// removed) will eventually cause the callback to stop being invoked.
fn on_stream_error(error: cpal::StreamError) {
    eprintln!("[FocusFlow audio] Stream error: {error}");
}

// ── WAV writer thread ──────────────────────────────────────────────────────

/// Receives `Vec<i16>` batches from the audio callback via `rx` and writes
/// them to `mic.wav`.
///
/// Runs until the channel is closed (all senders dropped), then calls
/// [`WavWriter::finalize`] to write the RIFF chunk-size header.
///
/// Any I/O error is returned to the caller of [`AudioRecorder::stop`] which
/// joins this thread.
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
    #[test]
    fn owner_handle_is_send_sync_without_unsafe_impls() {
        fn check<T: Send + Sync>() {}
        check::<AudioRecorder>();
    }
    #[test]
    fn unavailable_device_fails_before_returning_a_recording() {
        let path = std::env::temp_dir().join("focusflow-nonexistent-device.wav");
        let result = AudioRecorder::start(Some("focusflow-test-device-that-does-not-exist"), &path);
        assert!(result.is_err());
        assert!(!path.exists());
    }
}
