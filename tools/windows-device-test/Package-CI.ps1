# Dedicated test-only build. Never execute the tester CLI or an audio worker here.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($env:GITHUB_EVENT_NAME -ne 'push' -or $env:GITHUB_REF -ne 'refs/heads/codex-windows-device-test' -or $env:GITHUB_REPOSITORY -ne 'UmarAlMukhtar/FocusFlow') { throw 'Unexpected build authority' }
if ($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$' -or $env:GITHUB_RUN_ID -notmatch '^\d+$' -or $env:GITHUB_RUN_ATTEMPT -notmatch '^\d+$') { throw 'Missing build identity' }
Set-Location -LiteralPath $env:GITHUB_WORKSPACE
$Commit = & git rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $Commit -ne $env:GITHUB_SHA) { throw 'Checkout commit mismatch' }
$Tool = 'tools/windows-device-test'
$Manifest = "$Tool/Cargo.toml"
$MapPath = "$Tool/source-bindings.json"
$Source = Get-Content -Raw -LiteralPath $MapPath | ConvertFrom-Json -AsHashtable
$Roots = @($Tool, 'FocusFlow/crates/focusflow-core', 'FocusFlow/crates/focusflow-windows-audio-worker', '.github/workflows/windows-device-test.yml')
$Tracked = @(& git ls-files -- $Roots | Where-Object { $_ -ne $MapPath } | Sort-Object)
if ($LASTEXITCODE -ne 0 -or $Tracked.Count -lt 10 -or (Compare-Object $Tracked @($Source.Keys | Sort-Object))) { throw 'Source map must cover exactly the tracked staging inputs, excluding itself' }
function Assert-SourceMap {
    foreach ($Name in $Source.Keys) {
        if ($Name -notmatch '^(tools/windows-device-test|FocusFlow/crates/focusflow-core|FocusFlow/crates/focusflow-windows-audio-worker)/[A-Za-z0-9_/-][A-Za-z0-9_./-]*$' -and $Name -ne '.github/workflows/windows-device-test.yml') { throw "Unexpected source path: $Name" }
        if ($Name -match '(^|/)\.' -and $Name -ne '.github/workflows/windows-device-test.yml') { throw 'Hidden or traversal source path' }
        if ($Source[$Name] -notmatch '^[0-9a-f]{64}$') { throw 'Malformed source digest' }
        $Item = Get-Item -LiteralPath $Name
        if ($Item.PSIsContainer -or ($Item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Source is not a regular file' }
        if ((Get-FileHash -LiteralPath $Name -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Source[$Name]) { throw "Source mismatch: $Name" }
    }
}
Assert-SourceMap
$MapHash = (Get-FileHash -LiteralPath $MapPath -Algorithm SHA256).Hash.ToLowerInvariant()
$Toolchain = '1.89.0-x86_64-pc-windows-msvc'
$Target = 'x86_64-pc-windows-msvc'
$env:CARGO_TARGET_DIR = Join-Path $env:RUNNER_TEMP 'focusflow-device-test-target'
$Dist = Join-Path $env:GITHUB_WORKSPACE 'device-test-dist'
$Stage = Join-Path $env:RUNNER_TEMP 'focusflow-device-test-package'
foreach ($Path in @($env:CARGO_TARGET_DIR, $Dist, $Stage)) { if (Test-Path -LiteralPath $Path) { throw 'Build output must be fresh' } }
# Static CRT and path remapping keep compiler/local paths out of the portable payload.
$Flags = @('-C', 'target-feature=+crt-static', "--remap-path-prefix=$env:GITHUB_WORKSPACE=/source", "--remap-path-prefix=$env:USERPROFILE=/build-user", "--remap-path-prefix=$env:RUNNER_TEMP=/build-temp")
if ($env:CARGO_HOME) { $Flags += "--remap-path-prefix=$env:CARGO_HOME=/cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = $Flags -join [char]31
$env:CARGO_PROFILE_RELEASE_DEBUG = '0'
$env:CARGO_PROFILE_RELEASE_STRIP = 'symbols'
& rustup toolchain install $Toolchain --profile minimal
if ($LASTEXITCODE -ne 0) { throw 'Pinned official MSVC toolchain installation failed' }
$Rustc = (& rustup run $Toolchain rustc -vV) -join "`n"
if ($LASTEXITCODE -ne 0 -or $Rustc -notmatch 'release: 1\.89\.0' -or $Rustc -notmatch 'host: x86_64-pc-windows-msvc') { throw 'Wrong compiler' }
$Cargo = & rustup run $Toolchain cargo --version
if ($LASTEXITCODE -ne 0 -or $Cargo -notmatch '^cargo 1\.89\.0 ') { throw 'Wrong Cargo' }
# Only setup fetches registry dependencies. All compilation/tests below are locked/offline.
& rustup run $Toolchain cargo fetch --manifest-path $Manifest --locked --target $Target
if ($LASTEXITCODE -ne 0) { throw 'Locked dependency fetch failed' }
# Exact reviewed names only: no broad suite, binary test target, worker dispatch or native API.
$PureTests = @(
    'content_signature_accepts_gain_and_diagnostic_offset',
    'unrelated_signal_cannot_pass',
    'swapped_channels_cannot_pass',
    'missing_interior_burst_cannot_pass',
    'independent_inventory_rejects_declared_count_mismatch',
    'malformed_and_empty_wav_fail',
    'reference_has_independent_literal_golden_channel_order',
    'selection_is_explicit_and_bounded',
    'unsupported_existing_rate_is_unavailable_not_pass',
    'header_and_file_size_must_match_worker_inventory',
    'silence_is_not_a_content_pass'
)
foreach ($Test in $PureTests) {
    $Output = @(& rustup run $Toolchain cargo test --manifest-path $Manifest --target $Target --test offline --locked --offline -j 2 $Test -- --exact --nocapture)
    $Exit = $LASTEXITCODE
    $Output | Write-Host
    if ($Exit -ne 0 -or ($Output -join "`n") -notmatch 'test result: ok\. 1 passed; 0 failed;') { throw "Pure test did not run exactly once: $Test" }
}
& rustup run $Toolchain cargo build --manifest-path $Manifest --target $Target --bin focusflow-windows-device-test --release --locked --offline -j 2
if ($LASTEXITCODE -ne 0) { throw 'MSVC release build failed' }
Assert-SourceMap
if ((Get-FileHash -LiteralPath $MapPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $MapHash) { throw 'Source map changed during build' }
$Exe = Join-Path $env:CARGO_TARGET_DIR "$Target/release/focusflow-windows-device-test.exe"
if ((Get-Item -LiteralPath $Exe).Length -gt 64MB) { throw 'Unexpected executable size' }
$Bytes = [IO.File]::ReadAllBytes($Exe)
if ($Bytes.Length -lt 256 -or $Bytes[0] -ne 0x4d -or $Bytes[1] -ne 0x5a) { throw 'Missing PE executable' }
$Pe = [BitConverter]::ToInt32($Bytes, 0x3c)
if ($Pe -lt 0 -or $Pe -gt $Bytes.Length-6 -or [BitConverter]::ToUInt32($Bytes,$Pe) -ne 0x4550 -or [BitConverter]::ToUInt16($Bytes,$Pe+4) -ne 0x8664) { throw 'Executable is not AMD64 PE' }
# No raw environment dump or full local path enters build metadata. Reject embedded build roots too.
foreach ($Text in @([Text.Encoding]::UTF8.GetString($Bytes), [Text.Encoding]::Unicode.GetString($Bytes))) {
    foreach ($Local in @($env:GITHUB_WORKSPACE, $env:USERPROFILE, $env:RUNNER_TEMP, $env:CARGO_HOME)) {
        if ($Local -and ($Text.Contains($Local) -or $Text.Contains($Local.Replace('\','/')))) { throw 'Executable contains a local build root' }
    }
}
$BundleName = 'FocusFlow Windows Loopback Test'
$Bundle = Join-Path $Stage $BundleName
New-Item -ItemType Directory -Path $Bundle, $Dist | Out-Null
Copy-Item -LiteralPath $Exe -Destination $Bundle
foreach ($Name in @('Run-FocusFlow-Loopback-Test.cmd','README.md')) { Copy-Item -LiteralPath "$Tool/$Name" -Destination $Bundle }
Copy-Item -LiteralPath $MapPath -Destination (Join-Path $Bundle 'source-map.json')
$Build = [ordered]@{
    testOnly = $true; nativeExecuted = $false; commit = $Commit; repository = $env:GITHUB_REPOSITORY
    runId = $env:GITHUB_RUN_ID; runAttempt = $env:GITHUB_RUN_ATTEMPT; runner = 'windows-2022'
    builtUtc = [DateTime]::UtcNow.ToString('o'); target = $Target; profile = 'release'; staticCrt = $true
    toolchain = $Toolchain; rustc = $Rustc; cargo = $Cargo; sourceMapSha256 = $MapHash
    pureTests = $PureTests; lockSha256 = $Source["$Tool/Cargo.lock"]
}
$Build | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $Bundle 'build.json')
$Payload = @('focusflow-windows-device-test.exe','Run-FocusFlow-Loopback-Test.cmd','README.md','source-map.json','build.json')
$Hashes = @(foreach ($Name in $Payload) { $Path = Join-Path $Bundle $Name; [ordered]@{path=$Name; bytes=(Get-Item -LiteralPath $Path).Length; sha256=(Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()} })
$Hashes | ConvertTo-Json -Depth 3 | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $Bundle 'files.sha256.json')
$Expected = @($Payload + 'files.sha256.json' | Sort-Object)
if (Compare-Object $Expected @((Get-ChildItem -LiteralPath $Bundle -Force).Name | Sort-Object)) { throw 'Unexpected package member' }
$Archive = Join-Path $Dist 'FocusFlow-Windows-Loopback-Test.zip'
Compress-Archive -LiteralPath $Bundle -DestinationPath $Archive -CompressionLevel Optimal
$Zip = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    foreach ($Entry in $Zip.Entries) { if ($Entry.Name -eq '' -and $Entry.FullName.Replace('\','/') -ne "$BundleName/") { throw 'Unexpected ZIP directory' } }
    $Entries = @($Zip.Entries | Where-Object { $_.Name -ne '' })
    $ExpectedPaths = @($Expected | ForEach-Object { "$BundleName/$_" } | Sort-Object)
    if ($Zip.Entries.Count -gt 7 -or (Compare-Object $ExpectedPaths @($Entries.FullName | ForEach-Object { $_.Replace('\','/') } | Sort-Object))) { throw 'ZIP membership mismatch' }
    foreach ($Entry in $Entries) {
        $LocalFile = Join-Path $Bundle $Entry.Name
        if ($Entry.Length -ne (Get-Item -LiteralPath $LocalFile).Length) { throw 'ZIP size mismatch' }
        $Stream = $Entry.Open()
        try { $Hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($Stream)).ToLowerInvariant() } finally { $Stream.Dispose() }
        if ($Hash -ne (Get-FileHash -LiteralPath $LocalFile -Algorithm SHA256).Hash.ToLowerInvariant()) { throw 'ZIP content mismatch' }
    }
} finally { $Zip.Dispose() }
$ArchiveHash = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$ArchiveHash  FocusFlow-Windows-Loopback-Test.zip" | Set-Content -Encoding ascii -LiteralPath "$Archive.sha256"
Write-Host 'Verified portable ZIP produced. No tester CLI, worker, device capture or native qualification was executed.'
