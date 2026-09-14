[CmdletBinding()]
param(
    [string]$PackageRoot,
    [string]$DataDir,
    [string]$FallbackPackageRoot,
    [int]$HealthTimeoutSeconds = 90,
    [int]$DelaySeconds = 5,
    [string]$LogPath,
    [string]$ResultPath,
    [switch]$Worker,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Write-HandoffLog {
    param([Parameter(Mandatory = $true)][string]$Message)
    $line = "[{0}] {1}" -f (Get-Date).ToString('yyyy-MM-dd HH:mm:ss.fff'), $Message
    Write-Host $line
    if (-not [string]::IsNullOrWhiteSpace($script:ResolvedLogPath)) {
        $parent = Split-Path -Parent $script:ResolvedLogPath
        if ($parent) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
        Add-Content -LiteralPath $script:ResolvedLogPath -Value $line -Encoding UTF8
    }
}

function Resolve-AbsolutePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Base
    )
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $Base $Path))
}

function Get-PortableManifest {
    param([Parameter(Mandatory = $true)][string]$Root)
    $manifestPath = Join-Path $Root 'portable-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "Portable manifest not found: $manifestPath"
    }
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if ([string]::IsNullOrWhiteSpace([string]$manifest.nodeAgentVersion)) {
        throw "Portable manifest has no nodeAgentVersion: $manifestPath"
    }
    return $manifest
}

function Assert-CriticalPortableFiles {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)]$Manifest
    )
    $critical = @(
        'start-node-agent.bat',
        'portable-manifest.json',
        'app/dist/cli.js',
        'app/dist/server.js',
        'app/dist/ctmcp-protect.exe'
    )
    if ($Manifest.nodeRuntimeBundled -eq $true) {
        $critical += 'runtime/node.exe'
    }
    $checksumPath = Join-Path $Root 'SHA256SUMS.txt'
    if (-not (Test-Path -LiteralPath $checksumPath -PathType Leaf)) {
        throw "Portable checksum file not found: $checksumPath"
    }
    $checksums = @{}
    foreach ($line in Get-Content -LiteralPath $checksumPath) {
        if ($line -match '^([0-9a-fA-F]{64})  (.+)$') {
            $checksums[$Matches[2].Replace('\\', '/')] = $Matches[1].ToLowerInvariant()
        }
    }
    foreach ($relative in $critical) {
        $normalized = $relative.Replace('\\', '/')
        $path = Join-Path $Root ($relative.Replace('/', '\\'))
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Portable critical file not found: $path"
        }
        if (-not $checksums.ContainsKey($normalized)) {
            throw "Portable checksum is missing critical file: $normalized"
        }
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
        if ($actual -ne $checksums[$normalized]) {
            throw "Portable critical file checksum mismatch: $normalized"
        }
    }
}

function Get-ConfigPort {
    param([Parameter(Mandatory = $true)]$Config)
    if ($null -ne $Config.port) { return [int]$Config.port }
    if ($null -ne $Config.bind -and $null -ne $Config.bind.port) { return [int]$Config.bind.port }
    return 0
}
function Get-WorkspaceEndpoints {
    param([Parameter(Mandatory = $true)][string]$Directory)
    $registryPath = Join-Path $Directory 'workspace-profiles.json'
    $endpoints = @()
    if (Test-Path -LiteralPath $registryPath -PathType Leaf) {
        $registry = Get-Content -Raw -LiteralPath $registryPath | ConvertFrom-Json
        foreach ($workspace in @($registry.workspaces)) {
            $configPath = [string]$workspace.configPath
            if ([string]::IsNullOrWhiteSpace($configPath)) { continue }
            $configPath = Resolve-AbsolutePath -Path $configPath -Base (Split-Path -Parent $registryPath)
            if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
                throw "Workspace config not found: $configPath"
            }
            $config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
            $port = Get-ConfigPort -Config $config
            if ($port -lt 1 -or $port -gt 65535) {
                throw "Invalid workspace port in ${configPath}: $port"
            }
            $endpoints += [pscustomobject]@{
                name = [string]$workspace.name
                port = $port
                configPath = $configPath
            }
        }
    } else {
        $configPath = Join-Path $Directory 'agent.json'
        if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
            throw "Neither workspace-profiles.json nor agent.json exists under $Directory"
        }
        $config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
        $port = Get-ConfigPort -Config $config
        if ($port -lt 1 -or $port -gt 65535) {
            throw "Invalid workspace port in ${configPath}: $port"
        }
        $endpoints += [pscustomobject]@{ name = 'primary'; port = $port; configPath = $configPath }
    }
    $unique = @{}
    foreach ($endpoint in $endpoints) {
        $unique[[string]$endpoint.port] = $endpoint
    }
    return @($unique.Values | Sort-Object port)
}

function Get-NodeAgentProcesses {
    $self = $PID
    return @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        if ($_.ProcessId -eq $self -or [string]::IsNullOrWhiteSpace($_.CommandLine)) { return $false }
        $line = [string]$_.CommandLine
        return ($line -match '(?i)\\packages\\node-agent\\dist\\cli\.js') -or
            ($line -match '(?i)\\dist-node-portable\\[^\"\r\n]+\\app\\dist\\cli\.js') -or
            ($line -match '(?i)Coding\.Tools\.Node\.Agent[^\"\r\n]*\\app\\dist\\cli\.js') -or
            ($line -match '(?i)start-node-agent\.bat')
    })
}

function Stop-ExistingNodeAgents {
    for ($attempt = 0; $attempt -lt 6; $attempt++) {
        $processes = @(Get-NodeAgentProcesses)
        if ($processes.Count -eq 0) { return }
        foreach ($process in $processes | Sort-Object { if ($_.Name -ieq 'cmd.exe') { 0 } else { 1 } }) {
            Write-HandoffLog "Stopping old Agent process tree pid=$($process.ProcessId) name=$($process.Name)"
            $taskkill = Start-Process -FilePath 'taskkill.exe' -ArgumentList @('/PID', [string]$process.ProcessId, '/T', '/F') -WindowStyle Hidden -Wait -PassThru
            if ($taskkill.ExitCode -ne 0) {
                Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
            }
        }
        Start-Sleep -Milliseconds 500
    }
    $remaining = @(Get-NodeAgentProcesses)
    if ($remaining.Count -gt 0) {
        throw "Old Node Agent processes are still running: $($remaining.ProcessId -join ', ')"
    }
}

function Start-DetachedProcess {
    param([Parameter(Mandatory = $true)][string]$CommandLine)
    $result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $CommandLine }
    if ([int]$result.ReturnValue -ne 0 -or [int]$result.ProcessId -le 0) {
        throw "Win32_Process.Create failed with return value $($result.ReturnValue)."
    }
    return [int]$result.ProcessId
}

function Start-PortableAgent {
    param([Parameter(Mandatory = $true)][string]$Root)
    $launcher = Join-Path $Root 'start-node-agent.bat'
    $commandLine = "cmd.exe /d /c call `"$launcher`" --no-browser"
    $processId = Start-DetachedProcess -CommandLine $commandLine
    Write-HandoffLog "Started new portable supervisor pid=$processId root=$Root"
    return $processId
}

function Assert-PortableSupervisorRunning {
    param(
        [Parameter(Mandatory = $true)][int]$ProcessId,
        [Parameter(Mandatory = $true)][string]$Root
    )
    $process = Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction SilentlyContinue
    if ($null -eq $process) {
        throw "Portable supervisor exited before handoff completed. pid=$ProcessId root=$Root"
    }
    $launcher = (Join-Path $Root 'start-node-agent.bat').ToLowerInvariant()
    $commandLine = [string]$process.CommandLine
    if ($process.Name -ine 'cmd.exe' -or [string]::IsNullOrWhiteSpace($commandLine) -or $commandLine.ToLowerInvariant().IndexOf($launcher) -lt 0) {
        throw "Portable supervisor process contract mismatch. pid=$ProcessId name=$($process.Name) root=$Root"
    }
}

function Wait-NewAgentHealthy {
    param(
        [Parameter(Mandatory = $true)]$Endpoints,
        [Parameter(Mandatory = $true)][string]$ExpectedVersion,
        [Parameter(Mandatory = $true)][string]$ExpectedGitCommit,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $pending = @{}
    foreach ($endpoint in $Endpoints) { $pending[[string]$endpoint.port] = $endpoint }
    $lastErrors = @{}
    while ((Get-Date) -lt $deadline -and $pending.Count -gt 0) {
        foreach ($key in @($pending.Keys)) {
            $endpoint = $pending[$key]
            $url = "http://127.0.0.1:$($endpoint.port)/health"
            try {
                $health = Invoke-RestMethod -Uri $url -Method Get -TimeoutSec 2
                if ($health.ok -eq $true -and [string]$health.server -eq 'coding-tools-mcp-node' -and [string]$health.version -eq $ExpectedVersion -and [string]$health.buildGitSha -eq $ExpectedGitCommit) {
                    Write-HandoffLog "Healthy workspace '$($endpoint.name)' on port $($endpoint.port), version=$($health.version), build=$($health.buildGitSha)"
                    $pending.Remove($key)
                    continue
                }
                $lastErrors[$key] = "unexpected health payload/version: $($health | ConvertTo-Json -Compress)"
            } catch {
                $lastErrors[$key] = $_.Exception.Message
            }
        }
        if ($pending.Count -gt 0) { Start-Sleep -Milliseconds 500 }
    }
    if ($pending.Count -gt 0) {
        $details = @($pending.Keys | Sort-Object | ForEach-Object {
            $detail = if ($lastErrors.ContainsKey($_)) { [string]$lastErrors[$_] } else { 'no response' }
            "port ${_}: $detail"
        }) -join '; '
        throw "New Node Agent did not become healthy on every saved workspace port within ${TimeoutSeconds}s. $details"
    }
}

function Write-Result {
    param(
        [Parameter(Mandatory = $true)][bool]$Ok,
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)]$Endpoints,
        [string]$ErrorMessage = ''
    )
    $result = [ordered]@{
        ok = $Ok
        completedAt = (Get-Date).ToString('o')
        nodeAgentVersion = $Version
        packageRoot = $script:ResolvedPackageRoot
        dataDir = $script:ResolvedDataDir
        endpoints = @($Endpoints | ForEach-Object { [ordered]@{ name = $_.name; port = $_.port } })
        error = $ErrorMessage
    }
    $parent = Split-Path -Parent $script:ResolvedResultPath
    if ($parent) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    [System.IO.File]::WriteAllText($script:ResolvedResultPath, (($result | ConvertTo-Json -Depth 5) + "`r`n"), [System.Text.UTF8Encoding]::new($false))
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$script:ResolvedPackageRoot = if ([string]::IsNullOrWhiteSpace($PackageRoot)) {
    Join-Path $repoRoot 'dist-node-portable\ctnode-win64'
} else {
    Resolve-AbsolutePath -Path $PackageRoot -Base $repoRoot
}
$script:ResolvedFallbackPackageRoot = if ([string]::IsNullOrWhiteSpace($FallbackPackageRoot)) {
    $null
} else {
    Resolve-AbsolutePath -Path $FallbackPackageRoot -Base $repoRoot
}
$script:ResolvedDataDir = if (-not [string]::IsNullOrWhiteSpace($DataDir)) {
    Resolve-AbsolutePath -Path $DataDir -Base $repoRoot
} elseif (-not [string]::IsNullOrWhiteSpace($env:CTMCP_DATA_DIR)) {
    [System.IO.Path]::GetFullPath($env:CTMCP_DATA_DIR)
} else {
    Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'CodingToolsMCPNode'
}
$script:ResolvedLogPath = if ([string]::IsNullOrWhiteSpace($LogPath)) {
    Join-Path $script:ResolvedDataDir 'handoff.log'
} else {
    Resolve-AbsolutePath -Path $LogPath -Base $repoRoot
}
$script:ResolvedResultPath = if ([string]::IsNullOrWhiteSpace($ResultPath)) {
    Join-Path $script:ResolvedDataDir 'handoff.json'
} else {
    Resolve-AbsolutePath -Path $ResultPath -Base $repoRoot
}

if (-not (Test-Path -LiteralPath $script:ResolvedPackageRoot -PathType Container)) {
    throw "Portable package root not found: $script:ResolvedPackageRoot"
}
$manifest = Get-PortableManifest -Root $script:ResolvedPackageRoot
Assert-CriticalPortableFiles -Root $script:ResolvedPackageRoot -Manifest $manifest
$fallbackManifest = $null
if (-not [string]::IsNullOrWhiteSpace($script:ResolvedFallbackPackageRoot)) {
    if (-not (Test-Path -LiteralPath $script:ResolvedFallbackPackageRoot -PathType Container)) {
        throw "Fallback portable package root not found: $script:ResolvedFallbackPackageRoot"
    }
    $fallbackManifest = Get-PortableManifest -Root $script:ResolvedFallbackPackageRoot
    Assert-CriticalPortableFiles -Root $script:ResolvedFallbackPackageRoot -Manifest $fallbackManifest
}
$endpoints = @(Get-WorkspaceEndpoints -Directory $script:ResolvedDataDir)
if ($endpoints.Count -eq 0) { throw 'No saved Node Agent workspace endpoints were found.' }

if ($DryRun) {
    [pscustomobject]@{
        ok = $true
        dryRun = $true
        nodeAgentVersion = [string]$manifest.nodeAgentVersion
        packageRoot = $script:ResolvedPackageRoot
        dataDir = $script:ResolvedDataDir
        endpoints = $endpoints
        matchingOldProcesses = @((Get-NodeAgentProcesses).Count)
    } | ConvertTo-Json -Depth 5
    exit 0
}

if (-not $Worker) {
    $workerArgs = @(
        '-NoProfile',
        '-ExecutionPolicy', 'Bypass',
        '-File', "`"$PSCommandPath`"",
        '-Worker',
        '-PackageRoot', "`"$script:ResolvedPackageRoot`"",
        '-DataDir', "`"$script:ResolvedDataDir`"",
        '-HealthTimeoutSeconds', [string]$HealthTimeoutSeconds,
        '-DelaySeconds', [string]$DelaySeconds,
        '-LogPath', "`"$script:ResolvedLogPath`"",
        '-ResultPath', "`"$script:ResolvedResultPath`""
    )
    if (-not [string]::IsNullOrWhiteSpace($script:ResolvedFallbackPackageRoot)) {
        $workerArgs += @('-FallbackPackageRoot', "`"$script:ResolvedFallbackPackageRoot`"")
    }
    if (Test-Path -LiteralPath $script:ResolvedResultPath) {
        Remove-Item -LiteralPath $script:ResolvedResultPath -Force -ErrorAction SilentlyContinue
    }
    $workerCommandLine = 'powershell.exe ' + ($workerArgs -join ' ')
    $workerPid = Start-DetachedProcess -CommandLine $workerCommandLine
    [pscustomobject]@{
        ok = $true
        handoffScheduled = $true
        workerPid = $workerPid
        delaySeconds = $DelaySeconds
        expectedVersion = [string]$manifest.nodeAgentVersion
        packageRoot = $script:ResolvedPackageRoot
        resultPath = $script:ResolvedResultPath
        logPath = $script:ResolvedLogPath
        note = 'The detached worker will stop the old Agent only after this foreground invocation has returned.'
    } | ConvertTo-Json -Depth 4
    exit 0
}

$version = [string]$manifest.nodeAgentVersion
try {
    Write-HandoffLog "Detached handoff worker started; waiting ${DelaySeconds}s before stopping the old Agent. Target version=$version"
    Start-Sleep -Seconds ([Math]::Max(0, $DelaySeconds))
    Stop-ExistingNodeAgents
    Start-Sleep -Milliseconds 750
    $supervisorPid = Start-PortableAgent -Root $script:ResolvedPackageRoot
    Assert-PortableSupervisorRunning -ProcessId $supervisorPid -Root $script:ResolvedPackageRoot
    Wait-NewAgentHealthy -Endpoints $endpoints -ExpectedVersion $version -ExpectedGitCommit ([string]$manifest.gitCommit) -TimeoutSeconds $HealthTimeoutSeconds
    Assert-PortableSupervisorRunning -ProcessId $supervisorPid -Root $script:ResolvedPackageRoot
    Write-Result -Ok $true -Version $version -Endpoints $endpoints
    Write-HandoffLog "Node Agent handoff completed successfully. version=$version"
    exit 0
} catch {
    $message = $_.Exception.Message
    Write-HandoffLog "ERROR: $message"
    $rollbackMessage = ''
    if ($null -ne $fallbackManifest -and -not [string]::IsNullOrWhiteSpace($script:ResolvedFallbackPackageRoot)) {
        try {
            Write-HandoffLog "Target failed health validation; rolling back to $($fallbackManifest.nodeAgentVersion)."
            Stop-ExistingNodeAgents
            Start-Sleep -Milliseconds 750
            $rollbackPid = Start-PortableAgent -Root $script:ResolvedFallbackPackageRoot
            Assert-PortableSupervisorRunning -ProcessId $rollbackPid -Root $script:ResolvedFallbackPackageRoot
            Wait-NewAgentHealthy -Endpoints $endpoints -ExpectedVersion ([string]$fallbackManifest.nodeAgentVersion) -ExpectedGitCommit ([string]$fallbackManifest.gitCommit) -TimeoutSeconds $HealthTimeoutSeconds
            Assert-PortableSupervisorRunning -ProcessId $rollbackPid -Root $script:ResolvedFallbackPackageRoot
            $rollbackMessage = " Rollback to $($fallbackManifest.nodeAgentVersion) succeeded."
            Write-HandoffLog $rollbackMessage.Trim()
        } catch {
            $rollbackMessage = " Rollback also failed: $($_.Exception.Message)"
            Write-HandoffLog "ERROR:$rollbackMessage"
        }
    }
    try { Write-Result -Ok $false -Version $version -Endpoints $endpoints -ErrorMessage ($message + $rollbackMessage) } catch {}
    exit 1
}
