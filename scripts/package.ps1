[CmdletBinding()]
param(
    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string]$BuildDirectory = 'target',

    [Parameter()]
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Release',

    [Parameter()]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDirectory = 'artifacts'
)

$ErrorActionPreference = 'Stop'

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$buildPath = if ([System.IO.Path]::IsPathRooted($BuildDirectory)) {
    [System.IO.Path]::GetFullPath($BuildDirectory)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $BuildDirectory))
}
$outputPath = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    [System.IO.Path]::GetFullPath($OutputDirectory)
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $OutputDirectory))
}

if (-not (Test-Path -LiteralPath $buildPath -PathType Container)) {
    throw "Build directory does not exist: $buildPath"
}
if ($outputPath.Equals($repositoryRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'The package output directory must not be the repository root.'
}

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
$version = [string]$packages[0].version
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "Release version must use MAJOR.MINOR.PATCH: $version"
}

$configurationDirectory = $Configuration.ToLowerInvariant()
$sourceExecutable = Join-Path $buildPath "$configurationDirectory\codex-usage-monitor.exe"
if (-not (Test-Path -LiteralPath $sourceExecutable -PathType Leaf)) {
    throw "Built executable does not exist: $sourceExecutable"
}

$requiredRepositoryFiles = @('LICENSE', 'README.md', 'README_RU.md')
foreach ($relativePath in $requiredRepositoryFiles) {
    $requiredPath = Join-Path $repositoryRoot $relativePath
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Required package file is missing: $relativePath"
    }
}

New-Item -ItemType Directory -Force -Path $outputPath | Out-Null
$stagingPath = [System.IO.Path]::GetFullPath((Join-Path $outputPath ('.package-' + [guid]::NewGuid().ToString('N'))))
$outputPrefix = $outputPath.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if (-not $stagingPath.StartsWith($outputPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe package staging path: $stagingPath"
}

$archiveName = "CodexUsageMonitor-$version-Windows-x64.zip"
$rawExecutablePath = Join-Path $outputPath 'codex-usage-monitor.exe'
$rawChecksumPath = Join-Path $outputPath 'codex-usage-monitor.exe.sha256'
$archivePath = Join-Path $outputPath $archiveName
$archiveChecksumPath = Join-Path $outputPath "$archiveName.sha256"

New-Item -ItemType Directory -Path $stagingPath | Out-Null
try {
    Copy-Item -LiteralPath $sourceExecutable -Destination (Join-Path $stagingPath 'codex-usage-monitor.exe')
    foreach ($relativePath in $requiredRepositoryFiles) {
        Copy-Item -LiteralPath (Join-Path $repositoryRoot $relativePath) -Destination $stagingPath
    }

    Copy-Item -LiteralPath $sourceExecutable -Destination $rawExecutablePath -Force
    $rawHash = (Get-FileHash -LiteralPath $rawExecutablePath -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $rawChecksumPath -Value "$rawHash *codex-usage-monitor.exe" -Encoding ascii

    if (Test-Path -LiteralPath $archivePath) {
        Remove-Item -LiteralPath $archivePath -Force
    }
    if (Test-Path -LiteralPath $archiveChecksumPath) {
        Remove-Item -LiteralPath $archiveChecksumPath -Force
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory(
        $stagingPath,
        $archivePath,
        [System.IO.Compression.CompressionLevel]::Optimal,
        $false
    )

    $archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath $archiveChecksumPath -Value "$archiveHash *$archiveName" -Encoding ascii
} finally {
    if (Test-Path -LiteralPath $stagingPath) {
        Remove-Item -LiteralPath $stagingPath -Recurse -Force
    }
}

& (Join-Path $PSScriptRoot 'verify-release.ps1') `
    -OutputDirectory $outputPath `
    -ExpectedVersion $version

Write-Output "Executable: $rawExecutablePath"
Write-Output "Executable checksum: $rawChecksumPath"
Write-Output "Archive: $archivePath"
Write-Output "Archive checksum: $archiveChecksumPath"
