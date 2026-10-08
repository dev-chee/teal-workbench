[CmdletBinding()]
param(
    [string]$InstallDir = '',
    [switch]$Purge
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$script:MarkerName = '.teal-install.json'
$script:InstallFormatVersion = 1

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
                throw "Reparse points are not allowed in managed paths: $current"
            }
        }
        $parent = [System.IO.Directory]::GetParent($current)
        if ($null -eq $parent) {
            break
        }
        $current = $parent.FullName
    }
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

function Read-InstallMarker {
    param([Parameter(Mandatory = $true)][string]$Root)

    $markerPath = Join-Path $Root $script:MarkerName
    if (-not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
        throw "Directory is not managed by the Teal installer: $Root"
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
    foreach ($property in @('formatVersion', 'installDir', 'version', 'managedEntries')) {
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

    if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
        throw "Teal installation does not exist: $Root"
    }
    Assert-NoReparsePath $Root
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
    # 允许卸载损坏但仍可识别的安装，未知条目则必须由用户先处理。
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
    $candidate = [Environment]::ExpandEnvironmentVariables($Entry.Trim().Trim('"'))
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

function Remove-PathEntry {
    param(
        [AllowNull()][string]$Value,
        [Parameter(Mandatory = $true)][string]$Path
    )

    if ($null -eq $Value) {
        return $null
    }
    $kept = New-Object 'System.Collections.Generic.List[string]'
    foreach ($entry in @([regex]::Split($Value, ';'))) {
        if (-not (Test-EquivalentPathEntry $entry $Path)) {
            $kept.Add($entry)
        }
    }
    return ($kept -join ';')
}

function Get-TealHome {
    if (-not [string]::IsNullOrWhiteSpace($env:TEAL_HOME)) {
        return Get-NormalizedPath $env:TEAL_HOME
    }
    if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        return Get-NormalizedPath (Join-Path $env:USERPROFILE '.teal')
    }
    if (-not [string]::IsNullOrWhiteSpace($env:HOME)) {
        return Get-NormalizedPath (Join-Path $env:HOME '.teal')
    }
    throw 'Unable to determine TEAL_HOME.'
}

function Assert-SafeRemovalPath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Description
    )

    $normalized = Get-NormalizedPath $Path
    $root = [System.IO.Path]::GetPathRoot($normalized)
    if (Test-PathEqual $normalized $root) {
        throw "$Description cannot be a filesystem root: $normalized"
    }
    foreach ($homeVariable in @($env:USERPROFILE, $env:HOME)) {
        if (-not [string]::IsNullOrWhiteSpace($homeVariable)) {
            $userHomePath = Get-NormalizedPath $homeVariable
            if ((Test-PathEqual $normalized $userHomePath) -or (Test-PathInside $userHomePath $normalized)) {
                throw "$Description cannot be a user home directory or one of its parents: $normalized"
            }
        }
    }
    Assert-NoReparsePath $normalized
}

function Move-CurrentLocationOutside {
    param([Parameter(Mandatory = $true)][string[]]$RemovalRoots)

    $current = Get-NormalizedPath (Get-Location).Path
    foreach ($root in $RemovalRoots) {
        if ((Test-PathEqual $current $root) -or (Test-PathInside $current $root)) {
            $destination = Split-Path -Parent $root
            Set-Location -LiteralPath $destination
            return
        }
    }
}

function Uninstall-Teal {
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        throw 'The Teal PowerShell uninstaller supports Windows only.'
    }

    if ([string]::IsNullOrWhiteSpace($script:InstallDir)) {
        if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
            throw 'LOCALAPPDATA is unavailable; pass -InstallDir explicitly.'
        }
        $script:InstallDir = Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') 'Teal'
    }
    $installRoot = Get-NormalizedPath $script:InstallDir
    Assert-SafeRemovalPath $installRoot 'The installation directory'
    Assert-ManagedInstallation $installRoot

    $tealHome = $null
    if ($script:Purge) {
        $tealHome = Get-TealHome
        Assert-SafeRemovalPath $tealHome 'TEAL_HOME'
        if ((Test-PathEqual $tealHome $installRoot) -or
            (Test-PathInside $tealHome $installRoot) -or
            (Test-PathInside $installRoot $tealHome)) {
            throw 'TEAL_HOME and the installation directory cannot overlap during purge.'
        }
    }

    $removalRoots = @($installRoot)
    if ($script:Purge -and (Test-Path -LiteralPath $tealHome)) {
        $removalRoots += $tealHome
    }
    Move-CurrentLocationOutside $removalRoots

    $installParent = Split-Path -Parent $installRoot
    $installLeaf = Split-Path -Leaf $installRoot
    $nonce = [Guid]::NewGuid().ToString('N')
    $installQuarantine = Join-Path $installParent (".$installLeaf.uninstall-$nonce")
    $homeQuarantine = $null
    $installMoved = $false
    $homeMoved = $false
    $oldUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $oldProcessPath = $env:Path
    $pathChanged = $false

    # 先把两个删除目标移到同级隔离目录；PATH 写入失败时仍能原位恢复。
    try {
        Move-Item -LiteralPath $installRoot -Destination $installQuarantine
        $installMoved = $true

        if ($script:Purge -and (Test-Path -LiteralPath $tealHome)) {
            if (-not (Test-Path -LiteralPath $tealHome -PathType Container)) {
                throw "TEAL_HOME is not a directory: $tealHome"
            }
            $homeParent = Split-Path -Parent $tealHome
            $homeLeaf = Split-Path -Leaf $tealHome
            $homeQuarantine = Join-Path $homeParent (".$homeLeaf.purge-$nonce")
            Move-Item -LiteralPath $tealHome -Destination $homeQuarantine
            $homeMoved = $true
        }

        $newUserPath = Remove-PathEntry $oldUserPath $installRoot
        if (-not [string]::Equals($newUserPath, $oldUserPath, [System.StringComparison]::Ordinal)) {
            [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
            $pathChanged = $true
        }
        $env:Path = Remove-PathEntry $env:Path $installRoot
    }
    catch {
        $failure = $_
        if ($pathChanged) {
            try {
                [Environment]::SetEnvironmentVariable('Path', $oldUserPath, 'User')
            }
            catch {
                Write-Warning 'Unable to restore the previous user PATH after uninstall failure.'
            }
        }
        $env:Path = $oldProcessPath
        if ($homeMoved -and (Test-Path -LiteralPath $homeQuarantine) -and -not (Test-Path -LiteralPath $tealHome)) {
            try {
                Move-Item -LiteralPath $homeQuarantine -Destination $tealHome
            }
            catch {
                Write-Warning "Unable to restore TEAL_HOME from $homeQuarantine"
            }
        }
        if ($installMoved -and (Test-Path -LiteralPath $installQuarantine) -and -not (Test-Path -LiteralPath $installRoot)) {
            try {
                Move-Item -LiteralPath $installQuarantine -Destination $installRoot
            }
            catch {
                Write-Warning "Unable to restore the installation from $installQuarantine"
            }
        }
        throw $failure
    }

    $cleanupErrors = New-Object 'System.Collections.Generic.List[string]'
    foreach ($path in @($installQuarantine, $homeQuarantine)) {
        if (-not [string]::IsNullOrEmpty($path) -and (Test-Path -LiteralPath $path)) {
            try {
                Remove-Item -LiteralPath $path -Recurse -Force
            }
            catch {
                $cleanupErrors.Add($path)
            }
        }
    }
    if ($cleanupErrors.Count -gt 0) {
        throw "Teal was detached, but cleanup failed for: $($cleanupErrors -join ', ')"
    }

    Write-Host "Teal uninstalled from $installRoot"
    if ($script:Purge) {
        Write-Host "Teal user data purged from $tealHome"
    }
}

try {
    Uninstall-Teal
}
catch {
    Write-Error $_
    exit 1
}
