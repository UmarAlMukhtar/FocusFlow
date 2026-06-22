# FocusFlow Development Roadmap

This document outlines the planned future milestones, releases, and feature developments for the FocusFlow project. As an open-source, community-focused utility, our roadmap is subject to feedback and contributions.

---

## Current Status
* **[v0.1.0] - Hackathon MVP**: Screen/Region recording, raw click tracking, automated timelines, basic FFmpeg zoom/pan export.
* **[v0.1.1] - Stability Update (Current)**: High-performance window capture via native APIs, window-scoped click filtering, relative coordinate mapping, timeline deduplication, camera easing, and export performance fixes.

---

## Release Milestones

### [v0.2.0] - Microphone Audio Support
* **Microphone Capture**: Introduce native microphone audio recording synchronized with screen capture tracks.
* **Audio Device Selector**: Add frontend settings panel selectors for input microphone hardware.
* **Audio/Video Multiplexing**: Update the export pipeline to combine the recorded audio track with the H.264 video.

### [v0.3.0] - System Audio & Mixing
* **System Loopback Audio**: Record internal desktop and application sound tracks on Windows.
* **Audio Track Mixing**: Allow users to adjust relative volume levels for microphone input and system sound before export.
* **Waveform Generation**: Generate raw audio level metadata for visual feedback.

### [v0.4.0] - Interactive Timeline Editor
* **Timeline Visualization**: Build a visual timeline track in the React interface displaying captured frames and click events.
* **Interactive Editing Handles**: Allow users to drag, resize, add, or delete zoom segments.
* **Zoom Coordinates Editor**: Let users click and adjust the zoom target center point for specific timeline events.
* **Segment Trimming**: Enable basic trimming of the recording start and end timestamps.

### [v0.5.0] - Visual Customizations & Presets
* **Cursor Styling**: Support styling the cursor highlight (e.g., custom colors, custom ripple shapes, cursor halo).
* **Text & Vector Annotations**: Allow overlays like text callouts, arrows, and borders to emphasize parts of the recording.
* **Export Presets**: Provide performance quality presets (e.g., Fast Render, High Quality, YouTube-optimized 1080p, Twitter-optimized, GIF export).

---

## Future Goals

* **Cross-Platform Compatibility**: Rebuild capture engines to support macOS (via ScreenCaptureKit) and Linux (via PipeWire/Wayland).
* **AI-Assisted Editing**: Incorporate AI transcription models to generate automated captions, and use transcripts to auto-trim video pauses.
* **Direct Sharing & Cloud Integration**: Create one-click uploads to popular developer platforms (e.g., Loom-like private shares, direct YouTube/Twitter uploads, or self-hosted S3 links).
