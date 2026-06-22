# FocusFlow - Project Status

## Version: v0.1.1 (Stable Release)

---

## Current Stable Features

* **Multi-Source Recording**:
  * **Screen**: Captures full primary monitor.
  * **Region**: Captures user-defined desktop bounding-box area.
  * **Window**: Captures a targeted application window.
* **Low-Overhead Interaction Tracking**:
  * Captures mouse click events (timestamp, button, absolute and window-relative coordinate points).
  * Captures mouse drag tracks (origin points, movement vectors, release points).
  * Automatically filters out clicks occurring outside the target window during Window recording.
* **Automated Timeline Compilation**:
  * Parses click and drag datasets into non-overlapping, sorted zoom/pan camera instructions.
  * Employs a 300ms cleanup window to merge overlapping drag sequences and rapid click patterns into single continuous camera motions.
* **Polished MP4 Export Engine**:
  * Generates custom, optimized FFmpeg scripts executing in-memory.
  * Applies ease-in-out (cosine) easing for camera movements.
  * Embeds a post-pan settle state (`CAMERA_SETTLE_MS` = 120ms) to briefly focus attention on targets.
  * Uses a clean ripple effect to indicate mouse clicks.
  * Operates at `d=1` frame multiplication safety, preserving native source video durations and outputting standard MP4 sizes.
* **Desktop UX Comforts**:
  * Starts recordings with a visual 3-second countdown.
  * Houses a global system hotkey daemon (`Ctrl+Shift+R` to record, `Ctrl+Shift+S` to stop).
  * Exposes visual progress indicators during export.
  * Integrates deep directory shortcuts to navigate to AppData session files and exports using Windows Explorer.

---

## Current Architecture

FocusFlow runs a hybrid web-native system:

1. **Presentation Layer (React, TS, Tailwind CSS)**: Manages layout state, active user settings, session browsing, and rendering triggers. Connects to backend endpoints using Tauri's typed IPC command module.
2. **Controller Layer (Tauri Rust Commands)**: Validates input, parses settings, monitors OS window lifecycles, registers global hotkeys, and runs the recorder state machine.
3. **Capture Services (Rust Engine)**:
   * Launches raw background captures.
   * Runs a Win32 global mouse hook thread to stream cursor input.
   * Compiles event logs (`clicks.json` and `drags.json`) and finalizes recording timelines.
4. **Rendering & Export (FFmpeg Sidecar)**: Parses timeline logs, constructs multi-stage visual filter scripts, runs the FFmpeg process, and returns real-time progress calculations.

### Workspace Folder Structure

```text
AppData/
  Roaming/
    com.umar.focusflow/
      Recordings/
        <session-id>/
          screen.mp4        <- Raw video capture file
          clicks.json       <- Mouse click coordinates & timestamps
          drags.json        <- Mouse drag segments & timestamps
          timeline.json     <- Compiled, non-overlapping camera timeline
          edited.mp4        <- Exported, post-processed video file
          capture.log       <- Log stream outputting active status messages
```

---

## Current Recording Backends

1. **FFmpeg GDI Capture**: Utilized for full Screen and Region recordings. It records visual output at 30 frames per second using direct desktop capture and encodes H.264 video.
2. **WinRT Graphics Capture (via `windows-capture` crate)**: Used for Window Recording. FocusFlow hooks directly into the Windows Graphic Capture API to record the targeted window, ensuring highly reliable capturing and avoiding system lag.

---

## Current Export Pipeline

The rendering engine converts raw recordings into final video artifacts through the following automated sequence:

```text
1. Parse Session Logs ──► Loads clicks.json, drags.json, and timeline.json.
2. Build Keyframes ────► Calculates x, y, and zoom settings for every segment.
                         Applies cosine easing to interpolate pan and zoom steps.
3. Generate Filter ────► Generates FFmpeg complex filter strings using:
                         • 'trim' and 'setpts' for segment boundaries.
                         • 'zoompan' (d=1) to process frames without duplication.
                         • Overlay expressions for click ripple effects.
4. Spawn Sidecar ──────► Executes the bundled FFmpeg binary.
5. Stream Progress ────► Tauri parses frame metrics to dispatch progress events to React.
6. Atomic Finalize ────► Renames temp assets to 'edited.mp4' on success.
```

---

## Important Files

* [lib.rs](file:///c:/Users/Umar/OneDrive/Documents/FocusFlow/FocusFlow/src-tauri/src/lib.rs): Tauri plugin registration, window setup, global hotkeys, and Windows shell openers.
* [recorder.rs](file:///c:/Users/Umar/OneDrive/Documents/FocusFlow/FocusFlow/src-tauri/src/recorder.rs): Captures recording states, tracks mouse movements, filters window clicks, and compiles timelines.
* [export.rs](file:///c:/Users/Umar/OneDrive/Documents/FocusFlow/FocusFlow/src-tauri/src/export.rs): Formulates keyframes, generates FFmpeg filter expressions, manages the sidecar process, and exports MP4 files.
* [App.tsx](file:///c:/Users/Umar/OneDrive/Documents/FocusFlow/FocusFlow/src/App.tsx): Core React UI controller managing layout states, recording sources, and export tasks.
* [App.css](file:///c:/Users/Umar/OneDrive/Documents/FocusFlow/FocusFlow/src/App.css): Core CSS styling and design layout rules.
* [tauri.conf.json](file:///c:/Users/Umar/OneDrive/Documents/FocusFlow/FocusFlow/src-tauri/tauri.conf.json): Tauri desktop capabilities, packaging properties, and sidecar binary mappings.

---

## Known Issues & Limitations

* **Resize Boundary Box**: Resizing target applications during window recording is not fully supported. Expanding the window may result in black frames in newly exposed space.
* **Audio-less Recording**: FocusFlow does not record microphone inputs or system sound.
* **Automated Editing Only**: There is no timeline editing UI. Custom zoom coordinates and camera targets must be configured programmatically.
* **OS Compatibility**: FocusFlow is Windows-only due to deep Win32 API hooks and WinRT Graphic Capture dependencies.

---

## Next Planned Release

### **v0.2.0: Audio Recording Integration**
* Implement microphone audio capture using the `cpal` crate.
* Add audio device picker dropdowns to the primary React interface.
* Update the export pipeline to multiplex the recorded audio track with the final video output.
