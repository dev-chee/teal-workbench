[CmdletBinding()]
param(
    [string]$InstallDir = '',
    [switch]$NoPath
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$script:MarkerName = '.teal-install.json'
$script:ManifestName = 'manifest.json'
$script:InstallFormatVersion = 1
$script:RuntimeManifestFormatVersion = 1
$script:ExecutableNames = @(
    'teal.exe',
    'teal-run.exe',
    'teal-perm.exe',
    'teal-config.exe',
    'teal-bench.exe',
    'teal-knowledge.exe',
    'teal-fmt.exe',
    'teal-lsp.exe',
    'teal-dap.exe'
)

function Get-NormalizedPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($fullPath)
    if ($fullPath.Length -gt $root.Length) {
        $fullPath = $fullPath.TrimEnd([char[]]@('\', '/'))
    }
    return $fullPath
}

function Test-PathEqual {
    param(
        [Parameter(Mandatory = $true)][string]$Left,
        [Parameter(Mandatory = $true)][string]$Right
    )

    return [string]::Equals(
        (Get-NormalizedPath $Left),
        (Get-NormalizedPath $Right),
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Test-PathInside {
    param(
        [Parameter(Mandatory = $true)][string]$Child,
        [Parameter(Mandatory = $true)][string]$Parent
    )

    $normalizedChild = Get-NormalizedPath $Child
    $normalizedParent = Get-NormalizedPath $Parent
    $prefix = $normalizedParent
    if (-not $prefix.EndsWith([System.IO.Path]::DirectorySeparatorChar.ToString())) {
        $prefix += [System.IO.Path]::DirectorySeparatorChar
    }
    return $normalizedChild.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-NoReparsePath {
    param([Parameter(Mandatory = $true)][string]$Path)

    $current = Get-NormalizedPath $Path
    while (-not [string]::IsNullOrEmpty($current)) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse points are not allowed in installation paths: $current"
            }
        }
        $parent = [System.IO.Directory]::GetParent($current)
        if ($null -eq $parent) {
            break
        }
        $current = $parent.FullName
    }
}

function Assert-SafeInstallPath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$SourceRoot
    )

    $normalized = Get-NormalizedPath $Path
    $root = [System.IO.Path]::GetPathRoot($normalized)
    if (Test-PathEqual $normalized $root) {
        throw "The installation directory cannot be a filesystem root: $normalized"
    }

    $userProfilePath = $env:USERPROFILE
    if (-not [string]::IsNullOrWhiteSpace($userProfilePath)) {
        $userProfilePath = Get-NormalizedPath $userProfilePath
        if ((Test-PathEqual $normalized $userProfilePath) -or (Test-PathInside $userProfilePath $normalized)) {
            throw "The installation directory cannot be the user profile or one of its parents: $normalized"
        }
    }

    if ((Test-PathEqual $normalized $SourceRoot) -or
        (Test-PathInside $normalized $SourceRoot) -or
        (Test-PathInside $SourceRoot $normalized)) {
        throw "The installation directory cannot overlap the Teal source tree: $normalized"
    }

    Assert-NoReparsePath $normalized
}

function Assert-Property {
    param(
        [Parameter(Mandatory = $true)]$Object,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Context
    )

    if ($null -eq $Object -or -not ($Object.PSObject.Properties.Name -contains $Name)) {
        throw "$Context is missing property '$Name'."
    }
}

function ConvertTo-SafeRelativePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Context
    )

    if ([string]::IsNullOrWhiteSpace($Path) -or
        $Path.Contains('\') -or
        $Path.StartsWith('/') -or
        [System.IO.Path]::IsPathRooted($Path)) {
        throw "$Context contains an invalid relative path: $Path"
    }

    $segments = $Path.Split('/')
    foreach ($segment in $segments) {
        if ([string]::IsNullOrEmpty($segment) -or $segment -eq '.' -or $segment -eq '..' -or $segment.Contains(':')) {
            throw "$Context contains an invalid relative path: $Path"
        }
    }
    return ($segments -join '/')
}

function Get-TreeEntries {
    param([Parameter(Mandatory = $true)][string]$Root)

    $normalizedRoot = Get-NormalizedPath $Root
    if (-not (Test-Path -LiteralPath $normalizedRoot -PathType Container)) {
        throw "Directory does not exist: $normalizedRoot"
    }

    $prefix = $normalizedRoot
    if (-not $prefix.EndsWith([System.IO.Path]::DirectorySeparatorChar.ToString())) {
        $prefix += [System.IO.Path]::DirectorySeparatorChar
    }
    $pending = New-Object 'System.Collections.Generic.Stack[string]'
    $entries = New-Object 'System.Collections.Generic.List[object]'
    $pending.Push($normalizedRoot)

    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($item in Get-ChildItem -LiteralPath $directory -Force) {
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Reparse points are not allowed in managed trees: $($item.FullName)"
            }
            if (-not $item.FullName.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Managed entry escaped its root: $($item.FullName)"
            }
            $relative = $item.FullName.Substring($prefix.Length).Replace('\', '/')
            $kind = if ($item.PSIsContainer) { 'directory' } else { 'file' }
            $entries.Add([pscustomobject]@{ path = $relative; kind = $kind })
            if ($item.PSIsContainer) {
                $pending.Push($item.FullName)
            }
        }
    }

    return @($entries | Sort-Object -Property path)
}

function Get-EntryKey {
    param(
        [Parameter(Mandatory = $true)][string]$Kind,
        [Parameter(Mandatory = $true)][string]$Path
    )

    return ($Kind + ':' + $Path).ToLowerInvariant()
}

function Assert-RuntimeManifest {
    param([Parameter(Mandatory = $true)][string]$RuntimeRoot)

    $manifestPath = Join-Path $RuntimeRoot $script:ManifestName
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        throw "Runtime manifest is missing: $manifestPath"
    }
    $manifestItem = Get-Item -LiteralPath $manifestPath -Force
    if (($manifestItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Runtime manifest cannot be a reparse point: $manifestPath"
    }

    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    }
    catch {
        throw "Runtime manifest is not valid JSON: $manifestPath"
    }
    Assert-Property $manifest 'formatVersion' 'Runtime manifest'
    Assert-Property $manifest 'files' 'Runtime manifest'
    if ([int]$manifest.formatVersion -ne $script:RuntimeManifestFormatVersion) {
        throw "Unsupported runtime manifest version: $($manifest.formatVersion)"
    }

    # 清单是发行资源的完整文件集合；目录本身不参与摘要。
    $expected = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    [void]$expected.Add($script:ManifestName)
    $listedPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($file in @($manifest.files)) {
        Assert-Property $file 'path' 'Runtime manifest file'
        Assert-Property $file 'size' 'Runtime manifest file'
        Assert-Property $file 'sha256' 'Runtime manifest file'
        $relative = ConvertTo-SafeRelativePath ([string]$file.path) 'Runtime manifest'
        if (-not $listedPaths.Add($relative)) {
            throw "Runtime manifest contains a duplicate path: $relative"
        }
        $path = Join-Path $RuntimeRoot ($relative.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Runtime manifest file is missing: $relative"
        }
        $item = Get-Item -LiteralPath $path -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Runtime manifest file cannot be a reparse point: $relative"
        }
        if ([int64]$file.size -ne [int64]$item.Length) {
            throw "Runtime manifest size mismatch: $relative"
        }
        $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if (-not [string]::Equals($actualHash, ([string]$file.sha256).ToLowerInvariant(), [System.StringComparison]::Ordinal)) {
            throw "Runtime manifest hash mismatch: $relative"
        }
        [void]$expected.Add($relative)
    }

    foreach ($required in @('installs/install.ps1', 'installs/uninstall.ps1')) {
        if (-not $listedPaths.Contains($required)) {
            throw "Runtime manifest does not contain required installer resource: $required"
        }
    }

    $actualFiles = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in @(Get-TreeEntries $RuntimeRoot)) {
        if ($entry.kind -eq 'file') {
            [void]$actualFiles.Add([string]$entry.path)
        }
    }
    if ($actualFiles.Count -ne $expected.Count) {
        throw 'Runtime directory contains files not described by its manifest.'
    }
    foreach ($path in $expected) {
        if (-not $actualFiles.Contains($path)) {
            throw "Runtime directory does not match its manifest: $path"
        }
    }
}

function Assert-DistributionBundle {
    param([Parameter(Mandatory = $true)][string]$BundleRoot)

    if (-not (Test-Path -LiteralPath $BundleRoot -PathType Container)) {
        throw "The current build did not create the expected distribution: $BundleRoot"
    }
    Assert-NoReparsePath $BundleRoot

    $allowed = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($name in $script:ExecutableNames) {
        [void]$allowed.Add($name)
        $path = Join-Path $BundleRoot $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Distribution executable is missing: $name"
        }
        $item = Get-Item -LiteralPath $path -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Distribution executable cannot be a reparse point: $name"
        }
    }
    [void]$allowed.Add('runtime')

    foreach ($entry in Get-ChildItem -LiteralPath $BundleRoot -Force) {
        if (-not $allowed.Contains($entry.Name)) {
            throw "Distribution contains an unexpected top-level entry: $($entry.Name)"
        }
    }
    if (Test-Path -LiteralPath (Join-Path $BundleRoot $script:MarkerName)) {
        throw "Distribution cannot contain $($script:MarkerName)."
    }
    Assert-RuntimeManifest (Join-Path $BundleRoot 'runtime')
}

function Read-InstallMarker {
    param([Parameter(Mandatory = $true)][string]$Root)

    $markerPath = Join-Path $Root $script:MarkerName
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw "Existing non-empty directory is not managed by the Teal installer: $Root"
    }
    $markerItem = Get-Item -LiteralPath $markerPath -Force
    if (($markerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Installation marker cannot be a reparse point: $markerPath"
    }
    try {
        $marker = Get-Content -LiteralPath $markerPath -Raw | ConvertFrom-Json
    }
    catch {
        throw "Installation marker is not valid JSON: $markerPath"
    }
    foreach ($property in @('formatVersion', 'installDir', 'managedEntries')) {
        Assert-Property $marker $property 'Installation marker'
    }
    if ([int]$marker.formatVersion -ne $script:InstallFormatVersion) {
        throw "Unsupported installation marker version: $($marker.formatVersion)"
    }
    if (-not (Test-PathEqual ([string]$marker.installDir) $Root)) {
        throw "Installation marker belongs to another directory: $($marker.installDir)"
    }
    return $marker
}

function Assert-ManagedInstallation {
    param([Parameter(Mandatory = $true)][string]$Root)

    $children = @(Get-ChildItem -LiteralPath $Root -Force)
    if ($children.Count -eq 0) {
        return
    }
    $marker = Read-InstallMarker $Root
    $managed = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in @($marker.managedEntries)) {
        Assert-Property $entry 'path' 'Installation marker entry'
        Assert-Property $entry 'kind' 'Installation marker entry'
        $kind = [string]$entry.kind
        if ($kind -ne 'file' -and $kind -ne 'directory') {
            throw "Installation marker contains an invalid entry kind: $kind"
        }
        $relative = ConvertTo-SafeRelativePath ([string]$entry.path) 'Installation marker'
        if ($relative -eq $script:MarkerName) {
            throw 'Installation marker cannot list itself as a managed entry.'
        }
        $key = Get-EntryKey $kind $relative
        if (-not $managed.Add($key)) {
            throw "Installation marker contains a duplicate entry: $relative"
        }
    }

    # 缺失的受管文件允许通过重装修复，但任何未登记条目都可能是用户数据，必须拒绝删除。
    foreach ($entry in @(Get-TreeEntries $Root)) {
        if ($entry.path -eq $script:MarkerName) {
            continue
        }
        $key = Get-EntryKey ([string]$entry.kind) ([string]$entry.path)
        if (-not $managed.Contains($key)) {
            throw "Installation contains an unmanaged entry: $($entry.path)"
        }
    }
}

function Test-EquivalentPathEntry {
    param(
        [AllowNull()][string]$Entry,
        [Parameter(Mandatory = $true)][string]$Path
    )

    if ([string]::IsNullOrWhiteSpace($Entry)) {
        return $false
    }
    $candidate = $Entry.Trim().Trim('"')
    $candidate = [Environment]::ExpandEnvironmentVariables($candidate)
    try {
        if ([System.IO.Path]::IsPathRooted($candidate)) {
            return Test-PathEqual $candidate $Path
        }
    }
    catch {
        return $false
    }
    return $false
}

function Add-PathEntry {
    param(
        [AllowNull()][string]$Value,
        [Parameter(Mandatory = $true)][string]$Path
    )

    $entries = if ($null -eq $Value) { @() } else { @([regex]::Split($Value, ';')) }
    foreach ($entry in $entries) {
        if (Test-EquivalentPathEntry $entry $Path) {
            return $Value
        }
    }
    if ([string]::IsNullOrEmpty($Value)) {
        return $Path
    }
    if ($Value.EndsWith(';')) {
        return $Value + $Path
    }
    return $Value + ';' + $Path
}

function Invoke-Cargo {
    param(
        [Parameter(Mandatory = $true)][System.Management.Automation.CommandInfo]$Cargo,
        [Parameter(Mandatory = $true)][string]$SourceRoot,
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [switch]$Capture
    )

    Push-Location -LiteralPath $SourceRoot
    try {
        if ($Capture) {
            $output = & $Cargo.Source @Arguments
            $exitCode = $LASTEXITCODE
            if ($exitCode -ne 0) {
                throw "Cargo failed with exit code ${exitCode}: $($Arguments -join ' ')`n$($output -join [Environment]::NewLine)"
            }
            return @($output)
        }
        & $Cargo.Source @Arguments
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            throw "Cargo failed with exit code ${exitCode}: $($Arguments -join ' ')"
        }
    }
    finally {
        Pop-Location
    }
}

function Get-SourceBuildIdentity {
    param(
        [Parameter(Mandatory = $true)][System.Management.Automation.CommandInfo]$Cargo,
        [Parameter(Mandatory = $true)][string]$SourceRoot
    )

    $metadataText = Invoke-Cargo $Cargo $SourceRoot @(
        'metadata', '--locked', '--no-deps', '--format-version', '1', '--manifest-path',
        (Join-Path $SourceRoot 'Cargo.toml')
    ) -Capture
    try {
        $metadata = ($metadataText -join [Environment]::NewLine) | ConvertFrom-Json
    }
    catch {
        throw 'Cargo metadata did not return valid JSON.'
    }
    Assert-Property $metadata 'packages' 'Cargo metadata'
    $rootManifest = Get-NormalizedPath (Join-Path $SourceRoot 'Cargo.toml')
    $rootPackage = @($metadata.packages | Where-Object {
        $_.name -eq 'teal' -and (Test-PathEqual ([string]$_.manifest_path) $rootManifest)
    })
    if ($rootPackage.Count -ne 1) {
        throw 'Cargo metadata does not contain exactly one root Teal package.'
    }
    $version = [string]$rootPackage[0].version
    if ($version -notmatch '^[0-9A-Za-z.+-]+$') {
        throw "Cargo returned an invalid Teal version: $version"
    }

    $rustcCommand = if ([string]::IsNullOrWhiteSpace($env:RUSTC)) { 'rustc' } else { $env:RUSTC }
    $rustc = Get-Command $rustcCommand -CommandType Application -ErrorAction Stop
    $rustcOutput = & $rustc.Source '-vV' 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Rustc failed while determining the host triple.`n$($rustcOutput -join [Environment]::NewLine)"
    }
    $hostLine = @($rustcOutput | Where-Object { [string]$_ -like 'host: *' })
    if ($hostLine.Count -ne 1) {
        throw 'Rustc did not return exactly one host triple.'
    }
    $hostTriple = ([string]$hostLine[0]).Substring(6).Trim()
    if ($hostTriple -notmatch '^[0-9A-Za-z_.-]+$') {
        throw "Rustc returned an invalid host triple: $hostTriple"
    }

    return [pscustomobject]@{ version = $version; host = $hostTriple }
}

function Write-InstallMarker {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [Parameter(Mandatory = $true)][string]$Version
    )

    $managedEntries = @(
        Get-TreeEntries $Root | ForEach-Object {
            [ordered]@{ path = [string]$_.path; kind = [string]$_.kind }
        }
    )
    $marker = [ordered]@{
        formatVersion = $script:InstallFormatVersion
        installDir = $InstallDir
        version = $Version
        installedAtUtc = [DateTime]::UtcNow.ToString('o')
        managedEntries = $managedEntries
    }
    $encoded = $marker | ConvertTo-Json -Depth 5
    Set-Content -LiteralPath (Join-Path $Root $script:MarkerName) -Value $encoded -Encoding UTF8
}

function Remove-TreeBestEffort {
    param([AllowNull()][string]$Path)

    if (-not [string]::IsNullOrEmpty($Path) -and (Test-Path -LiteralPath $Path)) {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Install-Teal {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        throw 'The Teal PowerShell installer supports Windows only.'
    }

    $sourceRoot = Get-NormalizedPath (Join-Path $PSScriptRoot '..\..')
    foreach ($required in @('Cargo.toml', 'tools\teal-xtask\Cargo.toml', 'runtime')) {
        if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $required))) {
            throw "The installer is not inside a Teal source tree: missing $required"
        }
    }
    $expectedInstaller = Get-NormalizedPath (Join-Path $sourceRoot 'runtime\installs\install.ps1')
    if (-not (Test-PathEqual $PSCommandPath $expectedInstaller)) {
        throw 'The installer must run from runtime\installs in the current Teal source tree.'
    }

    if ([string]::IsNullOrWhiteSpace($script:InstallDir)) {
        if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
            throw 'LOCALAPPDATA is unavailable; pass -InstallDir explicitly.'
        }
        $script:InstallDir = Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') 'Teal'
    }
    $installRoot = Get-NormalizedPath $script:InstallDir
    Assert-SafeInstallPath $installRoot $sourceRoot

    $cargo = Get-Command 'cargo' -CommandType Application -ErrorAction Stop
    $identity = Get-SourceBuildIdentity $cargo $sourceRoot
    $buildStarted = [DateTime]::UtcNow
    Write-Host "Building Teal $($identity.version) from $sourceRoot"
    Invoke-Cargo $cargo $sourceRoot @('run', '--locked', '--package', 'teal-xtask', '--', 'dist')

    $bundleName = "teal-$($identity.version)-$($identity.host)"
    $bundleRoot = Get-NormalizedPath (Join-Path $sourceRoot (Join-Path 'target\dist' $bundleName))
    Assert-DistributionBundle $bundleRoot
    $manifestItem = Get-Item -LiteralPath (Join-Path $bundleRoot 'runtime\manifest.json')
    if ($manifestItem.LastWriteTimeUtc -lt $buildStarted.AddSeconds(-2)) {
        throw 'The distribution manifest was not produced by the current build.'
    }

    $parent = Split-Path -Parent $installRoot
    if ([string]::IsNullOrEmpty($parent)) {
        throw "Installation directory has no parent: $installRoot"
    }
    if (-not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    Assert-NoReparsePath $parent

    if (Test-Path -LiteralPath $installRoot) {
        if (-not (Test-Path -LiteralPath $installRoot -PathType Container)) {
            throw "Installation path is not a directory: $installRoot"
        }
        Assert-ManagedInstallation $installRoot
    }

    $leaf = Split-Path -Leaf $installRoot
    $nonce = [Guid]::NewGuid().ToString('N')
    $staging = Join-Path $parent (".$leaf.staging-$nonce")
    $backup = Join-Path $parent (".$leaf.backup-$nonce")
    $failed = Join-Path $parent (".$leaf.failed-$nonce")
    $hadExisting = Test-Path -LiteralPath $installRoot
    $swapped = $false
    $oldUserPath = $null
    $oldProcessPath = $env:Path
    $userPathChanged = $false

    try {
        New-Item -ItemType Directory -Path $staging | Out-Null
        foreach ($entry in Get-ChildItem -LiteralPath $bundleRoot -Force) {
            Copy-Item -LiteralPath $entry.FullName -Destination $staging -Recurse -Force
        }
        Write-InstallMarker $staging $installRoot ([string]$identity.version)

        # staging 与目标处于同一卷；旧目录先改名，任一步失败都可以恢复。
        if ($hadExisting) {
            Move-Item -LiteralPath $installRoot -Destination $backup
        }
        Move-Item -LiteralPath $staging -Destination $installRoot
        $swapped = $true

        if (-not $script:NoPath) {
            $oldUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
            $newUserPath = Add-PathEntry $oldUserPath $installRoot
            if (-not [string]::Equals($newUserPath, $oldUserPath, [System.StringComparison]::Ordinal)) {
                [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
                $userPathChanged = $true
            }
            $env:Path = Add-PathEntry $env:Path $installRoot
        }
    }
    catch {
        $failure = $_
        if ($userPathChanged) {
            try {
                [Environment]::SetEnvironmentVariable('Path', $oldUserPath, 'User')
            }
            catch {
                Write-Warning 'Unable to restore the previous user PATH after installation failure.'
            }
        }
        $env:Path = $oldProcessPath

        if ($swapped -and (Test-Path -LiteralPath $installRoot)) {
            try {
                Move-Item -LiteralPath $installRoot -Destination $failed
            }
            catch {
                Write-Warning "Unable to move the failed installation aside: $installRoot"
            }
        }
        if ($hadExisting -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $installRoot)) {
            try {
                Move-Item -LiteralPath $backup -Destination $installRoot
            }
            catch {
                Write-Warning "Unable to restore the previous installation from $backup"
            }
        }
        Remove-TreeBestEffort $staging
        Remove-TreeBestEffort $failed
        throw $failure
    }

    if (Test-Path -LiteralPath $backup) {
        try {
            Remove-Item -LiteralPath $backup -Recurse -Force
        }
        catch {
            Write-Warning "The new installation is active, but the previous installation could not be removed: $backup"
        }
    }
    Write-Host "Teal $($identity.version) installed at $installRoot"
    if (-not $script:NoPath) {
        Write-Host 'The user PATH is updated. Reopen existing terminals if they do not see the change.'
    }
}

try {
    Install-Teal
}
catch {
    Write-Error $_
    exit 1
}
