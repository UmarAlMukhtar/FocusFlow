# Controlled Windows encoder audio profile CI

This dedicated qualification lane builds a **new** controlled PE and runs that
PE's bounded generated consumer commands on Windows. It does not replace a
FocusFlow sidecar or run desktop capture, microphone capture, Tauri, installers,
or the product's Rust lifecycle. WAV fallback is explicitly modeled by the
existing command smoke harness; no product fallback integration claim is made.

The workflow listens only to `codex-controlled-encoder-audio-qualification`. Publication
of this staged payload requires root review. Run permissions are contents-read;
there is no push, release, tag, repository upload, or credential setup step.

## Inputs and provenance

`inputs.lock.json` contains the exact existing FFmpeg 8.0.3, unsigned x264
b35605ace3ddf7c1a5d67a2eb553f034aef41d55 and zlib 1.3.2 archive identities,
11 signed target MinGW RPMs, signed NASM binary/source RPMs, six signed target
compiler/CRT source RPMs, and detached source signatures. Official source/tool
routes and hashes were copied from receipts, not inferred from package names.
x264 uses the recorded canonical Git fetch/archive route and checks both Git
tree and exact resulting tar hash. It never follows a moving branch.

The Fedora44 AMD64 image digest and download-artifact action commit were verified
by root using official registry/GitHub metadata. The build container installs
host helpers (including native GCC for FFmpeg's host generators) and the exact RPM-header-derived host ELF library capabilities only from Fedora's `fedora`/`updates` repositories. Exact installed
NVR, source RPM, header/payload SHA256, cached RPM SHA256 and tool executable
SHA256 identities are recorded at job time. This explicit root-approved host
helper policy allows repository drift; it does not claim historical host-tool
identity or bit-identical reproduction of candidate b10d6e14. Target cross-tool
RPMs, source archives and build profile remain exact pins.

`provenance-index.json` binds the small original receipts to their staged copies.
Historical private checkout prefixes in those copies become repository-relative
provenance. They are never used as executable lookup paths. Public signing keys
are checked in and hash-pinned; signature verification uses private temporary
RPM/GnuPG databases and never imports into the host's key stores. zlib retains
its historical DSA1024/SHA1 authentication limit; no policy override is permitted.

The original two-job commands are reused. The profile adds only filters
`amix`, `asetrate`, `afade`, encoder `pcm_f32le`, and demuxer/muxer
`pcm_f32le` (runtime format `f32le`) for private sequential float PCM I/O.
No source version, target tool, library, capture input or dependency is added. The original recipe's
full environment dump is replaced by an explicit build-variable allowlist, and
the recipe is invoked under `env -i`. The inspector reuses the existing config,
PE, imports, symbol, static-link and notice checks. Its baseline config is a
small staged receipt instead of a frozen sibling candidate directory. A new
required gate compares the final selected external members against all 394
retained source mappings plus one explicitly closed audio-profile cbrt member,
and three hash-pinned startup objects. Unknown or missing contributions fail
before handoff. The singleton additionally verifies its pinned archive/member
hashes and complete retained source/header/generated-route/notice file set. This preserves the original bundle's 34 pending
header-closure entries and legal/distribution limitations; it does not promote
them to complete. Complete exact source RPMs and encoder archives are retained
as CI inputs. The historical source-index's `upstream/` and `source-units/` paths
describe members recoverable from those original source RPMs/archive inputs;
this CI input artifact is not a repack of the older full source bundle.

## Flow and artifact boundaries

1. Ubuntu runs 19 offline controls and checks pinned metadata.
2. The pinned disposable Fedora44 container installs/records official host
   helpers, downloads exact inputs, validates hashes/signatures, and extracts
   target RPM payloads into a new owned root without running RPM scriptlets.
   Archive traversal, escaping links and symlink parents are rejected. Absolute
   package helper links become equivalent relative links inside that root.
   Sources are compared against their archives by the reused recipe.
3. The new controlled PE is statically inspected and handed off with a complete
   file manifest. The handoff contains only its PE, receipts, notices and recipe.
   Full source/tool input archives and their checksums are a separate artifact.
4. Windows downloads that run's owned handoff and verifies its file set/hashes
   and candidate receipt before execution. The existing pinned vendor encoder
   is an independent decoder/prober only; it never encodes or supplies fixtures.
5. The reused smoke and smoke-extra scripts generate owned raw BGRA and run
   candidate H264 encoding, framehash/clock probes, scaling, zoompan d=1,
   styled composition, PAM alpha cursor overlays, PNG still export, concat/setts,
   all five accepted PCM WAV forms to AAC, video-only export, invalid/empty/
   missing-WAV explicit fallback, silence preview and bounded kill/reap. Every
   child has a deadline. Success is written only after all checks finish.

Only explicit setup/receipt/log/media paths are uploaded, with seven-day
retention. No whole working directory, environment dump, desktop input, live
recording, global host install, product build, source mutation, or sidecar copy
is part of the lane. Cached RPMs are source/tool qualification inputs, not
repository binary uploads. The 7-day artifacts are not a public source-delivery
or release route.

## Offline preparation checks

From the checkout containing this directory:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tools/controlled-encoder-ci -v
python3 tools/controlled-encoder-ci/validate.py --platform
python3 tools/controlled-encoder-ci/prepare.py --cache /chosen/input-cache --work /chosen/unused-work --plan-only
bash -n tools/controlled-encoder-ci/build.sh
bash -n tools/controlled-encoder-ci/host-tools.sh
```

`--plan-only` does not create directories, read source archives, install tools,
use the network, compile or execute any PE. The actual acquisition/preparation/
build/native commands are visible in the workflow and require review before
publication. Native invocation is:

```powershell
python encoder-handoff/recipe/windows-smoke.py --candidate encoder-handoff/candidate/ffmpeg.exe --ffmpeg-reference $env:ENCODER_REFERENCE --ffprobe-reference $env:ENCODER_PROBE --output encoder-evidence
```

Preparation has not executed the container, DNF, download routes, extraction,
rebuild, static inspector against a rebuilt PE, or Windows smoke. Those remain
first-run compatibility/runtime checks, including normal-policy zlib signature
acceptance. On acquisition failure, the error names the file, received count and exact byte limit; the explicit acquisition-failure.json receipt and completed acquisition.json are uploaded by the failure path. Build failures also retain the exact FFmpeg `build/ffmpeg/ffbuild/config.log`; the failure upload does not include the whole build tree. The signed source RPM byte/hash/URL identities are checked against FIX1 final receipts before network work.

No command-only smoke success can qualify hardware recording,
installed product behavior or distribution/legal closure.


## Additional audio profile boundary

The existing 50 Windows consumer commands remain in the lane. A separate
`windows-audio-profile.py` runs nine candidate commands: startup/version and
capability listings, raw-float write/reopen, distinguishable stereo fixed
0.5/0.5 mixing across sibling EOF, complementary 256-sample triangular fades,
and a simple 2x rate smoke. Output is at most 2048 stereo frames per raw file;
commands have 20-second deadlines and use explicit generated owned paths.
Python checks exact frame/channel byte counts, finite float samples, expected
amplitude and unchanged fixed gain. Receipts and file hashes include failures.
No independent tool encodes fixtures. The new script is not the fixed-Q mapper
and does not qualify mapper context/seek/short-segment policy or native capture.

This is a preparation candidate. The script has not run on Windows here; no
new binary has been built. The exact selected-member gate requires the
historical 394 members plus the reviewed cbrt singleton (395 total).
Resolve such a failure by separately reviewing exact new contributions, never
by weakening/removing that gate. Historical pending header/legal/source-delivery
limits remain open. Root review precedes publication on the separate branch.


## Explicit cbrt contribution closure

Run 41773701812 attempt2 stopped after a successful build and static PE checks:
it selected one additional member, `libmsvcrt.a(lib64_libmsvcrt_common_a-cbrt.o)`.
`provenance/audio-profile-extra/coverage.json` explicitly closes only that member;
the historical coverage-397.json is unchanged. The new gate requires exactly
395 external members, the existing three startup hashes, the pinned archive and
member hashes, and every retained source/notice file. Additional unknown or
missing members still fail; the receipt is not a generic exception list.

The small retained tree contains full cbrt.c, its 13 conservative reachable
headers, exact generated _mingw.h and _mingw_ddk.h plus their templates/rules,
Fedora CRT/header package recipes, upstream notices and all source/header text.
The source tar/member route comes from the already verified pinned source RPM;
package-provenance.json binds fresh local hashes/header queries to those retained
verification receipts and the failed CI target payload. The separate installed
SDK header uses the actual generated `#if 1` route rather than the source-tree
fallback `#if 0`. This narrow closure does not repair or promote historical
header unions, legal completeness, source delivery, native tests or releases.

prepare.py carries the helper and closure into the isolated candidate. The
inspector retains its unchanged startup checks, copies additional notices, and
records the explicit +1 result. artifact.py recursively carries the extra source
receipt tree and notices into the hash-verified handoff. New tests exercise exact
acceptance, unknown/missing contributions, source/archive/member/base/extra-entry
tampering, and actual source/notice retention through packing. These are offline
gate/handoff checks; no new controlled build or Windows execution follows from
them. Root fresh review and a new controlled run are still required.
