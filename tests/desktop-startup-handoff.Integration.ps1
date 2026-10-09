# This destructive-process fixture belongs ONLY on an isolated GitHub runner.
# Never run it on the user's desktop: that machine is running the real MCP.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or -not $env:RUNNER_TEMP) { throw 'Requires an isolated GitHub Actions runner.' }
$worker = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../scripts/desktop-startup-handoff.ps1'))
. $worker -LibraryOnly
if (@(Get-DesktopCandidates).Count -ne 0) { throw 'Refusing integration fixture: an existing desktop process was found.' }
$fixture = Join-Path $env:RUNNER_TEMP "ctmcp-upgrade-test-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $fixture | Out-Null
function New-FixtureExe([string]$Name, [string]$Version, [bool]$Fail) {
    $dir = Join-Path $fixture $Name
    New-Item -ItemType Directory -Path $dir | Out-Null
    $exe = Join-Path $dir 'ctmcp.exe'
    $entry = 'Entry_' + [guid]::NewGuid().ToString('N')
    $body = if ($Fail) { 'return;' } else { 'while(true) System.Threading.Thread.Sleep(1000);' }
    $source = @"
using System.Reflection;
[assembly: AssemblyProduct("Coding Tools MCP")]
[assembly: AssemblyFileVersion("$Version")]
[assembly: AssemblyInformationalVersion("$Version")]
public static class $entry { public static void Main(string[] args) { $body } }
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
    [IO.File]::WriteAllText($data, '{"profiles":[],"mcp_enabled_workspace_ids":[],"actions_enabled_workspace_ids":[]}')
    $oldProcess = Start-Process -FilePath $oldExe -PassThru
    $null = Wait-Fixture $oldExe
    & powershell -NoProfile -ExecutionPolicy Bypass -File $worker -NewExe $newExe -NewVersion '0.1.66' -DataFile $data -LauncherPid 0 -NoDialogs
    if ($LASTEXITCODE -ne 0) { throw 'Successful upgrade scenario failed.' }
    if (@(Get-FixtureProcess $oldExe).Count -ne 0) { throw 'Old process survived upgrade.' }
    $newProcess = Wait-Fixture $newExe
    # The real desktop consumes this snapshot. The dummy process deliberately
    # does not touch configuration, so clear only its temporary fixture file.
    Remove-Item -LiteralPath (Join-Path (Split-Path -Parent $data) 'runtime-handoff.json')
    & powershell -NoProfile -ExecutionPolicy Bypass -File $worker -NewExe $badExe -NewVersion '0.1.67' -DataFile $data -LauncherPid 0 -NoDialogs
    if ($LASTEXITCODE -ne 1) { throw 'Startup failure must be reported.' }
    $restored = Wait-Fixture $newExe
    if ($restored.ProcessId -eq $newProcess.ProcessId) { throw 'Rollback did not restart the previous executable.' }
    if (@(Get-FixtureProcess $badExe).Count -ne 0) { throw 'Failed replacement is still running.' }
    if (-not (Test-Path -LiteralPath $worker)) { throw 'The original worker source must not be deleted.' }
    Write-Output 'PASS: older process stopped, replacement started, failed upgrade rolled back; only synthetic executables used.'
} finally {
    foreach ($candidate in @(Get-DesktopCandidates)) {
        if ($candidate.ExecutablePath.StartsWith($fixture + '\', [StringComparison]::OrdinalIgnoreCase)) {
            Stop-Process -Id $candidate.ProcessId -Force -ErrorAction SilentlyContinue
        }
    }
    Remove-Item -LiteralPath $fixture -Recurse -Force -ErrorAction SilentlyContinue
}
# The rollback scenario intentionally leaves the child process exit code at 1.
# Only reaching this point means every assertion and cleanup completed.
exit 0
