# This destructive-process fixture belongs ONLY on an isolated GitHub runner.
# Never run it on the user's desktop: that machine is running the real MCP.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) { throw 'Requires an isolated GitHub Actions runner.' }
$worker = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../scripts/desktop-startup-handoff.ps1'))
. $worker -LibraryOnly
if (@(Get-DesktopCandidates).Count -ne 0) { throw 'Refusing integration fixture: an existing desktop process was found.' }
$fixture = Join-Path $env:RUNNER_TEMP "ctmcp-upgrade-test-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $fixture | Out-Null
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$listener.Start(); $servicePort=$listener.LocalEndpoint.Port
$otherListener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$otherListener.Start(); $stalePort=$otherListener.LocalEndpoint.Port
$listener.Stop(); $otherListener.Stop()
$helper = Join-Path $fixture 'child-helper.exe'
$helperEntry = 'Helper_' + [guid]::NewGuid().ToString('N')
Add-Type -TypeDefinition "public static class $helperEntry { public static void Main() { while(true) System.Threading.Thread.Sleep(1000); } }" -OutputAssembly $helper -OutputType ConsoleApplication
function New-FixtureExe([string]$Name, [string]$Version, [bool]$Fail) {
    $dir = Join-Path $fixture $Name
    New-Item -ItemType Directory -Path $dir | Out-Null
    $exe = Join-Path $dir "ctmcp-$Version-win64.exe"
    $entry = 'Entry_' + [guid]::NewGuid().ToString('N')
    $body = if ($Fail) { 'return;' } else { @"
bool created;
mutex = new System.Threading.Mutex(false, @"Local\CodingToolsMcpDesktop-SingleInstance", out created);
if(!created) return;
System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(@"$helper") { UseShellExecute=false, CreateNoWindow=true });
var listener = new System.Net.Sockets.TcpListener(System.Net.IPAddress.Loopback, $servicePort);
listener.Start();
while(true) {
    using(var client=listener.AcceptTcpClient()) using(var stream=client.GetStream()) {
        var reader=new System.IO.StreamReader(stream);
        string line;
        do { line=reader.ReadLine(); } while(line != null && line.Length>0);
        var body=System.Text.Encoding.UTF8.GetBytes("{\"name\":\"coding-tools-mcp\",\"version\":\"$Version\"}");
        var header=System.Text.Encoding.ASCII.GetBytes("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + body.Length + "\r\nConnection: close\r\n\r\n");
        stream.Write(header,0,header.Length); stream.Write(body,0,body.Length);
    }
}
"@ }
    $members = if ($Fail) { '' } else { 'static System.Threading.Mutex mutex;' }
    $source = @"
using System.Reflection;
[assembly: AssemblyProduct("Coding Tools MCP")]
[assembly: AssemblyFileVersion("$Version")]
[assembly: AssemblyInformationalVersion("$Version")]
public static class $entry { $members public static void Main(string[] args) { $body } }
"@
    Add-Type -TypeDefinition $source -OutputAssembly $exe -OutputType ConsoleApplication
    return $exe
}
function Get-FixtureProcess([string]$Path) { @(Get-DesktopCandidates | Where-Object { $_.ExecutablePath -eq $Path }) }
function Wait-Fixture([string]$Path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do { $found = @(Get-FixtureProcess $Path); if ($found.Count -eq 1) { return $found[0] }; Start-Sleep -Milliseconds 100 } while ([DateTime]::UtcNow -lt $deadline)
    throw "Fixture did not start: $Path"
}
try {
    $oldExe = New-FixtureExe 'old' '0.1.65' $false
    $newExe = New-FixtureExe 'new' '0.1.66' $false
    $badExe = New-FixtureExe 'bad' '0.1.67' $true
    $data = Join-Path $fixture 'data/profiles.json'
    New-Item -ItemType Directory -Path (Split-Path -Parent $data) | Out-Null
    $config=@{profiles=@(@{id='live';bind=@{host='127.0.0.1';port=$servicePort}},@{id='stale';bind=@{host='127.0.0.1';port=$stalePort}});mcp_enabled_workspace_ids=@('live','stale');actions_enabled_workspace_ids=@()} | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText($data, $config)
    if (Get-Process -Id 2147483647 -ErrorAction SilentlyContinue) { throw 'Missing PID fixture is unexpectedly present.' }
    if ((Invoke-UpgradeTaskKill 2147483647 $true) -eq 0) { throw 'Missing process shutdown must have a native nonzero exit.' }
    $oldProcess = Start-Process -FilePath $oldExe -PassThru
    $null = Wait-Fixture $oldExe
    $endpoint=[pscustomobject]@{kind='mcp';url="http://127.0.0.1:$servicePort/mcp/info"}
    $deadline=[DateTime]::UtcNow.AddSeconds(10)
    while (-not (Get-EndpointHealth $endpoint).healthy) { if([DateTime]::UtcNow -gt $deadline){throw 'Old fixture service did not become ready.'}; Start-Sleep -Milliseconds 100 }
    $oldHelpers=@(Get-CimInstance Win32_Process -Filter "Name='child-helper.exe'" | Where-Object { $_.ExecutablePath -eq $helper })
    if($oldHelpers.Count -ne 1){throw 'Expected one old child process.'}
    & powershell -NoProfile -ExecutionPolicy Bypass -File $worker -NewExe $newExe -NewVersion '0.1.66' -DataFile $data -LauncherPid 0 -NoDialogs
    if ($LASTEXITCODE -ne 0) { throw 'Successful upgrade scenario failed.' }
    if (@(Get-FixtureProcess $oldExe).Count -ne 0) { throw 'Old process survived upgrade.' }
    $newProcess = Wait-Fixture $newExe
    if ((Get-EndpointHealth $endpoint).version -ne '0.1.66') { throw 'Replacement did not restore the real local service.' }
    foreach($child in $oldHelpers){if(Get-Process -Id $child.ProcessId -ErrorAction SilentlyContinue){throw 'Old child process survived upgrade.'}}
    # The real desktop consumes this snapshot. The dummy process deliberately
    # does not touch configuration, so clear only its temporary fixture file.
    Remove-Item -LiteralPath (Join-Path (Split-Path -Parent $data) 'runtime-handoff.json')
    & powershell -NoProfile -ExecutionPolicy Bypass -File $worker -NewExe $badExe -NewVersion '0.1.67' -DataFile $data -LauncherPid 0 -NoDialogs
    if ($LASTEXITCODE -ne 1) { throw 'Startup failure must be reported.' }
    $restored = Wait-Fixture $newExe
    if ($restored.ProcessId -eq $newProcess.ProcessId) { throw 'Rollback did not restart the previous executable.' }
    if ((Get-EndpointHealth $endpoint).version -ne '0.1.66') { throw 'Rollback did not restore the local service.' }
    if (@(Get-FixtureProcess $badExe).Count -ne 0) { throw 'Failed replacement is still running.' }
    if (-not (Test-Path -LiteralPath $worker)) { throw 'The original worker source must not be deleted.' }
    Write-Output 'PASS: native taskkill stderr tolerated; old tree/mutex released; versioned replacement restored HTTP; stale endpoint ignored; failed upgrade rolled back. Synthetic executables only.'
} finally {
    foreach ($candidate in @(Get-CimInstance Win32_Process)) {
        if ($candidate.ExecutablePath -and $candidate.ExecutablePath.StartsWith($fixture + '\', [StringComparison]::OrdinalIgnoreCase)) {
            Stop-Process -Id $candidate.ProcessId -Force -ErrorAction SilentlyContinue
        }
    }
    Remove-Item -LiteralPath $fixture -Recurse -Force -ErrorAction SilentlyContinue
}
# The rollback scenario intentionally leaves the child process exit code at 1.
# Only reaching this point means every assertion and cleanup completed.
exit 0
