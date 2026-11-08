# Fixed -Command startup probe. The leased fetch source is data, never executed.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$phase = 'encoding'
$major = 0
$minor = 0
$policy = 'unavailable'
$dataRead = $false
$parserComplete = $false
$parseErrors = 0
$initialization = $false
try {
    [Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false, $true)
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false, $true)
    $phase = 'gate'
    $path = ''
    $ended = $false
    for ($i = 0; $i -lt 4096; $i++) {
        $c = [Console]::Read()
        if ($c -lt 0) { throw 'gate' }
        if ($c -eq 10) { $ended = $true; break }
        if ($c -eq 0 -or $c -eq 13) { throw 'gate' }
        $path += [char]$c
    }
    if (-not $ended -or $path.Length -eq 0) { throw 'gate' }
    $phase = 'version'
    $major = [int]$PSVersionTable.PSVersion.Major
    $minor = [int]$PSVersionTable.PSVersion.Minor
    if ($major -lt 0 -or $major -gt 16 -or $minor -lt 0 -or $minor -gt 16) { $major = 0; $minor = 0; throw 'version' }
    $phase = 'read'
    $bytes = [IO.File]::ReadAllBytes($path)
    if ($bytes.Length -eq 0 -or $bytes.Length -gt 65536) { throw 'read' }
    $encoding = New-Object System.Text.UTF8Encoding($false, $true)
    $source = $encoding.GetString($bytes)
    $dataRead = $true
    $phase = 'parser'
    $tokens = $null
    $errors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$errors)
    $parseErrors = [Math]::Min(64, $errors.Count)
    $parserComplete = $true
    $phase = 'policy'
    $observedPolicy = (Get-ExecutionPolicy).ToString()
    switch ($observedPolicy) {
        'Restricted' { $policy = 'Restricted' }
        'AllSigned' { $policy = 'AllSigned' }
        'RemoteSigned' { $policy = 'RemoteSigned' }
        'Unrestricted' { $policy = 'Unrestricted' }
        'Bypass' { $policy = 'Bypass' }
        'Undefined' { $policy = 'Undefined' }
        default { $policy = 'other' }
    }
    $phase = 'fromjson'
    $decoded = '{"value":1}' | ConvertFrom-Json
    if ($decoded.value -ne 1) { throw 'fromjson' }
    $phase = 'tojson'
    $encoded = $decoded | ConvertTo-Json -Compress
    if ($encoded -ne '{"value":1}') { throw 'tojson' }
    $initialization = $true
    $phase = 'complete'
} catch {
    # Only the fixed phase survives; no exception, source, token, path or stderr output.
}
# Reporting must work even when the JSON cmdlet under test failed.
[Console]::Write('{"schemaVersion":1,"entered":true,"psMajor":' + $major.ToString([Globalization.CultureInfo]::InvariantCulture) + ',"psMinor":' + $minor.ToString([Globalization.CultureInfo]::InvariantCulture) + ',"policy":"' + $policy + '","dataRead":' + $dataRead.ToString().ToLowerInvariant() + ',"parserComplete":' + $parserComplete.ToString().ToLowerInvariant() + ',"parseErrors":' + $parseErrors.ToString([Globalization.CultureInfo]::InvariantCulture) + ',"initialization":' + $initialization.ToString().ToLowerInvariant() + ',"phase":"' + $phase + '"}')
exit 0
