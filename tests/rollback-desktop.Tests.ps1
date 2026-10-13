$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../scripts/rollback-desktop.ps1') -TargetExe 'unused.exe' -LibraryOnly
function Assert-Equal($Actual, $Expected, $Message) { if ($Actual -ne $Expected) { throw "$Message (actual=$Actual expected=$Expected)" } }

Assert-Equal (Test-DesktopName 'ctmcp-0.1.141-win64.exe') $true 'Versioned download name must be recognized'
Assert-Equal (Test-DesktopName 'notepad.exe') $false 'Unrelated process must not be stopped'
Assert-Equal (Test-SafeWorkspaceId '00000000000000000000000000000001') $true 'Hex workspace IDs are valid'
Assert-Equal (Test-SafeWorkspaceId '..') $false 'Path traversal IDs are rejected'

$root = Join-Path ([IO.Path]::GetTempPath()) ("ctmcp-rollback-test-" + [guid]::NewGuid())
try {
    $data = Join-Path $root 'data'
    $shared = Join-Path $root 'shared'
    $backup = Join-Path $data 'pre-global-connection'
    $threw = $false
    try { Get-RestorePlan $backup $data $shared | Out-Null } catch { $threw = $true }
    Assert-Equal $threw $true 'Missing backup must fail before touching any file'

    New-Item -ItemType Directory -Path (Join-Path $backup 'workspaces/abc') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $backup 'workspaces/bad id') -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $backup 'profiles.json') -Value 'old-profiles' -NoNewline
    Set-Content -LiteralPath (Join-Path $backup 'workspaces/abc/workspace.json') -Value 'old-workspace' -NoNewline
    Set-Content -LiteralPath (Join-Path $backup 'workspaces/abc/other.txt') -Value 'ignored' -NoNewline
    Set-Content -LiteralPath (Join-Path $backup 'workspaces/bad id/workspace.json') -Value 'ignored' -NoNewline
    New-Item -ItemType Directory -Path (Join-Path $shared 'abc') -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $data 'profiles.json') -Value 'migrated-profiles' -NoNewline
    Set-Content -LiteralPath (Join-Path $shared 'abc/workspace.json') -Value 'migrated-workspace' -NoNewline

    $plan = @(Get-RestorePlan $backup $data $shared)
    Assert-Equal $plan.Count 2 'Only profiles.json and known mirror files of safe IDs are restored'
    $keep = Join-Path $data 'before-rollback-test'
    Invoke-RestorePlan $plan $keep
    Assert-Equal (Get-Content -LiteralPath (Join-Path $data 'profiles.json') -Raw) 'old-profiles' 'profiles.json not restored'
    Assert-Equal (Get-Content -LiteralPath (Join-Path $shared 'abc/workspace.json') -Raw) 'old-workspace' 'Shared mirror not restored'
    $kept = @(Get-ChildItem -LiteralPath $keep -File | Where-Object { $_.Name -ne 'index.txt' } | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw } | Sort-Object)
    Assert-Equal ($kept -join ',') 'migrated-profiles,migrated-workspace' 'Replaced files must be kept before restore'
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output 'PASS: rollback restore plan, path-safety and keep-before-restore (10 assertions). No process was stopped or started.'
