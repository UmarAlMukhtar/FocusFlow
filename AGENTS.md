# AGENTS.md

## Project Overview

FocusFlow is a Windows-first desktop app for creating polished product demos and tutorials from screen recordings.

It records screen, region, and window sources, tracks clicks and drag interactions, generates an interaction timeline, and exports an edited MP4 with automatic zoom, pan, and click indicators.

Current release target:

- `v0.2.0` â€” microphone audio recording and audio export stabilization

## Tech Stack

- Frontend: React, TypeScript, Vite
- Desktop shell: Tauri v2
- Backend: Rust
- Recording:
  - FFmpeg sidecar for screen/region capture
  - `windows-capture` for window capture
- Audio:
  - `cpal` for microphone capture
  - `hound` for WAV writing
- Export:
  - FFmpeg filter scripts
  - H.264 MP4 output
  - AAC audio when `mic.wav` is valid

## Repository Structure

Important paths:

- `FocusFlow/src/App.tsx` - main frontend UI and Tauri invoke calls
- `FocusFlow/src/App.css` - frontend styling
- `FocusFlow/src-tauri/src/lib.rs` - Tauri command registration and app setup
- `FocusFlow/src-tauri/src/recorder.rs` - recording lifecycle, session metadata, click/drag tracking
- `FocusFlow/src-tauri/src/audio_recorder.rs` - microphone device enumeration and WAV recording
- `FocusFlow/src-tauri/src/export.rs` - FFmpeg export pipeline
- `FocusFlow/package.json` - frontend package metadata and app version
- `FocusFlow/src-tauri/Cargo.toml` - Rust dependencies and app version
- `FocusFlow/src-tauri/tauri.conf.json` - Tauri app metadata and bundle config
- `CHANGELOG.md` - authoritative release notes
- `README.md` - public project documentation
- `docs/DEVELOPMENT.md` - local setup and build documentation

## General Coding Rules

- Make small, focused changes.
- Inspect the current code before editing.
- Do not rewrite working systems unless explicitly asked.
- Prefer stabilization over refactoring.
- Preserve existing behavior unless the task explicitly changes it.
- Do not introduce unrelated features.
- Do not do large UI redesigns during backend or release-stabilization tasks.
- Keep error messages clear and user-actionable.
- Avoid adding dependencies unless they are necessary and justified.
- Keep code readable over clever.

## Rust / Tauri Backend Rules

- Tauri commands must return clear `Result` types.
- Do not panic in user-facing recording/export paths.
- Recording failures should clean up partial state where practical.
- Once video recording has started, non-critical failures should not corrupt the whole session.
- Use structured helper functions instead of large inline command logic.
- Keep session metadata backward-compatible when possible.
- Do not expose unsafe code unless there is no reasonable safe alternative.
- If `unsafe` is used, document why it is sound.

## Recording Lifecycle Rules

The recording lifecycle is sensitive. Be careful when editing:

- `start_recording`
- `stop_recording`
- `stop_active_recording`
- click/drag tracking
- session folder creation
- metadata writing
- timeline generation

Expected session files may include:

- `screen.mp4`
- `mic.wav`
- `clicks.json`
- `drags.json`
- `timeline.json`
- `metadata.json`
- `edited.mp4`

Do not break screen, region, or window recording while working on audio.

## Audio Recording Rules

Microphone audio uses:

- `cpal` for input capture
- `hound` for WAV output
- `mic.wav` in the session folder

Rules:

- Microphone recording must default to OFF.
- Do not auto-enable microphone recording from saved settings.
- It is acceptable to persist the selected microphone device.
- If mic is explicitly enabled but no device is selected, block recording with a clear UI error.
- If mic start fails before video starts, fail cleanly.
- If mic finalization fails after video recording, save the video session anyway.
- Mark `hasMicAudio` true only if `mic.wav` is actually usable.
- Validate `mic.wav` using `hound::WavReader` before trusting it.
- A usable `mic.wav` must:
  - exist
  - be larger than the WAV header
  - be readable by `hound::WavReader`
  - have duration/sample count greater than 0

Do not add system audio unless explicitly requested.

## FFmpeg / Export Rules

Export logic lives in `FocusFlow/src-tauri/src/export.rs`.

Rules:

- Preserve no-audio export behavior.
- If no valid `mic.wav` exists, export with video only.
- If valid `mic.wav` exists, mux it as AAC audio.
- If FFmpeg audio export fails, retry without audio.
- Do not fail the whole export only because microphone audio is corrupted.
- Keep filter script generation separate from command execution.
- Avoid extremely long inline FFmpeg filter strings. Use filter script files.
- Do not reintroduce `zoompan d > 1` frame multiplication bugs.
- Log useful export diagnostics:
  - whether `mic.wav` was detected
  - whether audio export is enabled
  - final output size
  - fallback reason, if any

## React Frontend Rules

- Keep the UI simple and stable.
- Avoid major layout redesigns unless explicitly requested.
- Disable recording settings while recording.
- Show clear errors for missing mic devices or failed invokes.
- Use existing app style patterns.
- Do not add new UI libraries without explicit approval.
- Do not keep temporary labels such as `TEMP`, `Phase 1`, or `Test` in release UI.

## Versioning and Releases

Version files must stay consistent:

- `FocusFlow/package.json`
- `FocusFlow/src-tauri/Cargo.toml`
- `FocusFlow/src-tauri/tauri.conf.json`

Release naming:

- `v0.1.1` â€” recording stability update
- `v0.2.0` â€” microphone audio recording

Do not create git tags unless explicitly instructed.

Before release, update:

- versions
- `CHANGELOG.md`
- build artifacts only when requested

## Testing Commands

Run frontend and Tauri commands from the application directory:

```powershell
cd FocusFlow
pnpm run build
pnpm tauri dev
pnpm tauri build
```

Run Rust commands from the Tauri directory:

```powershell
cd FocusFlow/src-tauri
cargo check
cargo build
cargo test
```

Manual tests required for audio work:

1. Mic enabled recording:

   * select mic
   * record
   * confirm `mic.wav` exists
   * confirm `metadata.json` has `hasMicAudio: true`

2. Mic disabled recording:

   * record
   * confirm no required `mic.wav`
   * confirm session finalizes normally

3. Export with valid `mic.wav`:

   * confirm `edited.mp4` has video and AAC audio

4. Export without `mic.wav`:

   * confirm `edited.mp4` has video only

5. Export with empty/corrupted `mic.wav`:

   * confirm fallback to no-audio export

## Do Not Do

Do not add these unless explicitly requested:

* system audio
* captions
* timeline editor
* Remotion renderer
* cloud upload
* account/login system
* analytics
* auto-updater
* major UI redesign
* cross-platform rewrite
* large dependency changes
* unrelated cleanup

## Agent Workflow

Before coding:

1. Run `git status`.
2. Inspect relevant files.
3. Summarize current state.
4. Propose a focused plan.
5. Make changes only after the plan is approved for larger tasks.

After coding:

1. Run the relevant build/check commands.
2. Summarize files changed.
3. Explain behavior changes.
4. List manual tests still required.
5. Do not commit or tag unless explicitly asked.
