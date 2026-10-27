# Generated Windows capture probe

This standalone crate measures the pinned `windows-capture = 2.0.0` window encoder. It is not integrated into FocusFlow and never publishes an accepted capture clock. Only its own generated 640×360 popup is captured. There is no desktop/window enumeration, external source picker, cursor sampler, input injection or audio capture.

The parent owns the HWND because this pinned dependency rejects capture of a same-process window. A separate child validates the HWND's owner PID/title and receives a one-byte ownership gate only after assignment to a non-inheritable kill-on-close Job. Do not invoke the internal `worker` mode manually. The gate and title are implementation controls for this generated test, not a security boundary against a user who deliberately invokes internal modes.

## Local Windows run

Use an interactive Windows desktop supporting WGC and cursor-disable control. A hosted Windows runner may lack this capability; failure is evidence of an unavailable/failed attempt, not a passed runtime gate.

```powershell
cargo +1.89.0 test --locked --all-targets
cargo +1.89.0 build --locked
$env:PROBE_FFMPEG = 'C:\verified-tools\ffmpeg.exe'
$env:PROBE_FFPROBE = 'C:\verified-tools\ffprobe.exe'
python -m unittest discover -s . -p test_analyze.py -v
python run_probe.py --exe target/debug/focusflow-generated-windows-probe.exe --ffmpeg $env:PROBE_FFMPEG --ffprobe $env:PROBE_FFPROBE --evidence NEW_EVIDENCE_DIRECTORY
```

The workflow places the crate under `tools/windows-generated-probe`, uses a separate target directory and obtains the official 8.0.1 analysis binaries from a verified archive. It runs on pushes affecting this probe or its workflow on the dedicated `codex-windows-generated-probe` branch. Manual dispatch becomes available if the workflow is later included on the default branch. The Windows encoder under test is Media Foundation through `windows-capture`, not the FFmpeg analysis executable. The workflow does not package or distribute the vendor encoder.

## Evidence and interpretation

Five bounded scenarios run: first (350ms after WGC startup), static (3s), animated (3s), callback stall (350ms after submission 15), and source close (parent destroys only its window after 3s). Animation uses changing binary IDs and four distinct corner colors. The stall measures delayed capture/submission; it does not force an internal encoder/transcoder stall. No long-run or overload result is implied.

`native.jsonl` contains QPC frequency and startup time; client/window/DWM geometry and DPI; every successfully enqueued sequence, native 100ns compositor timestamp, explicit first timestamp, relative delta, dimensions, decoded source ID and callback/submission QPC brackets; and encoder finish events. `paint.jsonl` brackets synchronous painting; painting is not the compositor presentation timestamp. Source-close and callback-stall events are separately required by the analyzer.

FFprobe extracts every saved presentation timestamp and duration from both frames and packets. FFmpeg decodes RGB with timestamp passthrough and no rate or geometry filter. The independent Python decoder validates all four corner colors and binary IDs. `analysis.json` includes all PTS/durations, saved count, exact rational endpoint, native deltas, ordinal residuals and all matching source-ID sequence candidates. It never computes duration from configured FPS or enqueue count. A missing duration, failed finish, corrupt marker, changed geometry or unexplained packet/frame discrepancy fails closed.

`observed-preservation` means this finite sample's counts, ordinal IDs and timestamps agree within one saved time-base tick plus one 100ns native quantization unit. Duplicate IDs remain ambiguous and are reported; static pixels cannot prove unique frame identity. `resampled-or-unresolved` explicitly exposes changed cadence/count/identity or nonzero first PTS and exits nonzero. Neither status is an accepted product clock. Every analysis includes `authorityAccepted: false`.

## Bounds and ownership

- The main parent loop times out at 18s; a separate 23s watchdog exits the parent even if a parent native call blocks. OS handle closure then kills the assigned child. No dependency join runs in the parent.
- Child cleanup requests termination and polls at most 2s. It does not call an unbounded wait. If termination cannot be observed, the diagnostic says so; closing the Job is the fallback.
- An outer Python owner has a 28s parent deadline and a 3s termination-observation bound. A blocked or panicking WGC startup, stop or encoder finish stays in the owned child. Abrupt termination may leave an incomplete MP4; there is no salvage promise.
- The child admits at most 600 frames; each frame must be 640×360. Analyzer input is at most 64MiB video/2MiB receipt, decode at most 601 frames, each subprocess has a 30s deadline. Temporary RGB is deleted and never uploaded.
- Brushes are created/deleted per synchronous FillRect; BeginPaint/EndPaint owns the temporary paint DC; the parent alone destroys its HWND. Borrowed handles are not deleted. QPC/geometry calls use valid stack output pointers. The Job is non-inheritable, assigned before the gate, and closed on every parent exit. Before assignment, parent death closes stdin and the child exits before WGC startup.

The Actions workflow uses Windows2022, Rust1.89.0, Python3.12.10, commit-pinned checkout/setup/upload actions, an archive SHA-256 and a verified FFmpeg executable SHA-256. The hosted image/OS/GPU and installed rustup bootstrap remain GitHub-managed moving inputs; this is not a bit-reproducible VM or an installed release. Only generated receipt/media allowlist paths are uploaded, for seven days. No broad filesystem, screenshots, credentials, environment dumps, app package or analysis executable upload is requested. Native failure/unavailability/resampling makes the job fail after retaining available evidence.

Not measured: GDI/wall clock, Windows cursor image extraction, coordinate changes/mixed DPI, screen/region paths, physical input/audio, private/elevated desktop, parent-crash recovery, internal encoder overload, long-run timing, installer or product parity. These remain native acceptance gates; Linux tests and MSVC metadata checking do not satisfy them.
