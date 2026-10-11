$ErrorActionPreference = 'Stop'
$target = __TARGET__
$source = __SOURCE__
$expectedHash = __HASH__
$oldPid = __PID__
$resultPath = __RESULT__
$version = __VERSION__
$operationId = [guid]::NewGuid().ToString()
$backup = "$target.previous"
$next = "$target.next"
$replaced = $false
$oldExited = $false
function Save-Result([string]$state, [string]$detail) {
    $record = @{ id=$operationId; status=$state; version=$version; message=$detail; finishedAt=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
    $temp = "$resultPath.tmp"
    [IO.File]::WriteAllText($temp, ($record | ConvertTo-Json -Compress), (New-Object Text.UTF8Encoding($false)))
    Move-Item -LiteralPath $temp -Destination $resultPath -Force
}
try {
    Write-Host 'Waiting for Coding Tools MCP to exit...'
    Wait-Process -Id $oldPid -Timeout 120 -ErrorAction SilentlyContinue
    if (Get-Process -Id $oldPid -ErrorAction SilentlyContinue) { throw 'Application is still running. Retry the update after closing it.' }
    $oldExited = $true
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Update checksum mismatch' }
    Copy-Item -LiteralPath $source -Destination $next -Force
    Copy-Item -LiteralPath $target -Destination $backup -Force
    Move-Item -LiteralPath $next -Destination $target -Force
    $replaced = $true
    Save-Result 'installed' 'Update installed; the restarted application verifies its version.'
    Start-Process -FilePath $target -WorkingDirectory (Split-Path $target)
    Remove-Item -LiteralPath $source -Force -ErrorAction SilentlyContinue
    Write-Host 'Update installed. Closing the installer.'
    exit 0
} catch {
    $failure = $_.Exception.Message
    $outcome = 'failed'
    if ($replaced) {
        try { Copy-Item -LiteralPath $backup -Destination $target -Force; $outcome = 'rolled_back' }
        catch { $failure += "; rollback failed: " + $_.Exception.Message; $oldExited = $false }
    }
    try { Save-Result $outcome $failure } catch { Write-Host "Unable to save update outcome: $_" }
    if ($oldExited) {
        try { Start-Process -FilePath $target -WorkingDirectory (Split-Path $target) }
        catch { Write-Host "Unable to restart application: $_" }
    }
    Write-Host $failure
    exit 1
}
