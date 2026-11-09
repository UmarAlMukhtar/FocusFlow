# FocusFlow Windows loopback test candidate

This is a local diagnostic, **not a FocusFlow release**. This package runs on Windows x64 and does not install or modify FocusFlow. You need no Rust, Python, FFmpeg, account, or network connection.

## One deliberate launch

1. Extract the ZIP into a writable folder (a path containing spaces is supported). Double-click `Run-FocusFlow-Loopback-Test.cmd` on Windows x64.
2. Read the active output list and choose the number for the existing output you intend to play through. Nothing changes Windows default devices, routing, mixers, volume, services, or drivers. No microphone is requested. Other sounds on that output may be captured; close unrelated audio first.
3. The console creates and prints a fresh result folder containing `reference.wav`, a 12-second quiet, channel-distinct generated noise pattern with six bursts. Prepare that file in your normal player at a comfortable listening volume on the selected existing output. Do not play it yet. Do not change Windows settings to accommodate this test.
4. Type `START`. When the console says **CAPTURING**, play `reference.wav` once, within 20 seconds. Keep the console open. Capture stops automatically after 40 seconds, then finalizes. Allow up to about three minutes if storage/cleanup is slow.
5. Read the console result and `results/run-.../result.json`. All files remain local; nothing is uploaded. You may inspect them before sharing anything. The result contains the selected device's opaque ID/display label.

Each launch creates a fresh result folder. Do not precreate or reuse its `capture/` child directory. Results preserve `capture/source.partial.wav` and `capture/evidence.partial.json`; names are intentionally unchanged. The standalone test never installs or edits an app project.

## What the result means

- **Pass**: one explicitly selected System AudioSession reached admission, completed a direct Stop attempt, reconciled Terminal/WAV inventory and observed the exact worker's clean exit; independent WAV header/size/count/sample decoding matched the fixed per-channel six-burst content thresholds.
- **Unavailable**: no active output, or the captured existing route is outside this minimal analyzer's 48 kHz stereo support. Do not change Windows device settings solely for this test. No physical result is inferred.
- **Failed**: setup/capture/Stop/exit/WAV validation/content matching failed, or cleanup remains unconfirmed. Keep the report and partials. A timeout never counts as an exited child or a usable WAV.
- Cancelling before `START` records nothing and is not Pass. Closing the window during recording may leave unconfirmed partials; it is not a successful Stop test.

Playback through the selected route is your deliberate player choice, **not independently verified by this tool**. Content PASS does not certify speaker/headphone hardware, another device's silence, exact packet continuity, microphone operation, A/V synchronization, project placement, preview/export, or product readiness. The signature-search offset is fitted only to locate content; it must never become timing authority. The comparator is tolerant to scalar gain and quantization; it is not cryptographic seed identity or a bit-exact proof. Existing player effects, resampling, other sounds, low volume or a different route can fail this narrow test even when a WAV is readable.

Fixed policy: 48 kHz stereo; full supported PCM16/24/32 or float32 WAV decode; reconciled inventory/header/size match; 12..60-second analysis cap; sparse signature correlation >=0.98; each complete burst/channel correlation >=0.99; positive gain 0.001..4.0; silent-interval RMS <=0.002. Thresholds are frozen before any real-device run. No offset/gain fit is used for A/V claims.

