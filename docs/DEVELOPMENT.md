# FocusFlow Development

This guide covers local setup and release builds for the application in `FocusFlow/`. For the public project overview, installation, and usage documentation, see the repository root `README.md`.

## Prerequisites

- Windows 10 or Windows 11.
- Node.js and `pnpm`.
- The Rust toolchain with the MSVC target.
- The native Windows prerequisites required by Tauri v2, including Microsoft C++ Build Tools and WebView2.
- The bundled FFmpeg sidecar described below.

## Frontend Setup

Run frontend commands from the application directory:

```powershell
cd FocusFlow
pnpm install
pnpm run build
```

`pnpm run build` runs the TypeScript compiler and creates the Vite production bundle.

## Rust and Tauri Setup

Run Rust checks from the Tauri directory:

```powershell
cd FocusFlow/src-tauri
cargo check
cargo build
cargo test
```

Run the desktop application from the application directory:

```powershell
cd FocusFlow
pnpm tauri dev
```

## FFmpeg Sidecar

The Windows FFmpeg sidecar must exist at:

```text
FocusFlow/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe
```

Screen and region recording and edited-video export depend on this bundled executable. A production build must include it through the existing Tauri sidecar configuration.

## Production Build

Build the frontend and Windows application from `FocusFlow/`:

```powershell
cd FocusFlow
pnpm run build
pnpm tauri build
```

Tauri writes release artifacts beneath `FocusFlow/src-tauri/target/release/bundle/`.

## Important Project Paths

- `FocusFlow/src/App.tsx`: main React UI and Tauri command calls.
- `FocusFlow/src/App.css`: application styling.
- `FocusFlow/src-tauri/src/lib.rs`: Tauri command registration and application setup.
- `FocusFlow/src-tauri/src/recorder.rs`: recording lifecycle and session persistence.
- `FocusFlow/src-tauri/src/audio_recorder.rs`: microphone device enumeration and WAV capture.
- `FocusFlow/src-tauri/src/export.rs`: FFmpeg export pipeline.
- `FocusFlow/src-tauri/Cargo.toml`: Rust dependencies and package metadata.
- `FocusFlow/src-tauri/tauri.conf.json`: Tauri application and bundle configuration.
- `FocusFlow/package.json`: frontend dependencies, scripts, and application version.
- `CHANGELOG.md`: authoritative release history.
- `README.md`: public project documentation.

## Generated Recordings

Generated recordings are stored in the application's data directory, not in the repository. Session folders may contain `screen.mp4`, `mic.wav`, interaction JSON files, metadata, and `edited.mp4`.

## Troubleshooting

- If Tauri reports that FFmpeg cannot be found, verify the exact sidecar path and filename above.
- If `pnpm` cannot resolve scripts or dependencies, confirm the current directory is `FocusFlow/` and run `pnpm install`.
- If Cargo cannot resolve the package manifest, run Rust commands from `FocusFlow/src-tauri/`.
- If no recording files appear in the repository, this is expected; inspect FocusFlow's application data recording directory instead.
- For release verification, follow the checklist in the repository root `README.md`.
