# Codex Usage

FocusFlow was built iteratively with Codex acting as a senior Rust and Tauri engineering assistant. The user provided feature requirements turn by turn, and Codex implemented scoped changes directly in the generated Tauri project.

## Architecture Generation

Codex first produced a Tauri + React screen recorder architecture focused on a Windows-first hackathon build. The architecture identified the minimum files needed for recording, click tracking, timeline generation, and FFmpeg export while keeping the generated Tauri scaffold intact.

## Rust Implementation

Codex implemented the backend in Rust using Tauri commands:

- `recorder.rs` for screen recording, session folders, click tracking, drag tracking, and timeline generation
- `export.rs` for FFmpeg rendering, zoom/pan effects, click indicators, and progress events
- `lib.rs` for command registration, folder opening, global hotkeys, and window constraints

Implementation stayed scoped to the requested files and avoided a new architecture after the scaffold was established.

## Debugging

Codex helped diagnose and fix release blockers, including:

- Windows path issues in MSI builds
- Relative recordings paths causing AppData migration work
- Long FFmpeg command lines causing Windows `os error 206`
- Invalid nested FFmpeg zoom expressions
- Folder-opening permission failures from Tauri opener APIs
- Runtime consistency for window minimum size

## Sidecar Migration

The project moved from system-installed FFmpeg to a bundled Tauri sidecar. Codex updated recording and export code to resolve the same sidecar executable through Tauri shell APIs and documented the required binary location:

```text
FocusFlow/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe
```

## Export Pipeline

Codex implemented a hackathon-quality export pipeline:

1. Read `screen.mp4`, `clicks.json`, and `timeline.json` from the latest session folder.
2. Probe video duration and dimensions with FFmpeg.
3. Generate camera keyframes from timeline events.
4. Write FFmpeg `filter_complex` to a script file.
5. Render `edited.mp4` with zoom, pan, and click indicator effects.
6. Emit export progress to the React UI when FFmpeg progress data is available.

## UI and Release Polish

Codex replaced the default Tauri template UI with a focused dark theme, countdown flow, recording timer, status badge, export controls, folder shortcuts, logo branding, and release documentation for hackathon submission.
