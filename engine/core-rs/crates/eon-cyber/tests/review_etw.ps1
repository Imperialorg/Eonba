# Native Windows acceptance. Only enable EON's payload-free provider in a
# disposable, bounded ETL session. Requires permission to start an ETW trace.
param([Parameter(Mandatory)][string]$AuditExe)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Requires Windows and PowerShell 7.' }
$exe = (Get-Item -LiteralPath $AuditExe).FullName
$provider = [guid]'87a9f157-e3c2-4589-93a0-299a3d325e72'
$session = 'eon-audit-' + [guid]::NewGuid().ToString('N')
$scratch = (New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) $session)).FullName
$etl = Join-Path $scratch 'review.etl'
$children = [Collections.Generic.List[Diagnostics.Process]]::new()

function Invoke-Review([bool]$optIn) {
    $start = [Diagnostics.ProcessStartInfo]::new($exe)
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.ArgumentList.Add('rust')
    if ($optIn) { $start.ArgumentList.Add('--etw') }
    $child = [Diagnostics.Process]::Start($start)
    # Keep both process handles until after readback. Windows cannot reuse
    # either PID while its process object is retained.
    $children.Add($child)
    try {
        $child.StandardInput.WriteLine('fn example() {}')
        $child.StandardInput.Close()
        $reply = $child.StandardOutput.ReadToEndAsync()
        if (-not $child.WaitForExit(10000)) { throw 'Audit CLI timed out.' }
        if ($child.ExitCode -ne 0) { throw 'Audit CLI failed.' }
        $report = $reply.GetAwaiter().GetResult() | ConvertFrom-Json
        if ($optIn -and $report.etw.status -ne 'submitted') { throw 'Provider not enabled.' }
        if (-not $optIn -and $report.PSObject.Properties['etw']) { throw 'Unexpected opt-in.' }
        return $child.Id
    } finally {
        if (-not $child.HasExited) { $child.Kill(); $child.WaitForExit() }
    }
}

function Assert-EmptyPayload([xml]$xml) {
    if ($xml.DocumentElement.LocalName -ne 'Event' -or
        $xml.SelectSingleNode("//*[local-name()='ProcessingErrorData']")) {
        throw 'Cannot establish payload absence on an undecoded ETW event.'
    }
    $data = $xml.SelectNodes("/*[local-name()='Event']/*[local-name()='EventData' or local-name()='UserData' or local-name()='BinaryEventData']")
    foreach ($node in $data) {
        if ($node.InnerText.Length -gt 0) { throw 'Review marker must not contain a payload.' }
    }
}

$started = $false
try {
    logman create trace $session -p $provider.ToString('B') 0x1 4 -o $etl -max 4 -ets
    if ($LASTEXITCODE -ne 0) { throw 'Cannot start private acceptance trace.' }
    $started = $true
    $without = Invoke-Review $false
    $with = Invoke-Review $true
    logman stop $session -ets
    if ($LASTEXITCODE -ne 0) { throw 'Cannot flush acceptance trace.' }
    $started = $false
    $markers = @(Get-WinEvent -Path $etl -Oldest | Where-Object {
        $_.ProviderId -eq $provider -and $_.ProcessId -in $with, $without
    })
    if ($markers.Count -ne 1 -or $markers[0].Id -ne 1 -or $markers[0].Version -ne 1 -or
        $markers[0].ProcessId -ne $with -or $markers[0].ProcessId -eq $without) {
        throw 'Expected exactly the opted-in process completion in the ETL.'
    }
    Assert-EmptyPayload ([xml]($markers[0].ToXml()))
    Write-Output 'PASS: independently read one opted-in review completion from ETW.'
} finally {
    if ($started) { logman stop $session -ets }
    foreach ($child in $children) { $child.Dispose() }
    Remove-Item -LiteralPath $scratch -Recurse -Force
}
