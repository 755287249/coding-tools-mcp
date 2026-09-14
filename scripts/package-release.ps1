[CmdletBinding()]
param(
    [ValidateSet('all', 'desktop', 'node')]
    [string]$Target = 'all',

    [switch]$SkipVersionBump,
    [switch]$SkipCommit,
    [switch]$SkipTag,
    [switch]$SkipSmokeTest,
    [switch]$SkipNodeVerify,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

function Get-JsonVersion {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)
    $document = Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
    $version = [string]$document.version
    if ([string]::IsNullOrWhiteSpace($version)) {
        throw "$Label does not define a version."
    }
    return $version
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Command,
        [Parameter(Mandatory = $true)][string]$Label
    )
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE."
    }
}

function Get-RepoStatusLines {
    param([Parameter(Mandatory = $true)][string]$Repository)
    $lines = @(& git.exe -C $Repository status --porcelain --untracked-files=normal)
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect the repository worktree status.'
    }
    return @($lines)
}

function Get-GitIdentity {
    param([Parameter(Mandatory = $true)][string]$Repository)
    $name = [string](& git.exe -C $Repository config --get user.name 2>$null)
    $email = [string](& git.exe -C $Repository config --get user.email 2>$null)
    if (-not [string]::IsNullOrWhiteSpace($name) -and -not [string]::IsNullOrWhiteSpace($email)) {
        return [pscustomobject]@{ Name = $name.Trim(); Email = $email.Trim() }
    }
    $fallback = [string](& git.exe -C $Repository log -1 --format='%an|%ae')
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($fallback) -or -not $fallback.Contains('|')) {
        throw 'Git user.name/user.email are unset and no previous commit identity is available.'
    }
    $parts = $fallback.Trim().Split('|', 2)
    Write-Warning "Using last commit identity for git: $($parts[0]) <$($parts[1])>"
    return [pscustomobject]@{ Name = $parts[0]; Email = $parts[1] }
}

function Update-ParityBaselines {
    param(
        [Parameter(Mandatory = $true)][string]$Repository,
        [Parameter(Mandatory = $true)][string]$DesktopVersion,
        [Parameter(Mandatory = $true)][string]$NodeAgentVersion,
        [Parameter(Mandatory = $true)][string]$ClientVersion
    )

    $manifestPath = Join-Path $Repository 'docs\todo\node-agent-parity\manifest.json'
    $checklistPath = Join-Path $Repository 'docs\todo\node-agent-ui-parity\CHECKLIST.md'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath
    $manifest = [regex]::Replace($manifest, '"desktop_client_version"\s*:\s*"[^"]+"', "`"desktop_client_version`": `"$DesktopVersion`"")
    $manifest = [regex]::Replace($manifest, '"node_agent_version"\s*:\s*"[^"]+"', "`"node_agent_version`": `"$NodeAgentVersion`"")
    $manifest = [regex]::Replace($manifest, '"client_compatibility_version"\s*:\s*"[^"]+"', "`"client_compatibility_version`": `"$ClientVersion`"")
    $manifest = [regex]::Replace($manifest, '"audited_at"\s*:\s*"[^"]+"', "`"audited_at`": `"$((Get-Date).ToString('yyyy-MM-dd'))`"")
    [System.IO.File]::WriteAllText($manifestPath, $manifest)

    $checklist = Get-Content -Raw -LiteralPath $checklistPath
    $updated = [regex]::Replace(
        $checklist,
        '\*\*Baseline:\*\* Rust/Desktop Client `[^`]+`[^\r\n]*?Node Agent `[^`]+`[^\r\n]*?client compatibility `[^`]+`',
        "**Baseline:** Rust/Desktop Client ``$DesktopVersion`` · Node Agent ``$NodeAgentVersion`` · client compatibility ``$ClientVersion``"
    )
    if ($updated -eq $checklist) {
        throw 'Unable to update the Node Agent UI parity checklist baseline line.'
    }
    [System.IO.File]::WriteAllText($checklistPath, $updated)
}

function New-AnnotatedTag {
    param(
        [Parameter(Mandatory = $true)][string]$Repository,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Message,
        [Parameter(Mandatory = $true)]$Identity
    )
    $existing = [string](& git.exe -C $Repository tag -l $Name)
    if (-not [string]::IsNullOrWhiteSpace($existing) -and $existing.Trim() -eq $Name) {
        $tagCommit = [string](& git.exe -C $Repository rev-parse "$Name^{commit}")
        $head = [string](& git.exe -C $Repository rev-parse HEAD)
        if ($tagCommit.Trim() -eq $head.Trim()) {
            Write-Host "Release tag already points at HEAD: $Name"
            return
        }
        throw "Release tag $Name already exists and points at $tagCommit, not HEAD $head."
    }
    & git.exe -C $Repository -c "user.name=$($Identity.Name)" -c "user.email=$($Identity.Email)" tag -a $Name -m $Message
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to create annotated tag $Name."
    }
}

function Get-FileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
        $stream.Dispose()
    }
}

function Wait-HttpOk {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [int]$TimeoutSeconds = 30
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $lastError = $null
    while ((Get-Date) -lt $deadline) {
        try {
            $response = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -eq 200) {
                return $response
            }
        } catch {
            $lastError = $_
        }
        Start-Sleep -Milliseconds 400
    }
    throw "Timed out waiting for $Url. $lastError"
}

function Start-NodeSmoke {
    param(
        [Parameter(Mandatory = $true)][string]$PackageRoot,
        [Parameter(Mandatory = $true)][int]$Port,
        [Parameter(Mandatory = $true)][string]$DataDir
    )
    New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
    $bat = Join-Path $PackageRoot 'start-node-agent.bat'
    $process = Start-Process -FilePath 'cmd.exe' -ArgumentList @(
        '/c',
        "set `"CTMCP_PORT=$Port`"&& set `"CTMCP_DATA_DIR=$DataDir`"&& `"$bat`" --no-browser"
    ) -WorkingDirectory $PackageRoot -PassThru -WindowStyle Hidden
    return $process
}

function Stop-NodeSmoke {
    param([Parameter(Mandatory = $true)][string]$Marker)
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object {
            ($_.CommandLine -and $_.CommandLine.Contains($Marker)) -or
            ($_.ExecutablePath -and $_.ExecutablePath.Contains($Marker))
        } |
        ForEach-Object {
            Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
        }
}

function Test-NodePortableSmoke {
    param(
        [Parameter(Mandatory = $true)][string]$ZipPath,
        [Parameter(Mandatory = $true)][int]$Port
    )
    $stage = Join-Path ([System.IO.Path]::GetTempPath()) ("node-release-smoke {0}" -f ([guid]::NewGuid().ToString('N').Substring(0, 8)))
    New-Item -ItemType Directory -Path $stage | Out-Null
    try {
        Expand-Archive -LiteralPath $ZipPath -DestinationPath $stage -Force
        $root = Get-ChildItem -LiteralPath $stage -Directory | Select-Object -First 1
        if (-not $root) {
            throw "Portable ZIP has no top-level directory: $ZipPath"
        }
        $dataDir = Join-Path $root.FullName 'data-smoke'
        $process = Start-NodeSmoke -PackageRoot $root.FullName -Port $Port -DataDir $dataDir
        try {
            $health = Wait-HttpOk -Url "http://127.0.0.1:$Port/health"
            $ui = Wait-HttpOk -Url "http://127.0.0.1:$Port/ui"
            $css = Wait-HttpOk -Url "http://127.0.0.1:$Port/ui/app.css"
            $js = Wait-HttpOk -Url "http://127.0.0.1:$Port/ui/app.js"
            Write-Host "Smoke $($root.Name): health=$($health.StatusCode) ui=$($ui.StatusCode)/$($ui.RawContentLength) css=$($css.RawContentLength) js=$($js.RawContentLength)"
            Write-Host "Smoke health body: $($health.Content)"
        } finally {
            if ($process -and -not $process.HasExited) {
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            }
            Stop-NodeSmoke -Marker $root.FullName
        }
    } finally {
        Start-Sleep -Seconds 1
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$workspace = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$desktopPackage = Join-Path $workspace 'package.json'
$nodePackage = Join-Path $workspace 'packages\node-agent\package.json'
$portableMetadataPath = Join-Path $workspace 'packages\node-agent\portable-version.json'

Push-Location $workspace
try {
    $includeDesktop = $Target -in @('all', 'desktop')
    $includeNode = $Target -in @('all', 'node')
    $beforeDesktop = Get-JsonVersion -Path $desktopPackage -Label 'Desktop package.json'
    $beforeNode = Get-JsonVersion -Path $nodePackage -Label 'Node Agent package.json'
    $portableVersion = Get-JsonVersion -Path $portableMetadataPath -Label 'portable-version.json'
    $dirty = Get-RepoStatusLines -Repository $workspace

    Write-Host "Target: $Target"
    Write-Host "Current Desktop: $beforeDesktop"
    Write-Host "Current Node Agent: $beforeNode"
    Write-Host "Portable wrapper: $portableVersion"

    if ($dirty.Count -gt 0 -and -not $DryRun) {
        throw "Worktree must be clean before packaging.`n$($dirty -join "`n")"
    }
    if ($dirty.Count -gt 0) {
        Write-Warning "Worktree is dirty; dry run will not start a real package."
    }

    if ($DryRun) {
        $plannedDesktop = $beforeDesktop
        $plannedNode = $beforeNode
        if (-not $SkipVersionBump) {
            if ($includeDesktop) { $plannedDesktop = 'Desktop patch +1 (pnpm run version:patch)' }
            if ($includeNode) { $plannedNode = 'Node Agent patch +1 (pnpm version patch)' }
        }
        Write-Host "Dry run: would bump Desktop to $plannedDesktop"
        Write-Host "Dry run: would bump Node Agent to $plannedNode"
        if ($includeDesktop -and -not $SkipTag) { Write-Host 'Dry run: would create annotated Desktop tag v<desktop>' }
        if ($includeNode -and -not $SkipTag) { Write-Host 'Dry run: would create annotated Node tag node-agent-v<agent>-portable-v<portable>' }
        if ($includeDesktop) { Write-Host 'Dry run: would run pnpm run desktop:portable' }
        if ($includeNode) { Write-Host 'Dry run: would run pnpm run node-agent:portable' }
        return
    }

    $identity = Get-GitIdentity -Repository $workspace

    if (-not $SkipVersionBump) {
        if ($includeDesktop) {
            Write-Host 'Bumping Desktop patch version...'
            Invoke-Checked -Label 'Desktop version:patch' -Command { pnpm run version:patch }
        }
        if ($includeNode) {
            Write-Host 'Bumping Node Agent patch version...'
            Invoke-Checked -Label 'Node Agent version patch' -Command {
                Push-Location (Join-Path $workspace 'packages\node-agent')
                try {
                    pnpm version patch --no-git-tag-version
                } finally {
                    Pop-Location
                }
            }
        }
        $desktopVersion = Get-JsonVersion -Path $desktopPackage -Label 'Desktop package.json'
        $nodeVersion = Get-JsonVersion -Path $nodePackage -Label 'Node Agent package.json'
        $nodeDocument = Get-Content -Raw -LiteralPath $nodePackage | ConvertFrom-Json
        $clientVersion = [string]$nodeDocument.codingTools.clientVersion
        Update-ParityBaselines -Repository $workspace -DesktopVersion $desktopVersion -NodeAgentVersion $nodeVersion -ClientVersion $clientVersion
        Invoke-Checked -Label 'version:check' -Command { pnpm run version:check }

        if (-not $SkipCommit) {
            $allow = @(
                'package.json',
                'src-tauri/Cargo.toml',
                'src-tauri/Cargo.lock',
                'packages/node-agent/package.json',
                'packages/node-agent/src/clientVersion.generated.ts',
                'docs/todo/node-agent-parity/manifest.json',
                'docs/todo/node-agent-ui-parity/CHECKLIST.md'
            )
            $changed = @(Get-RepoStatusLines -Repository $workspace | ForEach-Object { $_.Substring(3).Trim().Replace('\', '/') })
            $unexpected = @($changed | Where-Object { $allow -notcontains $_ })
            if ($unexpected.Count -gt 0) {
                throw "Version bump produced unexpected files:`n$($unexpected -join "`n")"
            }
            if ($changed.Count -eq 0) {
                throw 'Version bump did not change any tracked files.'
            }
            & git.exe -C $workspace add -- $changed
            if ($LASTEXITCODE -ne 0) { throw 'git add failed.' }
            $message = if ($includeDesktop -and $includeNode) {
                "chore(release): bump versions to desktop $desktopVersion and node-agent $nodeVersion"
            } elseif ($includeDesktop) {
                "chore(release): bump desktop to $desktopVersion"
            } else {
                "chore(release): bump node-agent to $nodeVersion"
            }
            & git.exe -C $workspace -c "user.name=$($identity.Name)" -c "user.email=$($identity.Email)" commit -m $message
            if ($LASTEXITCODE -ne 0) { throw 'git commit failed.' }
        }
    }

    $desktopVersion = Get-JsonVersion -Path $desktopPackage -Label 'Desktop package.json'
    $nodeVersion = Get-JsonVersion -Path $nodePackage -Label 'Node Agent package.json'
    $desktopTag = "v$desktopVersion"
    $nodeTag = "node-agent-v$nodeVersion-portable-v$portableVersion"
    $head = [string](& git.exe -C $workspace rev-parse HEAD)

    if (-not $SkipTag) {
        if ($includeDesktop) {
            New-AnnotatedTag -Repository $workspace -Name $desktopTag -Message "Desktop portable $desktopVersion" -Identity $identity
        }
        if ($includeNode) {
            New-AnnotatedTag -Repository $workspace -Name $nodeTag -Message "node-agent portable`n`nagent: $nodeVersion`nportable: $portableVersion`ndesktop: $desktopVersion" -Identity $identity
        }
    }

    $dirtyAfter = Get-RepoStatusLines -Repository $workspace
    if ($dirtyAfter.Count -gt 0) {
        throw "Worktree is dirty after version/tag steps.`n$($dirtyAfter -join "`n")"
    }

    $artifacts = @()

    if ($includeDesktop) {
        Write-Host 'Building Desktop portable ZIP...'
        Invoke-Checked -Label 'desktop:portable' -Command { pnpm run desktop:portable }
        $desktopZip = Join-Path $workspace "dist-portable\ctmcp-${desktopVersion}-win64.zip"
        $desktopExe = Join-Path $workspace 'dist-portable\ctmcp-win64\ctmcp.exe'
        if (-not (Test-Path -LiteralPath $desktopZip -PathType Leaf)) {
            throw "Desktop portable ZIP was not produced: $desktopZip"
        }
        if (-not (Test-Path -LiteralPath $desktopExe -PathType Leaf)) {
            Write-Warning "Expanded Desktop executable is missing (usually locked by a running app). ZIP is current: $desktopZip"
        }
        $artifacts += [pscustomobject]@{
            kind = 'desktop'
            path = $desktopZip
            bytes = (Get-Item -LiteralPath $desktopZip).Length
            sha256 = Get-FileSha256 -Path $desktopZip
        }
    }

    if ($includeNode) {
        Write-Host 'Validating Node portable release identity...'
        $checker = Join-Path $workspace 'skills\node-agent-portable-packager\scripts\check-release-versions.ps1'
        Invoke-Checked -Label 'Node portable version check' -Command {
            powershell.exe -NoProfile -ExecutionPolicy Bypass -File $checker -RepositoryRoot $workspace
        }

        $bundledZip = Join-Path $workspace "dist-node-portable\ctnode-${nodeVersion}-p${portableVersion}-win64.zip"
        $systemZip = Join-Path $workspace "dist-node-portable\ctnode-${nodeVersion}-p${portableVersion}-sys-win64.zip"
        $nodePortableScript = Join-Path $workspace 'packages\node-agent\scripts\build-portable.ps1'

        Write-Host 'Building Node Agent portable ZIPs...'
        try {
            if ($SkipNodeVerify) {
                Invoke-Checked -Label 'node-agent:portable' -Command {
                    powershell.exe -NoProfile -ExecutionPolicy Bypass -File $nodePortableScript -SkipVerify
                }
            } else {
                Invoke-Checked -Label 'node-agent:portable' -Command { pnpm run node-agent:portable }
            }
        } catch {
            if ((Test-Path -LiteralPath $bundledZip -PathType Leaf) -and -not (Test-Path -LiteralPath $systemZip -PathType Leaf)) {
                Write-Warning "Full Node portable build stopped after bundled ZIP. Building system-node only. $_"
                if ($SkipNodeVerify) {
                    Invoke-Checked -Label 'node-agent:portable:system' -Command {
                        powershell.exe -NoProfile -ExecutionPolicy Bypass -File $nodePortableScript -Edition system-node -SkipVerify
                    }
                } else {
                    Invoke-Checked -Label 'node-agent:portable:system' -Command { pnpm run node-agent:portable:system }
                }
            } elseif ((Test-Path -LiteralPath $bundledZip -PathType Leaf) -and (Test-Path -LiteralPath $systemZip -PathType Leaf)) {
                Write-Warning "Node portable script reported an error, but both ZIPs exist. Continuing. $_"
            } else {
                throw
            }
        }

        foreach ($zip in @($bundledZip, $systemZip)) {
            if (-not (Test-Path -LiteralPath $zip -PathType Leaf)) {
                throw "Node portable ZIP was not produced: $zip"
            }
            $artifacts += [pscustomobject]@{
                kind = 'node'
                path = $zip
                bytes = (Get-Item -LiteralPath $zip).Length
                sha256 = Get-FileSha256 -Path $zip
            }
        }

        if (-not $SkipSmokeTest) {
            Write-Host 'Smoke-testing Node portable editions...'
            Test-NodePortableSmoke -ZipPath $bundledZip -Port 3791
            Test-NodePortableSmoke -ZipPath $systemZip -Port 3792
        }
    }

    Write-Host ''
    Write-Host "HEAD: $head"
    Write-Host "Desktop: $desktopVersion"
    Write-Host "Node Agent: $nodeVersion"
    Write-Host "Portable wrapper: $portableVersion"
    if ($includeDesktop) { Write-Host "Desktop tag: $desktopTag" }
    if ($includeNode) { Write-Host "Node tag: $nodeTag" }
    foreach ($artifact in $artifacts) {
        Write-Host ("{0}: {1}" -f $artifact.kind, $artifact.path)
        Write-Host ("  bytes={0}" -f $artifact.bytes)
        Write-Host ("  sha256={0}" -f $artifact.sha256)
    }
} finally {
    Pop-Location
}
