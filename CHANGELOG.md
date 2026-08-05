# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## Unreleased

### Added
- Windows SmartScreen notice for unsigned installer transparency
- Release checklist with checksum step

### Improved
- Release documentation for Windows users

---
## v0.2.0 - Microphone Audio Recording

### Added
- Microphone device listing
- Optional microphone recording
- mic.wav saved inside session folders
- Microphone metadata in metadata.json
- AAC microphone audio muxed into edited.mp4 exports
- Selected microphone persistence

### Improved
- Microphone UI is now part of the normal recording setup
- Mic controls are disabled while recording
- Export detects usable mic.wav before muxing audio
- Invalid or corrupted mic.wav falls back to no-audio export

### Fixed
- Prevent starting mic-enabled recording without a selected microphone
- Avoid marking header-only or unreadable mic.wav as valid audio
- Preserve no-audio export behavior for sessions without microphone audio

---

## [0.1.1] - 2026-06-22

### Added
* **Native Window Capture**: Integrated the high-performance `windows-capture` Rust crate for capturing specific application windows.
* **Window-Scoped Click Filtering**: Clicks that occur outside the bounds of the active recorded window are now automatically ignored during window-scoped recordings.
* **Window-Relative Click Coordinates**: Clicks tracked during window recordings are saved using coordinates relative to the window's top-left corner, ensuring click alignment is maintained even when the target window is moved.
* **Timeline Easing**: Applied ease-in-out cosine interpolation curves for camera panning and zoom transitions to make transitions feel organic.
* **Settle Keyframes**: Added a post-pan settle transition (`CAMERA_SETTLE_MS` of 120ms) that briefly pauses camera movement on click targets before moving to the next interaction.
* **Export Diagnostics**: Added debug output to console tracking input video duration, expected output duration, segment counts, script size, and final file sizes.

### Changed
* **Replaced Unreliable Captures**: Removed the buggy FFmpeg `gdigrab hwnd` capture backend for Window recording.
* **Camera Easing Durations**: Updated `PAN_TRANSITION_MS` to 220ms and `ZOOM_IN_MS` to 180ms to provide a smoother panning path.
* **Optimized Zoompan**: Modified the FFmpeg `zoompan` filter configuration to use a constant duration value `d=1` for video input segments.

### Fixed
* **FFmpeg Frame Multiplication**: Resolved a critical issue where the `zoompan` filter multiplied video frames by the segment duration frame count, which resulted in massive file sizes, long export times, and extended video durations.
* **Timeline Segment Collisions**: Fixed a bug in the timeline compiler where clicks and drags occurring near each other created overlapping or duplicate zoom segments starting at the identical millisecond. The compiler now uses a single-pass merge-and-clip cleanup window of 300ms.
* **Click Alignment Drift**: Resolved click indicators drifting or misaligning when the recorded application window was moved.

### Known Limitations
* **Window Resizing**: Resizing application windows during window capture is not fully supported; expanding window dimensions may result in black borders.
* **Audio Support**: FocusFlow remains video-only. Microphone and system audio recording are planned for future versions.
* **Manual Editing**: There is no timeline editing interface yet. Visual sequences are generated entirely from interaction heuristics.
* **OS Support**: FocusFlow is Windows-only due to dependency on Windows-native graphics capture and global user hook APIs.

---

## [0.1.0] - 2026-06-15

### Added
* **Screen & Region Recording**: Primary monitor capture and customizable regional bounding-box recording via FFmpeg.
* **Interaction Tracking**: Silently logs cursor click coordinates, click durations, and drag-and-drop paths in the background.
* **Automatic Timeline Compiler**: Automatically converts mouse events and drag sequences into camera tracking timelines (`timeline.json`).
* **Visual Click Ripples**: Renders visual indicator ripples at click positions on export.
* **Tauri Desktop Scaffold**: Integrated Tauri v2 with a custom, native-looking dark-mode UI built on React and Tailwind CSS.
* **Bundled Sidecars**: Packaged FFmpeg within the Tauri build so users do not need to install system-wide dependencies.
* **Global hotkeys**: Add support for global key bindings (`Ctrl+Shift+R` to start, `Ctrl+Shift+S` to stop recording).
