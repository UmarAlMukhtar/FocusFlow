# FocusFlow Demo Scripts

## Recommended Recording Flow

1. Open FocusFlow and show the dark dashboard.
2. Click `Open Recordings Folder` to show where sessions are stored.
3. Press `Start Recording`.
4. Let the countdown display 3, 2, 1.
5. Show FocusFlow minimizing automatically.
6. Perform a short workflow with several clicks and one click-drag.
7. Stop recording with `Ctrl+Shift+S` or the Stop button.
8. Show the restored FocusFlow window with completed status.
9. Press `Export`.
10. Show the progress bar.
11. Open the session folder and play `edited.mp4`.

## 60 Second Demo Script

FocusFlow is a Windows desktop screen recorder for faster product demos.

I press Start Recording, and FocusFlow gives me a short countdown before minimizing itself so it does not appear in the capture.

While I use the app, FocusFlow records the screen and tracks clicks and drag movements in the background.

When I stop recording, it saves everything into a unique session folder: the raw screen recording, click data, drag data, and generated timeline.

Now I press Export. FocusFlow uses bundled FFmpeg to render an edited MP4 with automatic zoom, smooth pan, and click indicators.

The result is a focused demo video without manually editing zooms in a video editor.

## 3 Minute Demo Script

FocusFlow solves a common demo problem: raw screen recordings are hard to follow, and manually adding zooms takes time.

This is the FocusFlow desktop app. It is built with Tauri, React, TypeScript, Rust, and FFmpeg. The UI is intentionally small: start recording, stop recording, tune export settings, and open the generated folders.

I press Start Recording. Before recording begins, FocusFlow shows a 3, 2, 1 countdown. After the countdown it minimizes automatically, so the recording starts cleanly.

Now I perform a short workflow. FocusFlow records the primary monitor with FFmpeg while the Rust backend tracks clicks and drag paths using Windows APIs. Each session gets its own folder under AppData, so release builds do not depend on relative paths or user document folders.

I stop recording. FocusFlow restores the window, shows completed status, and has already generated `clicks.json`, `drags.json`, and `timeline.json` next to `screen.mp4`.

Next I press Export. The export pipeline reads the session files, generates camera keyframes, writes an FFmpeg filter script, and renders `edited.mp4`. The progress bar comes from FFmpeg progress events, so it is based on real export progress rather than a fake timer.

Here is the session folder. It contains the raw recording, interaction data, timeline, and edited MP4. When I play the edited video, the camera zooms into the important clicks, follows drag movement, and displays click indicators.

The goal is to reduce demo editing friction. Instead of recording, opening a separate editor, keyframing zooms, and exporting manually, FocusFlow makes a polished first-pass demo from the interaction data automatically.

The next steps are audio capture, a manual timeline editor, annotations, and export presets.
