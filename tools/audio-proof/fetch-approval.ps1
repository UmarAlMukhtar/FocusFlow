# Embedded read-only transport. No CLI parameters or remote executable content.
$script:phase = 'init'
$script:failure = 'initialization'
$script:httpStatus = $null
function Set-HttpStatus($exception) {
    # Diagnostic inspection only: nested 404s must not alter the existing Pending catch.
    for ($depth = 0; $null -ne $exception -and $depth -lt 8; $depth++) {
        if ($exception -is [Net.WebException] -and $exception.Response -is [Net.HttpWebResponse]) {
            $status = [int]$exception.Response.StatusCode
            if ($status -ge 100 -and $status -le 599) { $script:httpStatus = $status; return }
        }
        $exception = $exception.InnerException
    }
}
try {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Console]::InputEncoding = New-Object System.Text.UTF8Encoding($false, $true)
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false, $true)
    $script:phase = 'gate'
    $script:failure = 'input'
    # Parent sends this bounded line only after exact Job assignment/query succeeds.
    $builder = New-Object System.Text.StringBuilder
    while ($true) {
        $c = [Console]::Read()
        if ($c -lt 0) { throw 'Gate EOF' }
        if ($c -eq 10) { break }
        if ($builder.Length -ge 4095) { throw 'Gate cap' }
        [void]$builder.Append([char]$c)
    }
    $gate = $builder.ToString() | ConvertFrom-Json
    $request = $gate.request
    $script:selectedCommit = ''
    if ($null -ne $gate.previous_commit -and $gate.previous_commit -cnotmatch '^[0-9a-f]{40}$') { throw 'Cache shape' }
    function Pending {
        [Console]::Write((@{schema_version=1; pending_commit=$script:selectedCommit} | ConvertTo-Json -Compress))
        exit 75
    }
    $script:failure = 'shape'
    if ($request.schema_version -ne 1 -or $request.repository -cne 'UmarAlMukhtar/FocusFlow' -or
        $request.source_ref -cne 'refs/heads/codex-audio-native-qualification' -or
        $request.request_id -cnotmatch '^[0-9a-f]{32}$' -or $request.run_id -le 0 -or $request.attempt -le 0) { throw 'Gate shape' }
    $script:phase = 'ref'
    $script:failure = 'transport'
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    function Get-FixedJson([string]$path, [bool]$pending) {
        $script:failure = 'transport'
        $script:httpStatus = $null
        $url = 'https://api.github.com/repos/UmarAlMukhtar/FocusFlow/' + $path
        $http = [Net.HttpWebRequest]::Create($url)
        $http.AllowAutoRedirect = $false
        $http.Timeout = 10000
        $http.ReadWriteTimeout = 10000
        $http.UserAgent = 'FocusFlow-private-audio-admission-v1'
        $http.Accept = 'application/vnd.github+json'
        $http.Headers['X-GitHub-Api-Version'] = '2022-11-28'
        # Public repository access needs no credential. No inherited token is exposed.
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        try {
            $async = $http.BeginGetResponse($null, $null)
            try {
                if (-not $async.AsyncWaitHandle.WaitOne(10000)) { $http.Abort(); throw 'HTTPS response deadline' }
                $response = $http.EndGetResponse($async)
            } finally { $async.AsyncWaitHandle.Close() }
        }
        catch [Net.WebException] {
            if ($pending -and $_.Exception.Response -and [int]$_.Exception.Response.StatusCode -eq 404) { Pending }
            Set-HttpStatus $_.Exception
            throw 'HTTPS request failed'
        }
        try {
            $script:failure = 'response'
            $status = [int]$response.StatusCode
            if ($status -ge 100 -and $status -le 599) { $script:httpStatus = $status }
            if ([int]$response.StatusCode -ne 200 -or $response.ResponseUri.Scheme -cne 'https' -or $response.ResponseUri.Host -cne 'api.github.com') { throw 'HTTPS origin/status' }
            $stream = $response.GetResponseStream()
            $buffer = New-Object byte[] 65536
            $memory = New-Object IO.MemoryStream
            try {
                while ($true) {
                    if ([DateTime]::UtcNow -ge $deadline) { throw 'HTTPS body deadline' }
                    $stream.ReadTimeout = [Math]::Max(1, [int]($deadline - [DateTime]::UtcNow).TotalMilliseconds)
                    $n = $stream.Read($buffer, 0, $buffer.Length)
                    if ($n -eq 0) { break }
                    if ($memory.Length + $n -gt 524288) { throw 'HTTPS body cap' }
                    $memory.Write($buffer, 0, $n)
                }
                $script:failure = 'shape'
                return ([Text.Encoding]::UTF8.GetString($memory.ToArray()) | ConvertFrom-Json)
            } finally { $memory.Dispose(); $stream.Dispose() }
        } finally { $response.Dispose() }
    }
    $tip = Get-FixedJson 'git/ref/heads/codex-audio-native-approvals' $true
    $script:failure = 'shape'
    $commit = [string]$tip.object.sha
    if ($tip.object.type -cne 'commit' -or $commit -cnotmatch '^[0-9a-f]{40}$') { throw 'Ref is not a commit' }
    $script:selectedCommit = $commit
    if ($gate.previous_commit -ceq $commit) { Pending }
    $base = 'approvals/' + [string][uint64]$request.run_id + '/' + [string][uint32]$request.attempt + '/DiscoverRender/' + $request.request_id + '/'
    function Get-ExactFile([string]$name) {
        $record = Get-FixedJson ('contents/' + $base + $name + '?ref=' + $commit) $true
        $script:failure = 'shape'
        if ($record.type -cne 'file' -or $record.encoding -cne 'base64' -or $record.sha -cnotmatch '^[0-9a-f]{40}$' -or $record.path -cne ($base + $name) -or $record.size -gt 262144 -or $record.size -le 0) { throw 'Blob shape' }
        $bytes = [Convert]::FromBase64String($record.content)
        if ($bytes.Length -ne $record.size) { throw 'Blob byte count' }
        return @{sha=$record.sha; text=(New-Object Text.UTF8Encoding($false,$true)).GetString($bytes)}
    }
    $script:phase = 'manifest'
    $manifest = Get-ExactFile 'manifest.json'
    $script:phase = 'request'
    $publication = Get-ExactFile 'request.json'
    $script:phase = 'reply'
    $script:failure = 'output'
    # Preserve publication JSON as raw text too; Rust rejects duplicate/unknown fields.
    $reply = @{schema_version=1; commit=$commit; blob=$manifest.sha; publication_blob=$publication.sha; manifest=$manifest.text; publication_raw=$publication.text}
    $json = $reply | ConvertTo-Json -Depth 4 -Compress
    if ([Text.Encoding]::UTF8.GetByteCount($json) -gt 524288) { throw 'Output cap' }
    [Console]::Write($json)
    exit 0
} catch {
    # Only fixed tags and an observed HTTP status leave this failure path.
    Set-HttpStatus $_.Exception
    $diagnostic = @{schemaVersion=1; phase=$script:phase; failure=$script:failure; httpStatus=$script:httpStatus}
    $json = $diagnostic | ConvertTo-Json -Compress
    if ([Text.Encoding]::UTF8.GetByteCount($json) -le 1024) { [Console]::Write($json) }
    exit 71
}
