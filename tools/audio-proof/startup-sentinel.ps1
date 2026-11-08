# Fixed harmless -File path discriminator, released only after exact Job assignment.
$ErrorActionPreference = 'Stop'
$gate = ''
$ended = $false
for ($i = 0; $i -lt 64; $i++) {
    $c = [Console]::Read()
    if ($c -lt 0) { exit 71 }
    if ($c -eq 10) { $ended = $true; break }
    $gate += [char]$c
}
if (-not $ended -or $gate -cne 'focusflow-startup-sentinel-v1') { exit 71 }
[Console]::Write('{"schemaVersion":1,"sentinel":true}')
exit 0
