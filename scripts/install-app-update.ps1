$ErrorActionPreference = 'Stop'
$target = __TARGET__
$source = __SOURCE__
$expectedHash = __HASH__
$oldPid = __PID__
$backup = "$target.previous"
$next = "$target.next"
try {
    Write-Host 'Waiting for Coding Tools MCP to exit...'
    Wait-Process -Id $oldPid -Timeout 120 -ErrorAction SilentlyContinue
    if (Get-Process -Id $oldPid -ErrorAction SilentlyContinue) { throw 'Application is still running. Retry the update after closing it.' }
    if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Update checksum mismatch' }
    Copy-Item -LiteralPath $source -Destination $next -Force
    Copy-Item -LiteralPath $target -Destination $backup -Force
    try {
        Move-Item -LiteralPath $next -Destination $target -Force
        Start-Process -FilePath $target -WorkingDirectory (Split-Path $target)
    } catch {
        Copy-Item -LiteralPath $backup -Destination $target -Force
        Start-Process -FilePath $target -WorkingDirectory (Split-Path $target)
        throw
    }
    Remove-Item -LiteralPath $source -Force
    Write-Host 'Update installed. The previous executable is retained next to the application.'
} catch {
    Write-Error $_
    exit 1
}
