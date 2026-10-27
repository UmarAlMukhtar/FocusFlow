# Generated Windows owned-frame experiment

This isolated probe captures only its own generated 640×360 HWND. It does not enumerate user windows, capture the desktop, change FocusFlow, accept a product capture clock, or record cursor/audio. `authorityAccepted` is always `false`. Native Windows execution is required for qualification; Linux Rust tests, metadata checks and synthetic FFmpeg tests establish narrower properties only.

The parent draws the original ID and asymmetric red/green/blue/yellow corners into one owned offscreen bitmap and performs a checked BitBlt in WM_PAINT. The bitmap/DC are reused and released through RAII. Static cases repaint once after capture readiness to request a post-admission observation. That readiness/paint is never the video epoch. Copied markers still have to validate; a blit is not claimed compositor-atomic.

## Owned pixels and causal clock

The WGC callback requests BGRA8 and copies every row into tightly packed owned CPU memory before returning. It validates dimensions, pitch, accessible length, original ID and corners, then stamps immutable snapshot sequence bits at y=224..256 (sample y=240). Original ID stays at y=128..192 (sample y=160). No borrowed slice or GPU frame leaves the callback. Immutable `Arc` references keep repeated/queued pixels alive.

Native timestamps are retained unchanged in native TimeSpan units of 100ns; this unit is not a measured resolution or accuracy. Queried QPC frequency can differ from 10MHz. Checked i128 rational arithmetic compares clocks. Admission A is sampled immediately before capture startup. Pre-roll timestamps are logged/discarded, never reassigned to zero. Negative or regressing timestamps and invalid copied pixels fail the case. Equal native timestamps are allowed, with distinct sequence identities and independently copied bytes retained. First valid post-admission native timestamp establishes E. Missing E after two seconds fails.

For output ordinal n, target is E+n/30, PTS n/30. A decision may run only once the complete cell end E+(n+1)/30 has elapsed. Among snapshots with native timestamp <= target delivered by that decision, it selects the lexicographically greatest (native timestamp, source sequence); higher sequence wins an equal-time tie. Static/missing updates hold the immutable last eligible snapshot. There is no frame invention, grid restart, skipped output ordinal or retroactive change when an old timestamp arrives late.

`copyEndQpc` brackets actual copy/tag completion. `copyCompleteQpc` is specifically the **published/available completion boundary**, sampled under the shared publication mutex immediately before nonblocking enqueue. The controller holds that same mutex while draining the source queue, sampling `decisionQpc`, and selecting. Therefore copied-but-unpublished memory cannot be mistaken for delivered memory. `deliveredThroughSeq` records the drained frontier to disambiguate publications after a decision within the same QPC tick. The analyzer checks the frontier against nondecreasing native timestamp ordering, strict sequence identity and publication availability and recomputes every causal choice independently. Receipt, ACK and ledger I/O happens after releasing publication state. Final state inspection uses a nonblocking snapshot after capture shutdown, so a timed-out callback cannot make finalization wait on that mutex.

The controller observes an explicit Stop request and samples/installs S when handling it. The parent's request time is diagnostic. For close, the parent waits for the installed-cutoff ACK and then destroys its HWND; WGC remains running until `on_closed` is observed, with a two-second deadline. A naturally earlier source-close observation installs the earlier cutoff. Any already selected endpoint exceeding that cutoff rejects the case. Parent death/control EOF is failure, never normal Stop.

Only N=floor(30*(S-E)) complete cells may be written; the final fractional cell is discarded and reported. The saved endpoint must independently equal N/30. No post-cutoff cells or black filler are emitted. Native/receipt counts and configured FPS are not saved-output evidence.

## Bounds and failures

- Source channel capacity 8; future queue capacity 8; one retained snapshot; output descriptor channel capacity 2. Queue overflow rejects explicitly.
- At most 22 owned pixel allocations including callback, both queues, retained/writer/controller references (20,275,200 bytes). The allocation counter enforces the cap and records high water; repeated `Arc` references share bytes.
- At most 600 native rows and 600 output ordinals; native/selection receipts capped at 2MiB; generated media capped at 64MiB. Separate supervisor monitors media size during feed/finalization. Stderr retains at most 64KiB and continues draining excess.
- One writer owns stdin and completes each frame in at most 64KiB write requests with a partial-write loop: Interrupted retries remaining bytes; zero/error fails; partial frames are never restarted.
- Independent supervisor alone requests termination when a frame write stays in progress over one second. Its result channel establishes whether the native action completed before supervisor shutdown; the controller persists that observed result. A two-second writer fallback is reported separately and cannot qualify independent-supervisor proof. Encoder finalization/reap, supervisor result observation and capture-stop observation each have a two-second bound. No unbounded joins.
- Parent loop 18 seconds, independent parent watchdog 23 seconds, Python process deadline 28 seconds plus at most 3 seconds shared cleanup observation.
- Parent assigns the gated worker to a non-inherited KILL_ON_JOB_CLOSE Job. Worker opens only a query handle to that named Job, verifies its encoder belongs to that exact Job before feed, then closes the query handle. The parent-death case separately tests whether parent-only termination kills worker and encoder during an armed phase without cooperative cleanup. Python retains the original child synchronization/termination handles; a kill request or missing PID is not exit proof.

Callback/copy/publication/scheduler/write delays and hold ages are reported. Native duplicate counts, the GCD of observed positive native steps (in 100ns units; zero when no positive step exists), and minimum as well as maximum callback-minus-native offsets are reported with exact rational offsets. The empirical GCD is not a universal clock-resolution claim; callbacks may precede native timestamps. No tolerance or epoch shift is inferred. Late observations do not rewrite output. There is no invented 100/25/250ms timing-accuracy gate. The 350ms callback-stall case must retain truthful mapping and expose its delay. The one-second write deadline is a bounded failure-handling mechanism, not a display/input accuracy claim. Unseen compositor updates, physical display/input latency, cursor, audio, and product/packaging acceptance remain unqualified.

## Encoder and independent saved-media checks

The runner resolves and hash-verifies FFmpeg from the existing pinned official archive, then passes its absolute path through parent and worker. No PATH discovery or encoder substitution occurs in native runs. The exact encoder command is:

```text
ffmpeg -hide_banner -loglevel warning -nostdin -n
  -f rawvideo -pixel_format bgra -video_size 640x360 -framerate 30 -i pipe:0
  -map 0:v:0 -an -c:v libx264 -preset ultrafast -crf 0 -pix_fmt yuv444p
  -bf 0 -fps_mode passthrough -video_track_timescale 30000 screen.mp4
```

`native.jsonl` stores all snapshot/copy brackets, Stop/source-close observations, ownership and terminal failure/finalization status. `selection.jsonl` separates each selection from its complete/partial write receipt. The independent Python analyzer decodes every saved frame and checks its snapshot tag, original ID and four corners against the recomputed causal selection. It requires contiguous ordinals, exactly N complete writes, exactly one video/no audio streams, packet/frame agreement, PTS n/30, duration 1/30, first PTS zero and file-derived endpoint N/30. It retains actual decoded/probed evidence when later validation rejects. Successful generated cases report mapping verified, lifecycle finalized, and latency measured with no accuracy bound claimed.

The dedicated branch workflow keeps its existing pinned checkout/Python/Rust/tool versions and upload allowlist. No other workflow or product files participate. CI's synthetic roundtrip verifies this exact FFmpeg command/options before native runs.

```powershell
cargo +1.89.0 test --locked --all-targets
cargo +1.89.0 build --locked
$env:PROBE_FFMPEG = (Resolve-Path analysis-tools/ffmpeg.exe).Path
$env:PROBE_FFPROBE = (Resolve-Path analysis-tools/ffprobe.exe).Path
python -m unittest discover -s . -p test_analyze.py -v
python run_probe.py --exe PATH_TO_PROBE --evidence NEW_DIRECTORY --ffmpeg $env:PROBE_FFMPEG --ffprobe $env:PROBE_FFPROBE
```

Finite native cases: first (350ms), static (3s), animated (3s), stall (3s with one 350ms callback pause), close (3s with ACK-before-destroy), blocked reader, reader exit mid-frame, and parent-only termination. Transport faults substitute a generated reader child for FFmpeg using the identical writer/supervisor/Job path: one never reads stdin, one reads 65536 bytes then exits. They stop capture after submitting a single frame so source/output queue overflow cannot mask the intended pipe failure. They qualify failure handling only, never capture mapping or FFmpeg behavior. Parent-death uses actual pinned FFmpeg. After exact-Job verification and at least one complete write, an idle writer and controller enter a generated-only ten-second parked phase. During that phase the controller cannot process EOF/source-close or run encoder cleanup, and its supervisor is parked. A QPC arm receipt must be at most one second old when Python observes both retained child handles still running and kills only the parent. Both exit observations must precede the QPC park deadline and fall within the shared three-second cleanup bound. The runner reserves the final second for exact-handle fallback termination if proof fails; fallback never qualifies Job enforcement. A stale arm, already-dead child, expired park or missing exit rejects proof. Ordinary parent-death handling outside this fixture stays unchanged. Failed scenarios retain their receipts and exit nonzero unless the exact expected fault and descendant cleanup are observed.
