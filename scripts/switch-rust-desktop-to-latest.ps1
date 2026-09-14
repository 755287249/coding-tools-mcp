[CmdletBinding()]
param(
    [string]$PackageZip = '',
    [string]$ExpectedVersion = '',
    [string]$DataFile = '',
    [string]$StagedExe = '',
    [int]$DelaySeconds = 2,
    [int]$HealthTimeoutSeconds = 90,
    [string]$LogPath = '',
    [string]$ResultPath = '',
    [switch]$Worker,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Resolve-StableWorkspaceRoot {
    param([Parameter(Mandatory = $true)][string]$CurrentWorkspace)

    try {
        $gitCommonDir = (& git -C $CurrentWorkspace rev-parse --path-format=absolute --git-common-dir 2>$null | Select-Object -First 1)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace([string]$gitCommonDir)) {
            $resolvedCommonDir = [System.IO.Path]::GetFullPath(([string]$gitCommonDir).Trim())
            if ((Split-Path -Leaf $resolvedCommonDir) -eq '.git') {
                return (Split-Path -Parent $resolvedCommonDir)
            }
        }
    } catch {}

    return $CurrentWorkspace
}

$stableWorkspace = Resolve-StableWorkspaceRoot -CurrentWorkspace $workspace

function Write-Log {
    param([string]$Message)
    $line = "[$([DateTime]::Now.ToString('s'))] $Message"
    Write-Host $line
    if (-not [string]::IsNullOrWhiteSpace($LogPath)) {
        $parent = Split-Path -Parent $LogPath
        if ($parent) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        Add-Content -LiteralPath $LogPath -Value $line -Encoding UTF8
    }
}

function Quote-CommandLineArg {
    param([string]$Value)
    return '"' + ($Value -replace '"', '\"') + '"'
}

function Get-ProfileHost {
    param([string]$BindAddress)
    if ([string]::IsNullOrWhiteSpace($BindAddress) -or $BindAddress -eq '0.0.0.0') { return '127.0.0.1' }
    if ($BindAddress -eq '::') { return '[::1]' }
    if ($BindAddress.Contains(':')) { return "[$BindAddress]" }
    return $BindAddress
}

function Get-ProfileMcpBindAddress {
    param($Profile)
    if ($null -ne $Profile.bind -and -not [string]::IsNullOrWhiteSpace([string]$Profile.bind.host)) {
        return [string]$Profile.bind.host
    }
    return [string]$Profile.runtime.bind_address
}

function Get-ProfileMcpPort {
    param($Profile)
    if ($null -ne $Profile.bind -and $null -ne $Profile.bind.port) {
        $canonicalPort = [int]$Profile.bind.port
        if ($canonicalPort -gt 0) { return $canonicalPort }
    }
    return [int]$Profile.runtime.local_port
}

function Get-ProfileActionsConfig {
    param($Profile)
    if ($null -ne $Profile.host -and $null -ne $Profile.host.desktop -and $null -ne $Profile.host.desktop.actions) {
        return $Profile.host.desktop.actions
    }
    return $Profile.actions
}
function Invoke-JsonProbe {
    param([string]$Url)
    try {
        return Invoke-RestMethod -Uri $Url -Method Get -TimeoutSec 2 -UseBasicParsing
    } catch {
        return $null
    }
}

function Get-RuntimeSnapshot {
    param([string]$Path)

    $mcpIds = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $actionsIds = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $endpoints = @()
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return [pscustomobject]@{ mcpWorkspaceIds = @(); actionsWorkspaceIds = @(); endpoints = @() }
    }

    $data = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8) | ConvertFrom-Json
    foreach ($id in @($data.mcp_enabled_workspace_ids)) {
        if (-not [string]::IsNullOrWhiteSpace([string]$id)) { [void]$mcpIds.Add([string]$id) }
    }
    foreach ($id in @($data.actions_enabled_workspace_ids)) {
        if (-not [string]::IsNullOrWhiteSpace([string]$id)) { [void]$actionsIds.Add([string]$id) }
    }

    foreach ($profile in @($data.profiles)) {
        $id = [string]$profile.id
        if ([string]::IsNullOrWhiteSpace($id)) { continue }

        $mcpHost = Get-ProfileHost (Get-ProfileMcpBindAddress $profile)
        $mcpPort = Get-ProfileMcpPort $profile
        if ($mcpPort -gt 0) {
            $url = "http://${mcpHost}:${mcpPort}/mcp/info"
            $payload = Invoke-JsonProbe $url
            if ($null -ne $payload -and [string]$payload.name -eq 'coding-tools-mcp') { [void]$mcpIds.Add($id) }
        }

        $actions = Get-ProfileActionsConfig $profile
        $actionsHost = Get-ProfileHost ([string]$actions.bind_address)
        $actionsPort = [int]$actions.local_port
        if ($actionsPort -gt 0) {
            $url = "http://${actionsHost}:${actionsPort}/openapi.json"
            $payload = Invoke-JsonProbe $url
            if ($null -ne $payload -and $null -ne $payload.info -and -not [string]::IsNullOrWhiteSpace([string]$payload.info.version)) {
                [void]$actionsIds.Add($id)
            }
        }
    }

    foreach ($profile in @($data.profiles)) {
        $id = [string]$profile.id
        if ($mcpIds.Contains($id)) {
            $hostName = Get-ProfileHost (Get-ProfileMcpBindAddress $profile)
            $mcpPort = Get-ProfileMcpPort $profile
            $endpoints += [pscustomobject]@{ kind = 'mcp'; workspaceId = $id; url = "http://${hostName}:${mcpPort}/mcp/info" }
        }
        if ($actionsIds.Contains($id)) {
            $actions = Get-ProfileActionsConfig $profile
            $hostName = Get-ProfileHost ([string]$actions.bind_address)
            $actionsPort = [int]$actions.local_port
            $endpoints += [pscustomobject]@{ kind = 'actions'; workspaceId = $id; url = "http://${hostName}:${actionsPort}/openapi.json" }
        }
    }

    return [pscustomobject]@{
        mcpWorkspaceIds = @($mcpIds)
        actionsWorkspaceIds = @($actionsIds)
        endpoints = @($endpoints)
    }
}

function Test-EndpointVersion {
    param($Endpoint, [string]$Version)
    $payload = Invoke-JsonProbe ([string]$Endpoint.url)
    if ($null -eq $payload) { return $false }
    if ([string]$Endpoint.kind -eq 'mcp') {
        if ([string]$payload.name -ne 'coding-tools-mcp') { return $false }
        return [string]::IsNullOrWhiteSpace($Version) -or [string]$payload.version -eq $Version
    }
    if ($null -eq $payload.info) { return $false }
    return [string]::IsNullOrWhiteSpace($Version) -or [string]$payload.info.version -eq $Version
}

function Wait-AllEndpoints {
    param([array]$Endpoints, [string]$Version, [int]$TimeoutSeconds)
    if ($Endpoints.Count -eq 0) { return $true }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $pending = @($Endpoints)
    while ([DateTime]::UtcNow -lt $deadline) {
        $next = @()
        foreach ($endpoint in $pending) {
            if (-not (Test-EndpointVersion $endpoint $Version)) { $next += $endpoint }
        }
        if ($next.Count -eq 0) { return $true }
        $pending = $next
        Start-Sleep -Milliseconds 250
    }
    foreach ($endpoint in $pending) {
        Write-Log "Health timeout: $($endpoint.kind) workspace=$($endpoint.workspaceId) url=$($endpoint.url) expected=$Version"
    }
    return $false
}

function Get-DesktopProcesses {
    Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('ctmcp.exe', 'Coding Tools MCP.exe', 'coding-tools-mcp-desktop.exe') }
}

function Stop-ProcessTree {
    param([int]$ProcessId)
    & taskkill.exe /PID $ProcessId /T /F 2>$null | Out-Null
}

function Start-Desktop {
    param([string]$ExePath)
    Start-Process -FilePath $ExePath -WorkingDirectory (Split-Path -Parent $ExePath) | Out-Null
}

function Test-StagedProcess {
    param([string]$ExePath, [int]$TimeoutSeconds = 10)
    $target = [System.IO.Path]::GetFullPath($ExePath)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $running = Get-DesktopProcesses | Where-Object {
            $_.ExecutablePath -and [System.IO.Path]::GetFullPath([string]$_.ExecutablePath) -eq $target
        }
        if ($running) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

if ([string]::IsNullOrWhiteSpace($ExpectedVersion)) {
    $package = Get-Content -Raw -LiteralPath (Join-Path $workspace 'package.json') | ConvertFrom-Json
    $ExpectedVersion = [string]$package.version
}
if ([string]::IsNullOrWhiteSpace($PackageZip)) {
    $PackageZip = Join-Path $workspace "dist-portable\ctmcp-${ExpectedVersion}-win64.zip"
}
if ([string]::IsNullOrWhiteSpace($DataFile)) {
    $DataFile = Join-Path $env:APPDATA 'coding-tools-mcp-desktop\data\profiles.json'
}
$dataDir = Split-Path -Parent $DataFile
if ([string]::IsNullOrWhiteSpace($LogPath)) { $LogPath = Join-Path $dataDir 'update-handoff.log' }
if ([string]::IsNullOrWhiteSpace($ResultPath)) { $ResultPath = Join-Path $dataDir 'update-handoff-result.json' }

if (-not $Worker) {
    if (-not (Test-Path -LiteralPath $PackageZip -PathType Leaf)) { throw "Rust portable package not found: $PackageZip" }
    $stream = [System.IO.File]::OpenRead($PackageZip)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $zipHash = ([System.BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
        $stream.Dispose()
    }
    $stageRoot = Join-Path $env:LOCALAPPDATA "CTMCP\u\${ExpectedVersion}-$($zipHash.Substring(0, 8))"
    if (-not (Test-Path -LiteralPath $stageRoot -PathType Container)) {
        $tempStage = "$stageRoot.next-$([guid]::NewGuid().ToString('N'))"
        New-Item -ItemType Directory -Path $tempStage -Force | Out-Null
        try {
            Expand-Archive -LiteralPath $PackageZip -DestinationPath $tempStage -Force
            New-Item -ItemType Directory -Path (Split-Path -Parent $stageRoot) -Force | Out-Null
            Move-Item -LiteralPath $tempStage -Destination $stageRoot
        } finally {
            if (Test-Path -LiteralPath $tempStage) { Remove-Item -LiteralPath $tempStage -Recurse -Force }
        }
    }
    $exeCandidates = @(Get-ChildItem -LiteralPath $stageRoot -Filter 'ctmcp.exe' -File -Recurse)
    if ($exeCandidates.Count -ne 1) { throw "Expected exactly one ctmcp.exe in staged package, found $($exeCandidates.Count)." }
    $StagedExe = $exeCandidates[0].FullName
    $fileVersion = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($StagedExe).ProductVersion
    if (-not [string]::IsNullOrWhiteSpace($fileVersion) -and $fileVersion -match '(\d+\.\d+\.\d+)') {
        $artifactVersion = $Matches[1]
        if ($artifactVersion -ne $ExpectedVersion) {
            throw "Staged Rust desktop version mismatch: expected $ExpectedVersion, executable reports $artifactVersion."
        }
    }
    Write-Log "Staged Rust desktop $ExpectedVersion before stopping the current process: $StagedExe"

    if ($DryRun) {
        $snapshot = Get-RuntimeSnapshot $DataFile
        Write-Log "Dry run: MCP=$($snapshot.mcpWorkspaceIds.Count), Actions=$($snapshot.actionsWorkspaceIds.Count); no process will be stopped."
        return
    }

    $args = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Quote-CommandLineArg $PSCommandPath),
        '-Worker', '-PackageZip', (Quote-CommandLineArg $PackageZip),
        '-ExpectedVersion', (Quote-CommandLineArg $ExpectedVersion),
        '-DataFile', (Quote-CommandLineArg $DataFile),
        '-StagedExe', (Quote-CommandLineArg $StagedExe),
        '-DelaySeconds', $DelaySeconds,
        '-HealthTimeoutSeconds', $HealthTimeoutSeconds,
        '-LogPath', (Quote-CommandLineArg $LogPath),
        '-ResultPath', (Quote-CommandLineArg $ResultPath)
    )
    $commandLine = 'powershell.exe ' + ($args -join ' ')
    $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $commandLine }
    if ($created.ReturnValue -ne 0) { throw "Failed to start detached Rust handoff worker: $($created.ReturnValue)" }
    Write-Log "Rust desktop handoff worker scheduled (pid=$($created.ProcessId))."
    return
}

if ([string]::IsNullOrWhiteSpace($StagedExe) -or -not (Test-Path -LiteralPath $StagedExe -PathType Leaf)) {
    throw "Staged Rust desktop executable is missing: $StagedExe"
}

Start-Sleep -Seconds ([Math]::Max(0, $DelaySeconds))
$snapshot = Get-RuntimeSnapshot $DataFile
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
$snapshotPath = Join-Path $dataDir 'runtime-handoff.json'
$snapshotJson = [pscustomobject]@{
    mcpWorkspaceIds = @($snapshot.mcpWorkspaceIds)
    actionsWorkspaceIds = @($snapshot.actionsWorkspaceIds)
} | ConvertTo-Json -Depth 4
[System.IO.File]::WriteAllText($snapshotPath, $snapshotJson, [System.Text.UTF8Encoding]::new($false))

$stagedFullPath = [System.IO.Path]::GetFullPath($StagedExe)
$oldProcesses = @(Get-DesktopProcesses | Where-Object {
    -not $_.ExecutablePath -or [System.IO.Path]::GetFullPath([string]$_.ExecutablePath) -ne $stagedFullPath
})
$oldExecutables = @($oldProcesses | ForEach-Object { [string]$_.ExecutablePath } | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } | Select-Object -Unique)

$result = [ordered]@{
    ok = $false
    rolledBack = $false
    completedAt = $null
    desktopVersion = $ExpectedVersion
    packageZip = $PackageZip
    stagedExe = $StagedExe
    mcpWorkspaceIds = @($snapshot.mcpWorkspaceIds)
    actionsWorkspaceIds = @($snapshot.actionsWorkspaceIds)
    error = $null
}

try {
    foreach ($process in $oldProcesses) {
        Write-Log "Stopping old Rust desktop pid=$($process.ProcessId) path=$($process.ExecutablePath)"
        Stop-ProcessTree ([int]$process.ProcessId)
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ((Get-DesktopProcesses | Where-Object { $_.ProcessId -in $oldProcesses.ProcessId }) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 250

    Write-Log "Starting replacement Rust desktop $ExpectedVersion"
    Start-Desktop $StagedExe
    if (-not (Test-StagedProcess $StagedExe 10)) {
        throw 'Replacement Rust desktop exited before it could take over the single-instance mutex.'
    }
    if (-not (Wait-AllEndpoints @($snapshot.endpoints) $ExpectedVersion $HealthTimeoutSeconds)) {
        throw "Replacement Rust desktop did not restore all previously enabled services as version $ExpectedVersion."
    }

    # Keep the stable portable entry point current, but only after the new
    # process is healthy. Directory replacement therefore does not lengthen the
    # service interruption window or compromise rollback during handoff.
    $canonicalDir = Join-Path $stableWorkspace 'dist-portable\ctmcp-win64'
    $canonicalNext = "$canonicalDir.next-$([guid]::NewGuid().ToString('N'))"
    $canonicalPrevious = "$canonicalDir.previous-$([guid]::NewGuid().ToString('N'))"
    $canonicalMovedAside = $false
    try {
        Copy-Item -LiteralPath (Split-Path -Parent $StagedExe) -Destination $canonicalNext -Recurse
        if (Test-Path -LiteralPath $canonicalDir) {
            Move-Item -LiteralPath $canonicalDir -Destination $canonicalPrevious
            $canonicalMovedAside = $true
        }
        try {
            Move-Item -LiteralPath $canonicalNext -Destination $canonicalDir
        } catch {
            if ($canonicalMovedAside -and (Test-Path -LiteralPath $canonicalPrevious) -and -not (Test-Path -LiteralPath $canonicalDir)) {
                Move-Item -LiteralPath $canonicalPrevious -Destination $canonicalDir
                $canonicalMovedAside = $false
            }
            throw
        }
        if ($canonicalMovedAside -and (Test-Path -LiteralPath $canonicalPrevious)) {
            Remove-Item -LiteralPath $canonicalPrevious -Recurse -Force -ErrorAction SilentlyContinue
            $canonicalMovedAside = Test-Path -LiteralPath $canonicalPrevious
        }
        Write-Log "Canonical portable folder updated: $canonicalDir"
    } catch {
        if ($canonicalMovedAside -and (Test-Path -LiteralPath $canonicalPrevious) -and -not (Test-Path -LiteralPath $canonicalDir)) {
            try {
                Move-Item -LiteralPath $canonicalPrevious -Destination $canonicalDir
                $canonicalMovedAside = $false
                Write-Log "Canonical portable folder restored after refresh failure: $canonicalDir"
            } catch {
                Write-Log "Warning: canonical portable restore failed; previous folder retained at $canonicalPrevious"
            }
        }
        Write-Log "Warning: replacement is healthy, but canonical portable folder could not be refreshed: $($_.Exception.Message)"
    } finally {
        if (Test-Path -LiteralPath $canonicalNext) {
            Remove-Item -LiteralPath $canonicalNext -Recurse -Force -ErrorAction SilentlyContinue
        }
        if (-not $canonicalMovedAside -and (Test-Path -LiteralPath $canonicalPrevious)) {
            Remove-Item -LiteralPath $canonicalPrevious -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    $result.ok = $true
    Write-Log "Rust desktop handoff complete; version $ExpectedVersion is serving all previously enabled runtimes."
} catch {
    $result.error = $_.Exception.Message
    Write-Log "Rust desktop handoff failed: $($result.error)"
    foreach ($process in @(Get-DesktopProcesses | Where-Object {
        $_.ExecutablePath -and [System.IO.Path]::GetFullPath([string]$_.ExecutablePath) -eq $stagedFullPath
    })) {
        Stop-ProcessTree ([int]$process.ProcessId)
    }

    $rollbackExe = $oldExecutables | Select-Object -First 1
    if ($rollbackExe) {
        Write-Log "Rolling back to previous Rust desktop: $rollbackExe"
        Start-Desktop $rollbackExe
        $result.rolledBack = Wait-AllEndpoints @($snapshot.endpoints) '' 30
        if ($result.rolledBack) { Write-Log 'Rollback restored the previous runtime endpoints.' }
        else { Write-Log 'Rollback process was started, but not all previous runtime endpoints became healthy.' }
    } else {
        Write-Log 'Rollback unavailable because the previous executable path could not be resolved.'
    }
} finally {
    $result.completedAt = [DateTime]::UtcNow.ToString('o')
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ResultPath -Encoding UTF8
}

if (-not $result.ok) { exit 1 }
