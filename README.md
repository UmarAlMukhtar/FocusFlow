# FocusFlow

FocusFlow is a Windows-first desktop screen recorder built with Tauri, React, TypeScript, and Rust. It records the primary monitor, captures click and drag interactions, generates a zoom timeline, and exports an edited MP4 that automatically follows the user's attention.

## Features

- Primary monitor screen recording
- Bundled FFmpeg sidecar for recording and export
- AppData session folders for reliable release builds
- Left and right click tracking with coordinates and timestamps
- Click-and-drag path tracking
- Timeline generation from interaction data
- Automatic zoom and pan export to `edited.mp4`
- Click ripple indicators in exported videos
- Export progress display
- Recording countdown, automatic window minimize, and restore on stop
- Global hotkeys: `Ctrl+Shift+R` to start, `Ctrl+Shift+S` to stop
- Windows File Explorer shortcuts for recordings, sessions, and edited output

## Screenshots

Recommended release screenshots:

- Main FocusFlow dashboard before recording
- Active recording state with timer and animated red indicator
- Export progress state
- Example output folder containing `screen.mp4`, `clicks.json`, `drags.json`, `timeline.json`, and `edited.mp4`

## Architecture

FocusFlow uses the generated Tauri scaffold and keeps the app split into a small React UI and Rust backend commands.

- `FocusFlow/src/App.tsx` renders the single-screen demo UI and calls Tauri commands with `invoke()`.
- `FocusFlow/src-tauri/src/lib.rs` registers commands, global hotkeys, folder-opening commands, and runtime window constraints.
- `FocusFlow/src-tauri/src/recorder.rs` records the primary monitor with FFmpeg, tracks clicks and drags, creates session folders, and writes session data.
- `FocusFlow/src-tauri/src/export.rs` reads `screen.mp4` and `timeline.json`, renders zoom/click effects with FFmpeg, and writes `edited.mp4`.
- `FocusFlow/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` is bundled as the FFmpeg sidecar.

Recordings are stored under the app data directory:

```text
AppData/
  Roaming/
    com.umar.focusflow/
      Recordings/
        <session-id>/
          screen.mp4
          clicks.json
          drags.json
          timeline.json
          edited.mp4
```

## Installation

For local development on Windows:

```powershell
cd FocusFlow
pnpm install
pnpm tauri dev
```

For release builds:

```powershell
cd FocusFlow
pnpm tauri build
```

The release build requires the FFmpeg sidecar at:

```text
FocusFlow/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe
```

## Usage

1. Launch FocusFlow.
2. Press `Start Recording`.
3. Wait for the 3, 2, 1 countdown. FocusFlow minimizes automatically.
4. Perform the workflow to capture.
5. Stop from the app or press `Ctrl+Shift+S`.
6. Press `Export` to generate `edited.mp4`.
7. Use the folder buttons to open the recordings root, session folder, or edited video folder.

## Tech Stack

- Tauri 2
- Rust
- React
- TypeScript
- Vite
- FFmpeg sidecar
- Windows APIs for mouse tracking and global hotkeys

## Future Roadmap

1. Audio and microphone recording
2. Pre-recording source selection
3. Better drag-pan smoothing controls
4. Timeline editor UI
5. Annotation tools and cursor highlights
6. One-click share/export presets
