//! Microphone audio recording backend for FocusFlow.
//!
//! Captures audio from a Windows WASAPI microphone using [`cpal`] and writes
//! 16-bit PCM samples to `mic.wav` in the active session folder using [`hound`].
//!
//! # Threading model
//!
//! ```text
//!  WASAPI callback thread
//!  (realtime priority, short deadline)
//!       │
//!       │  Vec<i16>  (mpsc channel)
//!       ▼
//!  focusflow-audio-writer thread
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

// Phase 1: AudioRecorder and its helpers are defined here but not yet called
// from recorder.rs.  They will be wired into ActiveRecording in Phase 2.
// Suppress dead_code warnings until that integration is complete.
#![allow(dead_code)]

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat,
};
use hound::{WavSpec, WavWriter};
use serde::Serialize;
use std::{
    fmt,
    path::{Path, PathBuf},
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
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        recoverable: bool,
    ) -> Self {
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
/// The `id` is the WASAPI device name string.  It is used as the selector
/// when calling [`AudioRecorder::start`].  On Windows, built-in device names
/// are stable across reboots; USB device names may change when re-plugged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDevice {
    /// Opaque selector — currently the WASAPI device name.
    pub id: String,
    /// Human-readable label shown in the UI microphone dropdown.
    pub name: String,
}

// ── Tauri command ──────────────────────────────────────────────────────────

/// Returns all available microphone input devices enumerated by WASAPI.
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

/// An active microphone recording session.
///
/// Create one with [`AudioRecorder::start`] and end it with
/// [`AudioRecorder::stop`].
///
/// # Drop behaviour
///
/// If dropped without calling `stop()`, the WAV header will **not** be
/// finalized and `mic.wav` will be unreadable.  Always call `stop()`.
pub struct AudioRecorder {
    /// Holding the stream keeps the WASAPI callback alive.
    ///
    /// Dropping it closes the mpsc channel (the `Sender` captured inside the
    /// callback closure is dropped), signalling the writer thread to exit.
    stream: cpal::Stream,

    /// Background thread that writes samples to mic.wav.
    writer_thread: Option<JoinHandle<AudioResult<()>>>,

    /// Path to the mic.wav being written — used for diagnostics only.
    output_path: PathBuf,
}

unsafe impl Send for AudioRecorder {}
unsafe impl Sync for AudioRecorder {}

impl AudioRecorder {
    /// Start recording from `device_name`, or from the system default
    /// microphone when `device_name` is `None`.
    ///
    /// `output_path` should be the full path to `mic.wav` inside the current
    /// session folder, e.g. `…/Recordings/2026-06-24T09-00-00/mic.wav`.
    pub fn start(
        device_name: Option<&str>,
        output_path: &Path,
    ) -> AudioResult<AudioRecorder> {
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
                     Windows microphone access is enabled in Privacy settings.",
                    true,
                )
            })?,
        };

        Self::start_for_device(&device, output_path)
    }

    fn start_for_device(
        device: &cpal::Device,
        output_path: &Path,
    ) -> AudioResult<AudioRecorder> {
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

        let (tx, rx) = mpsc::channel::<Vec<i16>>();

        let stream = build_input_stream(device, &stream_config, sample_format, tx)?;

        stream.play().map_err(|error| {
            AudioError::new(
                "stream_play_failed",
                format!(
                    "Could not start microphone stream: {error}. \
                     Try selecting a different device or restarting FocusFlow."
                ),
                true,
            )
        })?;

        let output_path_buf = output_path.to_path_buf();

        let writer_thread = thread::Builder::new()
            .name("focusflow-audio-writer".to_string())
            .spawn(move || run_wav_writer(rx, output_path_buf, spec))
            .map_err(|error| {
                AudioError::new(
                    "writer_thread_failed",
                    format!("Could not start audio writer thread: {error}"),
                    true,
                )
            })?;

        let device_label = device
            .name()
            .unwrap_or_else(|_| "<unknown>".to_string());

        println!(
            "[FocusFlow audio] Recording started \
             device={device_label:?} rate={sample_rate} ch={channels} \
             src_fmt={sample_format:?} → i16 PCM → {}",
            output_path.display()
        );

        Ok(AudioRecorder {
            stream,
            writer_thread: Some(writer_thread),
            output_path: output_path.to_path_buf(),
        })
    }

    /// Stop the recording and finalize `mic.wav`.
    ///
    /// 1. Drops the WASAPI stream — stops the callback and closes the channel.
    /// 2. Waits for the writer thread to flush remaining samples and write the
    ///    WAV header via `WavWriter::finalize()`.
    ///
    /// Returns an error if the writer thread panicked or if `hound` could not
    /// finalize the file.  The video session is unaffected either way — the
    /// caller should treat audio errors as non-fatal.
    pub fn stop(self) -> AudioResult<()> {
        let AudioRecorder {
            stream,
            mut writer_thread,
            output_path,
        } = self;

        // Step 1: drop the stream.
        //
        // This stops the WASAPI callback thread and drops the `Sender` that
        // was captured inside the callback closure.  Dropping the last Sender
        // closes the channel, causing `rx.recv()` in the writer thread to
        // return `Err(RecvError)`, which breaks the write loop and triggers
        // `WavWriter::finalize()`.
        drop(stream);

        // Step 2: join the writer thread.
        //
        // The thread's return value is `AudioResult<()>`, which propagates
        // any WAV write or finalize errors back to the caller.
        let writer_result = if let Some(handle) = writer_thread.take() {
            handle
                .join()
                .map_err(|_| {
                    AudioError::new(
                        "writer_thread_panicked",
                        "Audio writer thread panicked before mic.wav was finalized. \
                         The recording may be unreadable.",
                        true,
                    )
                })?
        } else {
            Ok(())
        };

        match &writer_result {
            Ok(()) => println!(
                "[FocusFlow audio] Recording finalized: {}",
                output_path.display()
            ),
            Err(error) => eprintln!(
                "[FocusFlow audio] Finalize error for {}: {error}",
                output_path.display()
            ),
        }

        writer_result
    }
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
        SampleFormat::I8  => build_typed_input_stream::<i8> (device, config, tx, i8_to_i16),
        SampleFormat::I16 => build_typed_input_stream::<i16>(device, config, tx, i16_identity),
        SampleFormat::I32 => build_typed_input_stream::<i32>(device, config, tx, i32_to_i16),
        SampleFormat::I64 => build_typed_input_stream::<i64>(device, config, tx, i64_to_i16),

        // Unsigned integer formats
        SampleFormat::U8  => build_typed_input_stream::<u8> (device, config, tx, u8_to_i16),
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

/// WASAPI stream error handler.
///
/// Logs the error to stderr.  Returning from this function allows cpal to
/// continue; transient errors will self-recover.  Fatal errors (e.g. device
/// removed) will eventually cause the callback to stop being invoked.
fn on_stream_error(error: cpal::StreamError) {
    eprintln!("[FocusFlow audio] WASAPI stream error: {error}");
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
fn run_wav_writer(
    rx: mpsc::Receiver<Vec<i16>>,
    output_path: PathBuf,
    spec: WavSpec,
) -> AudioResult<()> {
    let mut writer = WavWriter::create(&output_path, spec).map_err(|error| {
        AudioError::new(
            "wav_writer_failed",
            format!(
                "Could not create mic.wav at {}: {error}",
                output_path.display()
            ),
            true,
        )
    })?;

    // Write samples until the channel is closed.
    while let Ok(samples) = rx.recv() {
        for sample in samples {
            writer.write_sample(sample).map_err(|error| {
                AudioError::new(
                    "wav_writer_failed",
                    format!(
                        "Could not write audio sample to mic.wav: {error}. \
                         Check available disk space."
                    ),
                    true,
                )
            })?;
        }
    }

    // Flush buffered samples and write RIFF chunk sizes in the header.
    writer.finalize().map_err(|error| {
        AudioError::new(
            "wav_writer_failed",
            format!("Could not finalize mic.wav: {error}"),
            true,
        )
    })?;

    Ok(())
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
