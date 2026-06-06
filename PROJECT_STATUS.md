# FocusFlow Status

## Working

- Screen recording
- FFmpeg sidecar recording and export
- AppData session folders
- Click tracking
- Drag tracking
- Timeline generation
- MP4 export
- Auto zoom and smooth camera pan
- Click indicators in exported video
- Export progress
- Recording countdown
- Window minimize and restore flow
- Global hotkeys
- Folder opening through Windows File Explorer
- FocusFlow logo and Tauri icons
- MSI build configuration

## Current Folder Structure

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

## Next Features

1. Audio and microphone capture
2. Timeline editor UI
3. Annotation tools
4. Export presets
5. Source selection
6. Share workflow

## Important Files

- `FocusFlow/src-tauri/src/recorder.rs`
- `FocusFlow/src-tauri/src/export.rs`
- `FocusFlow/src-tauri/src/lib.rs`
- `FocusFlow/src/App.tsx`
- `FocusFlow/src/App.css`
- `FocusFlow/src-tauri/tauri.conf.json`
