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
