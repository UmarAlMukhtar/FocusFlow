# FocusFlow App

This folder contains the Tauri, React, TypeScript, and Rust application for FocusFlow.

## Development

```powershell
pnpm install
pnpm tauri dev
```

## Release Build

```powershell
pnpm tauri build
```

The FFmpeg sidecar must exist at:

```text
src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe
```

Generated recordings are stored in the application data directory, not in this project folder.
