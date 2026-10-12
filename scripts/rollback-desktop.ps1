[CmdletBinding()]
param(
    # Path of the older desktop executable to start, e.g. C:\Downloads\ctmcp-0.1.141-win64.exe
    [Parameter(Mandatory = $true)][string]$TargetExe,
    # Restore the settings saved right before the 0.1.142 global connection migration.
    [switch]$RestorePreMigrationSettings,
    # Desktop data folder. Default: %APPDATA%\coding-tools-mcp-desktop\data
    [string]$DataDir = '',
    # Shared workspace mirrors. Default: %LOCALAPPDATA%\CodingToolsMCP\workspaces
    [string]$SharedWorkspacesDir = '',
    # Load the functions only (used by tests).
    [switch]$LibraryOnly
)
$ErrorActionPreference = 'Stop'

# Roll the Coding Tools MCP desktop back to an older executable.
#
# Why this exists: every desktop since 0.1.67 refuses to replace a NEWER
# running instance (single-instance lock + startup handoff), and 0.1.142
# migrates settings one way to a "global MCP connection". Steps:
#   1. Stop every running Coding Tools MCP desktop in this Windows session.
#   2. Optionally restore data\pre-global-connection\ (written by 0.1.143+
#      right before the migration). The current files are kept in
#      data\before-rollback-<timestamp>\ first.
#   3. Start the requested executable.

function Test-DesktopName([string]$Name) {
    return $Name -match '^(ctmcp(?:-\d+\.\d+\.\d+-win64)?(?: \(\d+\))?|Coding Tools MCP|coding-tools-mcp-desktop)\.exe$'
}

function Test-SafeWorkspaceId([string]$Id) {
    return $Id -match '^[A-Za-z0-9_-]+$'
}

function Get-RunningDesktops([int]$SessionId) {
    foreach ($process in @(Get-CimInstance Win32_Process -Filter "Name LIKE 'ctmcp%.exe' OR Name='Coding Tools MCP.exe' OR Name='coding-tools-mcp-desktop.exe'")) {
        if (-not (Test-DesktopName $process.Name)) { continue }
        if ([int]$process.SessionId -ne $SessionId) { continue }
        if (-not $process.ExecutablePath) { continue }
        try { $info = [Diagnostics.FileVersionInfo]::GetVersionInfo([string]$process.ExecutablePath) } catch { continue }
        if ($info.ProductName -ne 'Coding Tools MCP') { continue }
        [pscustomobject]@{ ProcessId = [int]$process.ProcessId; Path = [string]$process.ExecutablePath; Version = [string]$info.ProductVersion }
    }
}

function Wait-DesktopMutexReleased([int]$TimeoutSeconds = 20) {
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $existing = $null
        try { $existing = [Threading.Mutex]::OpenExisting('Local\CodingToolsMcpDesktop-SingleInstance') }
        catch [Threading.WaitHandleCannotBeOpenedException] { return $true }
        finally { if ($existing) { $existing.Dispose() } }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    return $false
}

# Pure file plan: which backup files go where. No file is touched here.
function Get-RestorePlan([string]$BackupDir, [string]$DataDir, [string]$SharedWorkspacesDir) {
    $plan = @()
    $profiles = Join-Path $BackupDir 'profiles.json'
    if (-not (Test-Path -LiteralPath $profiles -PathType Leaf)) {
        throw "No pre-migration backup at $profiles. It is written only by 0.1.143 or later, before the first global connection migration."
    }
    $plan += [pscustomobject]@{ Source = $profiles; Destination = (Join-Path $DataDir 'profiles.json') }
    $workspaces = Join-Path $BackupDir 'workspaces'
    if (Test-Path -LiteralPath $workspaces -PathType Container) {
        foreach ($dir in @(Get-ChildItem -LiteralPath $workspaces -Directory)) {
            if (-not (Test-SafeWorkspaceId $dir.Name)) { continue }
            foreach ($file in @(Get-ChildItem -LiteralPath $dir.FullName -File)) {
                if ($file.Name -notin @('workspace.json', 'secrets.json')) { continue }
                $plan += [pscustomobject]@{ Source = $file.FullName; Destination = (Join-Path (Join-Path $SharedWorkspacesDir $dir.Name) $file.Name) }
            }
        }
    }
    return $plan
}

function Invoke-RestorePlan([array]$Plan, [string]$KeepDir) {
    New-Item -ItemType Directory -Path $KeepDir -Force | Out-Null
    $index = 0
    foreach ($item in $Plan) {
        $index++
        if (Test-Path -LiteralPath $item.Destination -PathType Leaf) {
            $keep = Join-Path $KeepDir ("{0:D3}-{1}" -f $index, (Split-Path -Leaf $item.Destination))
            Copy-Item -LiteralPath $item.Destination -Destination $keep -Force
            Add-Content -LiteralPath (Join-Path $KeepDir 'index.txt') -Value "$keep <= $($item.Destination)"
        }
        New-Item -ItemType Directory -Path (Split-Path -Parent $item.Destination) -Force | Out-Null
        Copy-Item -LiteralPath $item.Source -Destination $item.Destination -Force
    }
}

if ($LibraryOnly) { return }

if (-not $DataDir) { $DataDir = Join-Path $env:APPDATA 'coding-tools-mcp-desktop\data' }
if (-not $SharedWorkspacesDir) {
    $SharedWorkspacesDir = if ($env:CTMCP_SHARED_WORKSPACES_ROOT) { $env:CTMCP_SHARED_WORKSPACES_ROOT } else { Join-Path $env:LOCALAPPDATA 'CodingToolsMCP\workspaces' }
}

$target = [IO.Path]::GetFullPath($TargetExe)
if (-not (Test-Path -LiteralPath $target -PathType Leaf)) { throw "Executable not found: $target" }
$info = [Diagnostics.FileVersionInfo]::GetVersionInfo($target)
if ($info.ProductName -ne 'Coding Tools MCP') { throw "Not a Coding Tools MCP executable: $target" }
Write-Output "Target: $target (version $($info.ProductVersion))"

$sessionId = (Get-Process -Id $PID).SessionId
$running = @(Get-RunningDesktops $sessionId)
foreach ($desktop in $running) {
    Write-Output "Stopping running desktop pid=$($desktop.ProcessId) version=$($desktop.Version) path=$($desktop.Path)"
    # /T also stops the desktop's own children (cloudflared, frpc, MCP helpers).
    & "$env:SystemRoot\System32\taskkill.exe" /PID $desktop.ProcessId /T /F | Out-Null
}
if (-not (Wait-DesktopMutexReleased)) { throw 'A Coding Tools MCP desktop still holds the single-instance lock. Exit it from the tray and run this script again.' }

if ($RestorePreMigrationSettings) {
    $backup = Join-Path $DataDir 'pre-global-connection'
    $plan = @(Get-RestorePlan $backup $DataDir $SharedWorkspacesDir)
    $keep = Join-Path $DataDir ("before-rollback-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    Invoke-RestorePlan $plan $keep
    Write-Output "Restored $($plan.Count) file(s) from $backup. The replaced files are kept in $keep."
}

Start-Process -FilePath $target -WorkingDirectory (Split-Path -Parent $target) -WindowStyle Normal | Out-Null
Write-Output 'Started. If MCP or the tunnel is off, enable it again in the older version.'
