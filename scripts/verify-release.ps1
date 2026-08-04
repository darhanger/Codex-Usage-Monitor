[CmdletBinding()]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDirectory = 'artifacts',

    [Parameter()]
    [string]$ExpectedVersion
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$outputPath = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    [System.IO.Path]::GetFullPath($OutputDirectory)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory))
}

function Get-RootPackageMetadata {
    $manifestPath = Join-Path $repositoryRoot 'Cargo.toml'
    $metadataJson = & cargo metadata --no-deps --locked --format-version 1 --manifest-path $manifestPath
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE."
    }

    try {
        $metadata = ($metadataJson -join [System.Environment]::NewLine) | ConvertFrom-Json
    } catch {
        throw "Unable to parse cargo metadata: $($_.Exception.Message)"
    }

    $manifestFullPath = [System.IO.Path]::GetFullPath($manifestPath)
    $packages = @($metadata.packages | Where-Object {
        [System.IO.Path]::GetFullPath($_.manifest_path) -eq $manifestFullPath
    })
    if ($packages.Count -ne 1) {
        throw "Expected exactly one root Cargo package, found $($packages.Count)."
    }

    return $packages[0]
}

function Assert-PeVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value,

        [Parameter(Mandatory = $true)]
        [string]$Version
    )

    $pattern = '^{0}(?:\.0)?$' -f [regex]::Escape($Version)
    if ($Value -notmatch $pattern) {
        throw "$Name '$Value' does not match Cargo version $Version."
    }
}

function Assert-Checksum {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string]$ChecksumPath
    )

    $checksumLine = (Get-Content -LiteralPath $ChecksumPath -Raw).Trim()
    $checksumMatch = [regex]::Match(
        $checksumLine,
        '^(?<hash>[0-9a-fA-F]{64}) \*(?<name>[^\r\n]+)$'
    )
    $fileName = [System.IO.Path]::GetFileName($FilePath)
    if (-not $checksumMatch.Success -or $checksumMatch.Groups['name'].Value -cne $fileName) {
        throw "Invalid checksum format or file name in $ChecksumPath."
    }

    $actualHash = (Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash
    if (-not $actualHash.Equals(
            $checksumMatch.Groups['hash'].Value,
            [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "SHA-256 verification failed for $fileName."
    }

    return $actualHash.ToLowerInvariant()
}

function Get-StreamSha256 {
    param(
        [Parameter(Mandatory = $true)]
        [System.IO.Stream]$Stream
    )

    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hashBytes = $sha256.ComputeHash($Stream)
        return -join ($hashBytes | ForEach-Object { $_.ToString('x2') })
    } finally {
        $sha256.Dispose()
    }
}

function Test-ForbiddenArchiveEntry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$EntryName
    )

    $normalized = $EntryName.Replace('\', '/').TrimStart('/')
    if ($normalized -match '(^|/)\.\.(/|$)') {
        return $true
    }
    if ($normalized -match '(^|/)(\.git|\.github|\.idea|\.vscode|target|debug|artifacts|manifests|logs?|diagnostics?|cache|temp)(/|$)') {
        return $true
    }

    $leafName = [System.IO.Path]::GetFileName($normalized).ToLowerInvariant()
    $extension = [System.IO.Path]::GetExtension($leafName).ToLowerInvariant()
    if ($extension -in @('.pdb', '.log', '.tmp', '.temp', '.bak', '.dmp', '.diag', '.user', '.suo')) {
        return $true
    }
    if ($leafName -in @('settings.json', 'auth.json', '.credentials.json', 'credentials.json', 'credentials.dat', 'secrets.json')) {
        return $true
    }
    if ($leafName -match '(token|credential|secret)' -and $leafName -ne 'codex-usage-monitor.exe') {
        return $true
    }
    if ($extension -eq '.exe' -and $leafName -ne 'codex-usage-monitor.exe') {
        return $true
    }

    return $false
}

$package = Get-RootPackageMetadata
$version = [string]$package.version
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Release version must use MAJOR.MINOR.PATCH: $version"
}
if (-not [string]::IsNullOrWhiteSpace($ExpectedVersion) -and $ExpectedVersion -cne $version) {
    throw "Expected version $ExpectedVersion does not match Cargo version $version."
}
if (-not (Test-Path -LiteralPath $outputPath -PathType Container)) {
    throw "Release output directory does not exist: $outputPath"
}

$archiveName = "CodexUsageMonitor-$version-Windows-x64.zip"
$expectedPaths = [ordered]@{
    Executable         = Join-Path $outputPath 'codex-usage-monitor.exe'
    ExecutableChecksum = Join-Path $outputPath 'codex-usage-monitor.exe.sha256'
    Archive            = Join-Path $outputPath $archiveName
    ArchiveChecksum    = Join-Path $outputPath "$archiveName.sha256"
}
foreach ($entry in $expectedPaths.GetEnumerator()) {
    if (-not (Test-Path -LiteralPath $entry.Value -PathType Leaf)) {
        throw "Missing release asset ($($entry.Key)): $($entry.Value)"
    }
}

$executable = Get-Item -LiteralPath $expectedPaths.Executable
$peStream = [System.IO.File]::OpenRead($executable.FullName)
$peReader = [System.IO.BinaryReader]::new($peStream)
try {
    if ($peStream.Length -lt 64 -or $peReader.ReadUInt16() -ne 0x5a4d) {
        throw 'The release executable does not have a valid DOS/PE header.'
    }
    $peStream.Position = 0x3c
    $peOffset = $peReader.ReadInt32()
    if ($peOffset -lt 64 -or ($peOffset + 6) -gt $peStream.Length) {
        throw 'The release executable has an invalid PE header offset.'
    }
    $peStream.Position = $peOffset
    if ($peReader.ReadUInt32() -ne 0x00004550) {
        throw 'The release executable has an invalid PE signature.'
    }
    if ($peReader.ReadUInt16() -ne 0x8664) {
        throw 'The release executable is not an AMD64 (Windows x64) binary.'
    }
} finally {
    $peReader.Dispose()
}

$versionInfo = $executable.VersionInfo
Assert-PeVersion -Name 'FileVersion' -Value ([string]$versionInfo.FileVersion) -Version $version
Assert-PeVersion -Name 'ProductVersion' -Value ([string]$versionInfo.ProductVersion) -Version $version

$winres = $package.metadata.winres
if ($null -eq $winres) {
    throw 'Cargo package metadata does not define package.metadata.winres.'
}
foreach ($field in @('CompanyName', 'ProductName', 'FileDescription', 'OriginalFilename', 'LegalCopyright')) {
    $expectedValue = [string]$winres.$field
    $actualValue = [string]$versionInfo.$field
    if ([string]::IsNullOrWhiteSpace($expectedValue)) {
        throw "Cargo package metadata is missing winres.$field."
    }
    if ($actualValue -cne $expectedValue) {
        throw "PE $field '$actualValue' does not match Cargo metadata '$expectedValue'."
    }
}

Add-Type -AssemblyName System.Drawing
$associatedIcon = [System.Drawing.Icon]::ExtractAssociatedIcon($executable.FullName)
if ($null -eq $associatedIcon) {
    throw 'The release executable does not expose an associated icon.'
}
$sourceIconPath = Join-Path $repositoryRoot 'src\icons\icon.ico'
if (-not (Test-Path -LiteralPath $sourceIconPath -PathType Leaf)) {
    throw "The source application icon does not exist: $sourceIconPath"
}
$sourceIcon = $null
$associatedBitmap = $null
$sourceBitmap = $null
try {
    if ($associatedIcon.Width -le 0 -or $associatedIcon.Height -le 0) {
        throw 'The release executable associated icon has invalid dimensions.'
    }
    $sourceIcon = [System.Drawing.Icon]::new($sourceIconPath, $associatedIcon.Size)
    $associatedBitmap = $associatedIcon.ToBitmap()
    $sourceBitmap = $sourceIcon.ToBitmap()
    for ($y = 0; $y -lt $associatedBitmap.Height; $y++) {
        for ($x = 0; $x -lt $associatedBitmap.Width; $x++) {
            if ($associatedBitmap.GetPixel($x, $y).ToArgb() -ne $sourceBitmap.GetPixel($x, $y).ToArgb()) {
                throw 'The release executable icon does not match src/icons/icon.ico.'
            }
        }
    }
} finally {
    if ($null -ne $sourceBitmap) {
        $sourceBitmap.Dispose()
    }
    if ($null -ne $associatedBitmap) {
        $associatedBitmap.Dispose()
    }
    if ($null -ne $sourceIcon) {
        $sourceIcon.Dispose()
    }
    $associatedIcon.Dispose()
}

$executableHash = Assert-Checksum `
    -FilePath $expectedPaths.Executable `
    -ChecksumPath $expectedPaths.ExecutableChecksum
$archiveHash = Assert-Checksum `
    -FilePath $expectedPaths.Archive `
    -ChecksumPath $expectedPaths.ArchiveChecksum

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($expectedPaths.Archive)
try {
    $requiredEntries = @('codex-usage-monitor.exe', 'LICENSE', 'README.md', 'README_RU.md')
    $allEntryNames = @($archive.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    if ($allEntryNames.Count -ne $requiredEntries.Count) {
        throw "The release archive must contain exactly $($requiredEntries.Count) entries."
    }

    $fileEntries = @($archive.Entries | Where-Object { -not [string]::IsNullOrEmpty($_.Name) })
    $entryNames = @($fileEntries | ForEach-Object { $_.FullName.Replace('\', '/') })
    foreach ($requiredEntry in $requiredEntries) {
        $matches = @($entryNames | Where-Object { $_ -ceq $requiredEntry })
        if ($matches.Count -ne 1) {
            throw "The release archive must contain exactly one '$requiredEntry' entry."
        }
    }

    $forbiddenEntries = @($entryNames | Where-Object { Test-ForbiddenArchiveEntry $_ })
    if ($forbiddenEntries.Count -gt 0) {
        throw "The release archive contains forbidden entries: $($forbiddenEntries -join ', ')"
    }

    $archiveExecutables = @($entryNames | Where-Object {
        [System.IO.Path]::GetExtension($_).Equals('.exe', [System.StringComparison]::OrdinalIgnoreCase)
    })
    if ($archiveExecutables.Count -ne 1 -or $archiveExecutables[0] -cne 'codex-usage-monitor.exe') {
        throw 'The release archive must contain exactly one executable named codex-usage-monitor.exe.'
    }

    $entrySources = [ordered]@{
        'codex-usage-monitor.exe' = $expectedPaths.Executable
        'LICENSE'                 = Join-Path $repositoryRoot 'LICENSE'
        'README.md'               = Join-Path $repositoryRoot 'README.md'
        'README_RU.md'            = Join-Path $repositoryRoot 'README_RU.md'
    }
    foreach ($entrySource in $entrySources.GetEnumerator()) {
        $archiveEntry = $fileEntries | Where-Object {
            $_.FullName.Replace('\', '/') -ceq $entrySource.Key
        } | Select-Object -First 1
        $entryStream = $archiveEntry.Open()
        try {
            $entryHash = Get-StreamSha256 -Stream $entryStream
        } finally {
            $entryStream.Dispose()
        }
        $sourceHash = (Get-FileHash -LiteralPath $entrySource.Value -Algorithm SHA256).Hash
        if (-not $entryHash.Equals($sourceHash, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "The '$($entrySource.Key)' ZIP entry is not byte-identical to its verified source file."
        }
    }
} finally {
    $archive.Dispose()
}

Write-Output "Verified version: $version"
Write-Output "Executable SHA256: $executableHash"
Write-Output "Archive SHA256: $archiveHash"
Write-Output "Archive: $archiveName"
