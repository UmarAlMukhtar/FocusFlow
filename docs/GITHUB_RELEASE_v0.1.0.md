# GitHub Release: v0.1.0

FocusFlow v0.1.0 is the first hackathon-ready build.

FocusFlow records your screen, tracks clicks and drags, builds a timeline from those interactions, and exports an edited MP4 with automatic zoom, smooth pan, and click indicators.

## What's Included

- Windows MSI build support
- Bundled FFmpeg sidecar
- Primary monitor screen recording
- Click and drag tracking
- Per-session AppData recording folders
- Timeline generation
- Auto zoom export to `edited.mp4`
- Click ripple rendering
- Export progress UI
- Countdown before recording
- Window minimize and restore flow
- Global hotkeys:
  - `Ctrl+Shift+R` starts recording
  - `Ctrl+Shift+S` stops recording

## Output Structure

```text
Recordings/
  <session-id>/
    screen.mp4
    clicks.json
    drags.json
    timeline.json
    edited.mp4
```

## Notes

This release is optimized for hackathon demonstration on Windows. Audio capture, manual editing, and source selection are planned future improvements.

