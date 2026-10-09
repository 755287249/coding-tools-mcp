[CmdletBinding()]
param()

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '')
    } finally {
        $sha.Dispose()
        $stream.Dispose()
    }
}

$ErrorActionPreference = 'Stop'
$totalTimer = [Diagnostics.Stopwatch]::StartNew()
$phaseTimer = [Diagnostics.Stopwatch]::new()
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$manifestPath = Join-Path $workspace 'src-tauri\Cargo.toml'
$packageJsonPath = Join-Path $workspace 'package.json'
$versionSyncScript = Join-Path $workspace 'scripts\sync-version.mjs'
$cargoTargetRoot = if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    Join-Path $workspace 'src-tauri\target'
} elseif ([System.IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $workspace $env:CARGO_TARGET_DIR))
}
$releaseExe = Join-Path $cargoTargetRoot 'release\coding-tools-mcp-desktop.exe'

Push-Location $workspace
try {
    Write-Host 'Synchronizing version metadata from package.json...'
    & node $versionSyncScript
    if ($LASTEXITCODE -ne 0) {
        throw "Version synchronization failed with exit code $LASTEXITCODE."
    }

    $packageJson = Get-Content -Raw -LiteralPath $packageJsonPath | ConvertFrom-Json
    $version = [string]$packageJson.version
    if ([string]::IsNullOrWhiteSpace($version)) {
        throw 'package.json does not define a version.'
    }

    $phaseTimer.Restart()
    Write-Host 'Building frontend assets...'
    & pnpm run build
    if ($LASTEXITCODE -ne 0) {
        throw "Frontend build failed with exit code $LASTEXITCODE."
    }

    Write-Host ('Frontend seconds: {0:N1}' -f $phaseTimer.Elapsed.TotalSeconds)
    $phaseTimer.Restart()
    Write-Host 'Building production Tauri executable with custom protocol...'
    & cargo build `
        --release `
        --locked `
        --timings `
        --manifest-path $manifestPath `
        --features custom-protocol `
        --bin coding-tools-mcp-desktop
    if ($LASTEXITCODE -ne 0) {
        throw "Rust release build failed with exit code $LASTEXITCODE."
    }
    if (-not (Test-Path -LiteralPath $releaseExe -PathType Leaf)) {
        throw "Release executable was not produced: $releaseExe"
    }

    Write-Host ('Rust seconds: {0:N1}' -f $phaseTimer.Elapsed.TotalSeconds)
    $phaseTimer.Restart()
    $packageName = "ctmcp-${version}-win64"
    $expandedName = 'ctmcp-win64'
    $distRoot = Join-Path $workspace 'dist-portable'
    New-Item -ItemType Directory -Path $distRoot -Force | Out-Null
    $expandedDir = Join-Path $distRoot $expandedName
    $zipPath = Join-Path $distRoot "$packageName.zip"
    $standaloneExe = Join-Path $distRoot "$packageName.exe"
    $stagingRoot = Join-Path ([System.IO.Path]::GetTempPath()) "ctmcp-$([guid]::NewGuid().ToString('N'))"
    $stagingPackageDir = Join-Path $stagingRoot $packageName
    $stagingExe = Join-Path $stagingPackageDir 'ctmcp.exe'
    $stagingZip = Join-Path $stagingRoot "$packageName.zip"
    $expandedStagingDir = Join-Path $distRoot ".$expandedName.next-$([guid]::NewGuid().ToString('N'))"

    try {
        New-Item -ItemType Directory -Path $stagingPackageDir -Force | Out-Null
        Copy-Item -LiteralPath $releaseExe -Destination $stagingExe
        Compress-Archive `
            -LiteralPath $stagingPackageDir `
            -DestinationPath $stagingZip `
            -CompressionLevel Optimal
        Move-Item -LiteralPath $stagingZip -Destination $zipPath -Force
        Copy-Item -LiteralPath $stagingExe -Destination $standaloneExe -Force

        Copy-Item -LiteralPath $stagingPackageDir -Destination $expandedStagingDir -Recurse
        try {
            if (Test-Path -LiteralPath $expandedDir) {
                Remove-Item -LiteralPath $expandedDir -Recurse -Force
            }
            Move-Item -LiteralPath $expandedStagingDir -Destination $expandedDir
        } catch [System.IO.IOException] {
            Write-Warning "Expanded portable folder was not replaced, usually because it is running. The ZIP is current: $zipPath"
        }
    } finally {
        if (Test-Path -LiteralPath $expandedStagingDir) {
            try {
                Remove-Item -LiteralPath $expandedStagingDir -Recurse -Force
            } catch {
                Write-Warning "Could not remove expanded staging folder (usually locked by a running app): $expandedStagingDir"
            }
        }
        $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
        $resolvedStaging = [System.IO.Path]::GetFullPath($stagingRoot)
        if ($resolvedStaging.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
            (Test-Path -LiteralPath $resolvedStaging)) {
            Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
        }
    }

    $exeHash = Get-Sha256 -Path $releaseExe
    $zipHash = Get-Sha256 -Path $zipPath
    $exeInfo = Get-Item -LiteralPath $releaseExe
    $zipInfo = Get-Item -LiteralPath $zipPath

    Write-Host "Release executable: $($exeInfo.FullName)"
    Write-Host "Executable bytes: $($exeInfo.Length)"
    Write-Host "Executable SHA-256: $exeHash"
    Write-Host "Portable ZIP: $($zipInfo.FullName)"
    Write-Host "ZIP bytes: $($zipInfo.Length)"
    Write-Host "ZIP SHA-256: $zipHash"
    Write-Host "Expanded portable: $expandedDir"
    Write-Host "Standalone EXE: $standaloneExe"
    Write-Host "Standalone SHA-256: $(Get-Sha256 $standaloneExe)"
    Write-Host ('Packaging seconds: {0:N1}; total seconds: {1:N1}' -f $phaseTimer.Elapsed.TotalSeconds, $totalTimer.Elapsed.TotalSeconds)
} finally {
    Pop-Location
}
