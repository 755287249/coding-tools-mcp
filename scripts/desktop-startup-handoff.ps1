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
        if (-not (Test-DesktopName $candidate.Name)) { continue }
        if ($candidate.ProductName -ne 'Coding Tools MCP' -or [string]::IsNullOrWhiteSpace($candidate.ExecutablePath)) { continue }
        if ([string]::Equals($candidate.ExecutablePath, $TargetPath, [StringComparison]::OrdinalIgnoreCase)) { continue }
        $version = $null
        if (-not [version]::TryParse([string]$candidate.Version, [ref]$version)) { continue }
        if ($version -lt $TargetVersion) { $candidate }
    }
}

function Test-DesktopName([string]$Name) {
    return $Name -match '^(ctmcp(?:-\d+\.\d+\.\d+-win64)?(?: \(\d+\))?|Coding Tools MCP|coding-tools-mcp-desktop)\.exe$'
}

function Get-DesktopCandidates {
    foreach ($process in @(Get-CimInstance Win32_Process -Filter "Name LIKE 'ctmcp%.exe' OR Name='Coding Tools MCP.exe' OR Name='coding-tools-mcp-desktop.exe'")) {
        if (-not (Test-DesktopName $process.Name)) { continue }
        if (-not $process.ExecutablePath) { continue }
        try {
            $info = [Diagnostics.FileVersionInfo]::GetVersionInfo([string]$process.ExecutablePath)
            [pscustomobject]@{ ProcessId=[int]$process.ProcessId; SessionId=[int]$process.SessionId; Name=[string]$process.Name; ExecutablePath=[IO.Path]::GetFullPath([string]$process.ExecutablePath); ProductName=$info.ProductName; Version=$info.ProductVersion; CreatedAt=$process.CreationDate.ToUniversalTime().Ticks }
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

function Write-UpgradeLog([string]$Message) {
    if ($script:log) { Add-Content -LiteralPath $script:log -Value "[$([DateTime]::UtcNow.ToString('o'))] $Message" }
}

function Get-EndpointHealth($Endpoint) {
    $response = $null
    try {
        # Local readiness must not depend on a user's HTTP proxy settings.
        $request = [Net.HttpWebRequest]::Create($Endpoint.url)
        $request.Proxy = $null
        $request.Timeout = 2000
        $request.ReadWriteTimeout = 2000
        $response = $request.GetResponse()
        $reader = [IO.StreamReader]::new($response.GetResponseStream())
        try { $reply = $reader.ReadToEnd() | ConvertFrom-Json } finally { $reader.Dispose() }
        $version = if ($Endpoint.kind -eq 'mcp' -and $reply.name -eq 'coding-tools-mcp') { [string]$reply.version } elseif ($Endpoint.kind -eq 'actions') { [string]$reply.info.version } else { '' }
        $valid = $version -match '^\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$'
        return [pscustomobject]@{healthy=$valid;version=$(if($valid){$version}else{''});detail=$(if($valid){'ready'}else{'unexpected response'})}
    } catch {
        return [pscustomobject]@{healthy=$false;version='';detail=$_.Exception.GetType().Name}
    } finally { if ($response) { $response.Dispose() } }
}

function Test-UpgradeHealth {
    param($Process, [array]$Endpoints, [string]$Version, [int]$TimeoutSeconds = 90)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $started = [DateTime]::UtcNow
    $last = ''
    do {
        $Process.Refresh()
        if ($Process.HasExited) { return [pscustomobject]@{ready=$false;reason="Replacement exited with code $($Process.ExitCode)."} }
        $failures = @()
        foreach ($endpoint in $Endpoints) {
            $health = Get-EndpointHealth $endpoint
            if (-not $health.healthy -or $health.version -ne $Version) {
                $failures += "$($endpoint.kind) port=$(([uri]$endpoint.url).Port) expected=$Version observed=$($health.version) result=$($health.detail)"
            }
        }
        $current = $failures -join '; '
        if ($current -and $current -ne $last) { Write-UpgradeLog "Waiting for services: $current"; $last=$current }
        if ($failures.Count -eq 0 -and ([DateTime]::UtcNow - $started).TotalSeconds -ge 3) { return [pscustomobject]@{ready=$true;reason='ready'} }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    return [pscustomobject]@{ready=$false;reason="Services did not recover: $last"}
}

function Get-UpgradeTree($Root) {
    $all = @(Get-CimInstance Win32_Process)
    $matchingRoot = @($all | Where-Object { $_.ProcessId -eq $Root.ProcessId -and $_.SessionId -eq $Root.SessionId -and $_.CreationDate -and $_.CreationDate.ToUniversalTime().Ticks -eq $Root.CreatedAt })
    if ($matchingRoot.Count -eq 0) { return }
    $ids = [Collections.Generic.HashSet[int]]::new()
    [void]$ids.Add([int]$Root.ProcessId)
    do {
        $changed=$false
        foreach ($item in $all) {
            if ($item.SessionId -eq $Root.SessionId -and $ids.Contains([int]$item.ParentProcessId)) {
                if ($ids.Add([int]$item.ProcessId)) { $changed=$true }
            }
        }
    } while ($changed)
    foreach ($item in $all) {
        if ($ids.Contains([int]$item.ProcessId) -and $item.CreationDate) {
            [pscustomobject]@{ProcessId=[int]$item.ProcessId;SessionId=[int]$item.SessionId;CreatedAt=$item.CreationDate.ToUniversalTime().Ticks;Name=[string]$item.Name}
        }
    }
}

function Get-RemainingUpgradeProcesses([array]$Captured) {
    $all = @(Get-CimInstance Win32_Process)
    foreach ($identity in $Captured) {
        foreach ($item in $all) {
            if ($item.ProcessId -eq $identity.ProcessId -and $item.SessionId -eq $identity.SessionId -and $item.CreationDate -and $item.CreationDate.ToUniversalTime().Ticks -eq $identity.CreatedAt) { $identity }
        }
    }
}

function Invoke-UpgradeTaskKill([int]$ProcessId, [bool]$Tree = $false) {
    # Calling a native command with 2>$null under Windows PowerShell + Stop
    # can throw on stderr even when the target parent has already exited.
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $env:SystemRoot 'System32\taskkill.exe'
    $start.Arguments = "/PID $ProcessId /F" + $(if($Tree){' /T'}else{''})
    $start.UseShellExecute=$false; $start.CreateNoWindow=$true
    $start.RedirectStandardOutput=$true; $start.RedirectStandardError=$true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(15000)) { $process.Kill(); throw 'Timed out while requesting application shutdown.' }
        $null=$stdout.GetAwaiter().GetResult(); $null=$stderr.GetAwaiter().GetResult()
        return $process.ExitCode
    } finally { $process.Dispose() }
}

function Stop-UpgradeTree($Candidate) {
    $captured = @(Get-UpgradeTree $Candidate)
    if ($captured.Count -eq 0) { return }
    $null = Invoke-UpgradeTaskKill $Candidate.ProcessId $true
    $remaining = @(Get-RemainingUpgradeProcesses $captured)
    # Retry only identities captured from this exact old process tree. A reused
    # PID must never become a termination target.
    foreach ($identity in $remaining) {
        if (@(Get-RemainingUpgradeProcesses @($identity)).Count -gt 0) { $null = Invoke-UpgradeTaskKill $identity.ProcessId }
    }
    $deadline=[DateTime]::UtcNow.AddSeconds(15)
    do {
        $remaining=@(Get-RemainingUpgradeProcesses $captured)
        if ($remaining.Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "The previous process tree did not exit: $($remaining.ProcessId -join ', '). Check whether it is running as administrator."
}

function Wait-DesktopMutexReleased([int]$TimeoutSeconds=15) {
    $deadline=[DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $existing=$null
        try { $existing=[Threading.Mutex]::OpenExisting('Local\CodingToolsMcpDesktop-SingleInstance') }
        catch [Threading.WaitHandleCannotBeOpenedException] { return $true }
        finally { if($existing){$existing.Dispose()} }
        Start-Sleep -Milliseconds 100
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
    $requiredEndpoints = @()
    foreach ($endpoint in @($snapshot.endpoints)) {
        if ((Get-EndpointHealth $endpoint).healthy) { $requiredEndpoints += $endpoint }
        else { Write-UpgradeLog "Already unavailable before upgrade: $($endpoint.kind) port=$(([uri]$endpoint.url).Port)" }
    }
    Write-UpgradeLog "Preparing $NewVersion; previously healthy services=$($requiredEndpoints.Count)"
    $handoff = Join-Path $dataDir 'runtime-handoff.json'
    if (Test-Path -LiteralPath $handoff) { throw 'A previous runtime handoff is pending. Restart the existing application first.' }
    $json = @{mcpWorkspaceIds=@($snapshot.mcpWorkspaceIds);actionsWorkspaceIds=@($snapshot.actionsWorkspaceIds)} | ConvertTo-Json -Depth 4
    $temporary = "$handoff.$PID.tmp"
    [IO.File]::WriteAllText($temporary, $json, [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $handoff
    $wroteHandoff = $true
    foreach ($candidate in $old) {
        $current = @(Get-DesktopCandidates | Where-Object { $_.ProcessId -eq $candidate.ProcessId -and $_.ExecutablePath -eq $candidate.ExecutablePath -and $_.Version -eq $candidate.Version -and $_.SessionId -eq $sessionId -and $_.CreatedAt -eq $candidate.CreatedAt })
        if ($current.Count -eq 0) { continue }
        Write-UpgradeLog "Replacing desktop pid=$($candidate.ProcessId) version=$($candidate.Version) with $NewVersion"
        # Mark before shutdown: a native failure may have already stopped the
        # root. Recovery below checks actual surviving desktop identities.
        $stoppedAny = $true
        Stop-UpgradeTree $candidate
    }
    if (-not (Wait-DesktopMutexReleased)) { throw 'The desktop single-instance lock is still held after shutdown.' }
    $replacement = Start-Process -FilePath $target -WorkingDirectory (Split-Path -Parent $target) -ArgumentList '--handoff-child' -PassThru
    $health = Test-UpgradeHealth $replacement $requiredEndpoints $NewVersion
    if (-not $health.ready) { throw $health.reason }
    Write-UpgradeLog "Ready: desktop $NewVersion pid=$($replacement.Id)"
} catch {
    $failure = $_.Exception.Message
    Write-UpgradeLog "Failed: $failure"
    if ($replacement) {
        $replacement.Refresh()
        if (-not $replacement.HasExited) {
            try { $null = Invoke-UpgradeTaskKill $replacement.Id $true; $null=$replacement.WaitForExit(15000) }
            catch { Write-UpgradeLog 'Replacement shutdown failed; inspect the running application before retrying.' }
        }
    }
    $survivors = @(Get-DesktopCandidates | Where-Object { $_.SessionId -eq $sessionId })
    if ($stoppedAny -and $survivors.Count -eq 0 -and $old.Count -gt 0 -and (Test-Path -LiteralPath $old[0].ExecutablePath) -and (Wait-DesktopMutexReleased)) {
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
