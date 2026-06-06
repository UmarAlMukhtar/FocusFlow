# FocusFlow v0.1.0 Release Notes

## Summary

FocusFlow v0.1.0 is the first hackathon release of a Windows-first desktop screen recorder that turns raw screen recordings into focused demo videos with automatic zoom and click highlights.

## Highlights

- Record the primary monitor to `screen.mp4`
- Capture clicks, drag paths, and session metadata
- Generate a zoom timeline from user interactions
- Export `edited.mp4` with smooth zoom and pan effects
- Render click ripple indicators into the edited video
- Bundle FFmpeg as a Tauri sidecar for release builds
- Store recordings in AppData session folders
- Provide folder shortcuts for recordings and exported videos
- Show recording countdown, elapsed timer, status badge, and export progress
- Support global hotkeys on Windows

## Known Limits

- Windows-first implementation
- Primary monitor only
- No audio capture in v0.1.0
- No manual timeline editor yet
- Export quality is tuned for hackathon demo speed, not final production presets

## Verification

- Rust backend checks with `cargo check`
- TypeScript checks with `tsc`
- Frontend production build with Vite
- FFmpeg sidecar path verified in Tauri configuration

