# FocusFlow v0.1.1 Release Notes

## Release Title: Recording Stability Update

FocusFlow v0.1.1 is a stability and bug-fix release that addresses core capture reliability and rendering performance issues. This update introduces high-performance window-scoped recording, corrects coordinate drifting, resolves timeline overlapping issues, and fixes a critical FFmpeg rendering bug that caused bloated output file sizes.

---

## Summary

In this release, we transitioned FocusFlow's Window Recording mode to utilize the native WinRT Graphics Capture API via the `windows-capture` crate. Along with native capture, mouse click telemetry is now filtered and stored relative to the target window. The export rendering engine was optimized to eliminate the FFmpeg frame multiplication bug, reducing export times from hours to seconds and restoring expected file sizes. Finally, camera pans and zooms were smoothed out using ease-in-out cosine curves and settle periods.

---

## Feature Improvements

* **WinRT Window Capture**: Replaced the unreliable FFmpeg `gdigrab hwnd` capture backend with the `windows-capture` crate, providing high frame rates and improved capture reliability.
* **Window-Scoped Click Filtering**: Clicks occurring outside the boundaries of the recorded window are ignored when recording a specific window.
* **Window-Relative Coordinate Mapping**: Mouse click coordinates are mapped relative to the target window at click time. Clicks now track perfectly with the application UI even if you drag the recorded window across your desktop.
* **Camera Easing**: Transitioned camera zooms and pans to ease-in-out cosine curves.
* **Camera Settle State**: Added a brief settle pause (`120ms`) at the end of camera pans, keeping the camera stable on click targets before initiating the next movement.

---

## Bug Fixes

* **Export Duration & File Size Bug**: Fixed a bug where the FFmpeg `zoompan` filter multiplied the output frame rate by the segment duration frame count. Exports that previously took hours and resulted in multi-gigabyte files now compile in seconds with correct output durations.
* **Timeline Segment Collisions**: Fixed a bug in the timeline compiler where clicks and drags occurring near each other created overlapping zoom segments starting at the same time. The compiler now uses a 300ms merge window to group nearby interactions.
* **Coordinate Drift**: Resolved click indicator alignment issues that occurred when recorded windows were moved or repositioned on the screen during a session.

---

## Known Limitations

* **Window Resizing**: Resizing target applications during active window recording is not fully supported. Expanding window boundaries may result in black frames in newly exposed space.
* **Audio Capture**: FocusFlow does not capture microphone or system audio in this release.
* **Manual Timeline Editor**: The editing timeline is generated automatically based on click telemetry. A graphical timeline editor is planned for a future release.
* **OS Support**: FocusFlow is Windows-only due to deep Win32 API hooks and WinRT Graphic Capture dependencies.

---

## Upgrade Notes

1. Download the `FocusFlow_0.1.1_x64_en-US.msi` installer from the [GitHub Releases Page](https://github.com/UmarAlMukhtar/FocusFlow/releases).
2. Run the installer to overwrite your previous installation.
3. Your historical recording sessions stored in `AppData/Roaming/com.umar.focusflow/Recordings/` will be preserved. Any unexported historical sessions can be exported using the new, faster export engine.

---

## Testing Notes

FocusFlow v0.1.1 underwent the following testing procedures before release:

* **Compilation and Lints**: Verified clean builds with `cargo check` and `pnpm tauri build`, and clean styling with `cargo fmt` and `cargo clippy`.
* **Capture Engine Validation**: Verified H.264 video streams are recorded correctly under Screen, Region, and Window capture modes.
* **Drift Testing**: Recorded an application window while actively moving it across a multi-monitor setup; verified exported click highlights remained perfectly aligned with the application buttons.
* **Performance Analysis**: Verified that a 20-second window recording session exports in under 15 seconds to a file under 10MB (compared to gigabytes in v0.1.0).
