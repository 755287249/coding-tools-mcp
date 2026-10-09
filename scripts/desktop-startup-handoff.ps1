[CmdletBinding()]
param(
    [string]$NewExe,
    [string]$NewVersion,
    [string]$DataFile,
    [int]$LauncherPid,
    [switch]$LibraryOnly,
    [switch]$TemporaryWorker,
    [switch]$NoDialogs
)
$ErrorActionPreference = 'Stop'

# Pure selection: never stop a different product, Windows session, equal/newer
# build, or the launcher itself. Unreadable metadata is not proof of ownership.
function Select-UpgradeTargets {
    param([array]$Candidates, [string]$TargetPath, [version]$TargetVersion, [int]$SessionId, [int]$ExcludePid)
    foreach ($candidate in $Candidates) {
        if ($candidate.ProcessId -eq $ExcludePid -or $candidate.SessionId -ne $SessionId) { continue }
        if ($candidate.Name -notin @('ctmcp.exe', 'Coding Tools MCP.exe', 'coding-tools-mcp-desktop.exe')) { continue }
        if ($candidate.ProductName -ne 'Coding Tools MCP' -or [string]::IsNullOrWhiteSpace($candidate.ExecutablePath)) { continue }
        if ([string]::Equals($candidate.ExecutablePath, $TargetPath, [StringComparison]::OrdinalIgnoreCase)) { continue }
        $version = $null
        if (-not [version]::TryParse([string]$candidate.Version, [ref]$version)) { continue }
        if ($version -lt $TargetVersion) { $candidate }
    }
}

function Get-DesktopCandidates {
    foreach ($process in @(Get-CimInstance Win32_Process -Filter "Name='ctmcp.exe' OR Name='Coding Tools MCP.exe' OR Name='coding-tools-mcp-desktop.exe'")) {
        if (-not $process.ExecutablePath) { continue }
        try {
            $info = [Diagnostics.FileVersionInfo]::GetVersionInfo([string]$process.ExecutablePath)
            [pscustomobject]@{ ProcessId=[int]$process.ProcessId; SessionId=[int]$process.SessionId; Name=[string]$process.Name; ExecutablePath=[IO.Path]::GetFullPath([string]$process.ExecutablePath); ProductName=$info.ProductName; Version=$info.ProductVersion }
        } catch { continue }
    }
}

function Get-UpgradeSnapshot {
    param($Data)
    $mcp = @($Data.mcp_enabled_workspace_ids | Where-Object { $_ -is [string] -and $_ })
    $actions = @($Data.actions_enabled_workspace_ids | Where-Object { $_ -is [string] -and $_ })
    $endpoints = @()
    foreach ($profile in @($Data.profiles)) {
        $bind = $profile.bind
        $hostName = if ($bind.host) { [string]$bind.host } else { [string]$profile.runtime.bind_address }
        if (-not $hostName -or $hostName -eq '0.0.0.0') { $hostName = '127.0.0.1' }
        if ($hostName -eq '::') { $hostName = '[::1]' } elseif ($hostName.Contains(':') -and -not $hostName.StartsWith('[')) { $hostName = "[$hostName]" }
        $port = if ($bind.port) { [int]$bind.port } else { [int]$profile.runtime.local_port }
        if ($profile.id -in $mcp -and $port -gt 0) { $endpoints += [pscustomobject]@{kind='mcp';url="http://${hostName}:${port}/mcp/info"} }
        $config = if ($profile.host.desktop.actions) { $profile.host.desktop.actions } else { $profile.actions }
        if ($profile.id -in $actions -and [int]$config.local_port -gt 0) {
            $hostName = [string]$config.bind_address
            if (-not $hostName -or $hostName -eq '0.0.0.0') { $hostName = '127.0.0.1' }
            if ($hostName -eq '::') { $hostName = '[::1]' } elseif ($hostName.Contains(':') -and -not $hostName.StartsWith('[')) { $hostName = "[$hostName]" }
            $endpoints += [pscustomobject]@{kind='actions';url="http://${hostName}:$($config.local_port)/openapi.json"}
        }
    }
    [pscustomobject]@{mcpWorkspaceIds=$mcp;actionsWorkspaceIds=$actions;endpoints=$endpoints}
}

function Test-UpgradeHealth {
    param($Process, [array]$Endpoints, [string]$Version, [int]$TimeoutSeconds = 90)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $started = [DateTime]::UtcNow
    do {
        $Process.Refresh()
        if ($Process.HasExited) { return $false }
        $healthy = $true
        foreach ($endpoint in $Endpoints) {
            try {
                $reply = Invoke-RestMethod -Uri $endpoint.url -TimeoutSec 2 -UseBasicParsing
                $actual = if ($endpoint.kind -eq 'mcp' -and $reply.name -eq 'coding-tools-mcp') { $reply.version } elseif ($endpoint.kind -eq 'actions') { $reply.info.version } else { '' }
                if ([string]$actual -ne $Version) { $healthy = $false }
            } catch { $healthy = $false }
        }
        # Also detect immediate startup failures when no services were enabled.
        if ($healthy -and ([DateTime]::UtcNow - $started).TotalSeconds -ge 3) { return $true }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    return $false
}

if ($LibraryOnly) { return }
$mutex = [Threading.Mutex]::new($false, 'Local\CodingToolsMcpDesktop-Upgrade')
$owned = $false
$old = @()
$replacement = $null
$stoppedAny = $false
$wroteHandoff = $false
$log = $null
try {
    try { $owned = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $owned = $true }
    if (-not $owned) { return }
    $target = [IO.Path]::GetFullPath($NewExe)
    $targetVersion = [version]$NewVersion
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($target)
    if ($info.ProductName -ne 'Coding Tools MCP' -or [version]$info.ProductVersion -ne $targetVersion) { throw 'The new executable has invalid product/version metadata.' }
    $sessionId = (Get-Process -Id $PID).SessionId
    Start-Sleep -Milliseconds 400
    $old = @(Select-UpgradeTargets @(Get-DesktopCandidates) $target $targetVersion $sessionId $LauncherPid)
    if ($old.Count -eq 0) { throw 'An equal/newer instance is already running, or the running application could not be safely identified. Exit it from the tray before opening this copy.' }
    $dataDir = Split-Path -Parent $DataFile
    New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
    $log = Join-Path $dataDir 'startup-handoff.log'
    if (Test-Path -LiteralPath $DataFile) {
        try { $data = [IO.File]::ReadAllText($DataFile) | ConvertFrom-Json } catch { throw 'Saved workspace settings could not be read. The running application was not stopped.' }
    } else { $data = [pscustomobject]@{profiles=@();mcp_enabled_workspace_ids=@();actions_enabled_workspace_ids=@()} }
    $snapshot = Get-UpgradeSnapshot $data
    $handoff = Join-Path $dataDir 'runtime-handoff.json'
    if (Test-Path -LiteralPath $handoff) { throw 'A previous runtime handoff is pending. Restart the existing application first.' }
    $json = @{mcpWorkspaceIds=@($snapshot.mcpWorkspaceIds);actionsWorkspaceIds=@($snapshot.actionsWorkspaceIds)} | ConvertTo-Json -Depth 4
    $temporary = "$handoff.$PID.tmp"
    [IO.File]::WriteAllText($temporary, $json, [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $handoff
    $wroteHandoff = $true
    foreach ($candidate in $old) {
        $current = @(Get-DesktopCandidates | Where-Object { $_.ProcessId -eq $candidate.ProcessId -and $_.ExecutablePath -eq $candidate.ExecutablePath -and $_.Version -eq $candidate.Version -and $_.SessionId -eq $sessionId })
        if ($current.Count -eq 0) { continue }
        Add-Content -LiteralPath $log -Value "Replacing desktop pid=$($candidate.ProcessId) version=$($candidate.Version) with $NewVersion"
        & taskkill.exe /PID $candidate.ProcessId /T /F 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'The previous application could not be stopped. Check whether it is running as administrator.' }
        $stoppedAny = $true
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        $remaining = @(Get-DesktopCandidates | Where-Object { $_.ProcessId -in $old.ProcessId -and $_.ExecutablePath -in $old.ExecutablePath })
        if ($remaining.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($remaining.Count -gt 0) { throw 'The old application did not exit in time.' }
    $replacement = Start-Process -FilePath $target -WorkingDirectory (Split-Path -Parent $target) -ArgumentList '--handoff-child' -PassThru
    if (-not (Test-UpgradeHealth $replacement @($snapshot.endpoints) $NewVersion)) { throw 'The new application did not restore its saved services.' }
    Add-Content -LiteralPath $log -Value "Ready: desktop $NewVersion"
} catch {
    $failure = $_.Exception.Message
    if ($log) { Add-Content -LiteralPath $log -Value "Failed: $failure" }
    if ($replacement -and -not $replacement.HasExited) { & taskkill.exe /PID $replacement.Id /T /F 2>$null | Out-Null }
    if ($stoppedAny -and $old.Count -gt 0 -and (Test-Path -LiteralPath $old[0].ExecutablePath)) {
        try {
            Start-Process -FilePath $old[0].ExecutablePath -WorkingDirectory (Split-Path -Parent $old[0].ExecutablePath) | Out-Null
            $failure += "`nThe previous executable was launched again. Check its tray status."
        } catch { $failure += "`nRollback could not start. Open the previous executable manually." }
    } elseif ($wroteHandoff -and (Test-Path -LiteralPath $handoff)) {
        # No application was stopped, so do not leave a stale restore request.
        Remove-Item -LiteralPath $handoff -Force
    }
    if ($NoDialogs) { Write-Output $failure } else {
        Add-Type -AssemblyName System.Windows.Forms
        [System.Windows.Forms.MessageBox]::Show($failure, 'Coding Tools MCP update') | Out-Null
    }
    exit 1
} finally {
    if ($owned) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
    if ($TemporaryWorker -and $PSCommandPath -and (Split-Path -Leaf (Split-Path -Parent $PSCommandPath)) -eq 'handoff-workers') { Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue }
}
