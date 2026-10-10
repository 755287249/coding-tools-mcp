$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../scripts/desktop-startup-handoff.ps1') -LibraryOnly
function Assert-Equal($Actual, $Expected, $Message) { if ($Actual -ne $Expected) { throw "$Message (actual=$Actual expected=$Expected)" } }
function Candidate($Id, $Session, $Name, $Product, $Version, $Path) {
    [pscustomobject]@{ProcessId=$Id;SessionId=$Session;Name=$Name;ProductName=$Product;Version=$Version;ExecutablePath=$Path}
}
$candidates = @(
    (Candidate 1 3 'ctmcp.exe' 'Coding Tools MCP' '0.1.65' 'C:\old\ctmcp.exe'),
    (Candidate 2 4 'ctmcp.exe' 'Coding Tools MCP' '0.1.65' 'C:\other-session\ctmcp.exe'),
    (Candidate 3 3 'ctmcp.exe' 'Different App' '0.1.65' 'C:\other-app\ctmcp.exe'),
    (Candidate 4 3 'ctmcp.exe' 'Coding Tools MCP' '0.1.66' 'C:\equal\ctmcp.exe'),
    (Candidate 5 3 'ctmcp.exe' 'Coding Tools MCP' '0.2.0' 'C:\newer\ctmcp.exe'),
    (Candidate 6 3 'ctmcp.exe' 'Coding Tools MCP' 'unknown' 'C:\unknown\ctmcp.exe'),
    (Candidate 7 3 'ctmcp.exe' 'Coding Tools MCP' '0.1.65' 'C:\new\ctmcp.exe'),
    (Candidate 8 3 'something.exe' 'Coding Tools MCP' '0.1.65' 'C:\random\something.exe'),
    (Candidate 9 3 'ctmcp.exe' 'Coding Tools MCP' '0.1.65' 'C:\launcher\ctmcp.exe')
)
$selected = @(Select-UpgradeTargets $candidates 'C:\new\ctmcp.exe' ([version]'0.1.66') 3 9)
Assert-Equal $selected.Count 1 'Only the identified older instance may be stopped'
Assert-Equal $selected[0].ProcessId 1 'Wrong process selected'
Assert-Equal @(Select-UpgradeTargets $candidates 'C:\old\ctmcp.exe' ([version]'0.1.65') 3 9).Count 0 'Same version / path must not restart'
$data = @'
{"mcp_enabled_workspace_ids":["canonical","legacy"],"actions_enabled_workspace_ids":["legacy"],"profiles":[{"id":"canonical","bind":{"host":"::","port":3000}},{"id":"legacy","runtime":{"bind_address":"0.0.0.0","local_port":3001},"actions":{"bind_address":"127.0.0.1","local_port":3002}},{"id":"off","bind":{"host":"127.0.0.1","port":3003}}]}
'@ | ConvertFrom-Json
$snapshot = Get-UpgradeSnapshot $data
Assert-Equal @($snapshot.mcpWorkspaceIds).Count 2 'MCP enabled IDs changed'
Assert-Equal @($snapshot.actionsWorkspaceIds).Count 1 'Actions enabled IDs changed'
Assert-Equal @($snapshot.endpoints).Count 3 'Disabled endpoints must not be probed'
Assert-Equal $snapshot.endpoints[0].url 'http://[::1]:3000/mcp/info' 'IPv6 wildcard must map to loopback'
Assert-Equal $snapshot.endpoints[1].url 'http://127.0.0.1:3001/mcp/info' 'Legacy runtime port was lost'
Assert-Equal $snapshot.endpoints[2].url 'http://127.0.0.1:3002/openapi.json' 'Actions endpoint was lost'
$empty = Get-UpgradeSnapshot ([pscustomobject]@{profiles=@();mcp_enabled_workspace_ids=@();actions_enabled_workspace_ids=@()})
Assert-Equal @($empty.endpoints).Count 0 'Empty workspace state must stay empty'
Assert-Equal (Test-DesktopName 'ctmcp-0.1.67-win64.exe') $true 'Versioned download name must be recognized'
Assert-Equal (Test-DesktopName 'ctmcp (2).exe') $true 'Browser numbered download must be recognized'
Assert-Equal (Test-DesktopName 'ctmcp-random.exe') $false 'Unrelated filename must not be accepted'
Assert-Equal @(Select-UpgradeTargets @((Candidate 10 3 'ctmcp-0.1.66-win64.exe' 'Coding Tools MCP' '0.1.66' 'C:\old\ctmcp-0.1.66-win64.exe')) 'C:\new\ctmcp-0.1.67-win64.exe' ([version]'0.1.67') 3 9).Count 1 'Versioned old build must be selectable'
Write-Output 'PASS: version/product/session/PID/path selection and canonical/legacy runtime snapshots (14 assertions). No processes were started or stopped.'

# Pure doubles: no production process is stopped or launched by these checks.
$stamp=[DateTime]::new(638960000000000000,[DateTimeKind]::Utc)
$identities=@(1,2,3,4,5,6 | ForEach-Object { [pscustomobject]@{ProcessId=$_;SessionId=3;CreatedAt=$stamp.Ticks;Name='msedgewebview2.exe'} })
function Get-CimInstance {
    param($ClassName)
    foreach($item in $identities){[pscustomobject]@{ProcessId=$item.ProcessId;SessionId=$item.SessionId;CreationDate=$stamp}}
}
function Get-Process {
    param($Id,$ErrorAction)
    if($Id -eq 4){throw [UnauthorizedAccessException]::new('Synthetic access denied')}
    if($Id -eq 5){
        $record=[Management.Automation.ErrorRecord]::new([ArgumentException]::new('Synthetic process disappeared'),'NoProcessFoundForGivenId',[Management.Automation.ErrorCategory]::ObjectNotFound,$Id)
        throw $record
    }
    $result=[pscustomobject]@{HasExited=($Id -eq 2);SessionId=$(if($Id -eq 6){4}else{3});StartTime=$(if($Id -eq 3){$stamp.AddSeconds(1)}else{$stamp.AddTicks(1)})}
    $result | Add-Member ScriptMethod Dispose { $script:disposedCount++ }
    return $result
}
$script:disposedCount=0
try {
    $remaining=@(Get-RemainingUpgradeProcesses $identities)
    Assert-Equal ($remaining.ProcessId -join ',') '1,4' 'Only live and unreadable identities may remain; exited/reused/missing/foreign-session processes must not block'
    Assert-Equal $script:disposedCount 4 'Every successfully opened process must be disposed'
} finally {
    Remove-Item Function:\Get-Process
    Remove-Item Function:\Get-CimInstance
}
function New-TaskKillDouble($Finished,$Exited,$ExitCode,$DenyKill) {
    $p=[pscustomobject]@{Finished=$Finished;HasExited=$Exited;ExitCode=$ExitCode;DenyKill=$DenyKill;KillCalls=0}
    $p | Add-Member ScriptMethod WaitForExit {param($Timeout); return $this.Finished}
    $p | Add-Member ScriptMethod Kill {$this.KillCalls++;if($this.DenyKill){throw 'Synthetic cleanup access denied'}}
    return $p
}
$normal=New-TaskKillDouble $true $true 128 $false
Assert-Equal (Wait-UpgradeTaskKill $normal) 128 'Native nonzero exit must remain visible'
$race=New-TaskKillDouble $false $true 0 $true
Assert-Equal (Wait-UpgradeTaskKill $race) 1460 'Timeout/exit race must return for identity verification'
Assert-Equal $race.KillCalls 0 'An exited helper must not be killed'
$denied=New-TaskKillDouble $false $false 0 $true
Assert-Equal (Wait-UpgradeTaskKill $denied) 1460 'Cleanup denial must not bypass subsequent identity checks'
Assert-Equal $denied.KillCalls 1 'A live timed-out helper gets one cleanup attempt'

# Exercise shutdown orchestration with synthetic observations and requests only.
function Get-UpgradeTree {param($Candidate); return $identities[0]}
function Get-RemainingUpgradeProcesses {
    param($Captured)
    $script:remainingCalls++
    if($script:remainingCalls -le 2){return $identities[0]}
}
function Invoke-UpgradeTaskKill {param($ProcessId,$Tree);$script:shutdownCalls++;return 1460}
$script:remainingCalls=0;$script:shutdownCalls=0
try {
    Stop-UpgradeTree $identities[0]
    Assert-Equal $script:shutdownCalls 2 'A timeout must recheck and retry only the captured surviving identity'
    Assert-Equal $script:remainingCalls 3 'Shutdown succeeds only after no live captured identity remains'
} finally {
    Remove-Item Function:\Get-UpgradeTree
    Remove-Item Function:\Get-RemainingUpgradeProcesses
    Remove-Item Function:\Invoke-UpgradeTaskKill
}
Write-Output 'PASS: stale CIM entries, PID/session reuse, missing/denied process state, handle disposal, helper timeout/exit/denied-cleanup races and shutdown identity rechecks. Pure doubles only.'

function Start-Process {
    param($FilePath,$WorkingDirectory,$WindowStyle,[switch]$PassThru,$ArgumentList)
    return [pscustomobject]@{Path=$FilePath;Directory=$WorkingDirectory;Style=$WindowStyle;Retained=$PassThru.IsPresent;Arguments=$ArgumentList}
}
try {
    $replacement=Start-UpgradeDesktop -Path 'C:\with spaces\ctmcp.exe' -Handoff
    Assert-Equal $replacement.Style 'Normal' 'Replacement must override the hidden worker startup style'
    Assert-Equal $replacement.Directory 'C:\with spaces' 'Replacement working directory must be preserved'
    Assert-Equal $replacement.Arguments '--handoff-child' 'Replacement must retain recursive handoff guard'
    Assert-Equal $replacement.Retained $true 'Health checks need the replacement process handle'
    $rollback=Start-UpgradeDesktop -Path 'C:\old\ctmcp.exe'
    Assert-Equal $rollback.Style 'Normal' 'Rollback must restore a visible desktop too'
    Assert-Equal ([string]$rollback.Arguments) '' 'Rollback must preserve normal launch semantics'
} finally { Remove-Item Function:\Start-Process }
Write-Output 'PASS: replacement and rollback launch visibly, preserve paths/process handles and isolate the handoff argument. No real process launched.'
