#!/usr/bin/env pwsh
# install.ps1 - installer for shelfbox
#
# Usage:
#   irm https://raw.githubusercontent.com/massa-kj/shelfbox/main/scripts/install.ps1 | iex
#
# Parameters:
#   -Version     Tag to install (e.g. v0.1.0). Defaults to the latest release.
#   -InstallDir  Directory to place the binary. Defaults to %LOCALAPPDATA%\Programs\shelfbox\bin.

[CmdletBinding()]
param(
    [string]$Version = $env:VERSION,
    [string]$InstallDir = $env:INSTALL_DIR
)

$ErrorActionPreference = "Stop"

$Repo = "massa-kj/shelfbox"
$Binary = "shelfbox"

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $LocalAppData = [Environment]::GetFolderPath("LocalApplicationData")
    if ([string]::IsNullOrWhiteSpace($LocalAppData)) {
        $LocalAppData = Join-Path $HOME "AppData\Local"
    }
    $InstallDir = Join-Path $LocalAppData "Programs\shelfbox\bin"
}

switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { $Arch = "x86_64" }
    default {
        throw "unsupported architecture: $env:PROCESSOR_ARCHITECTURE"
    }
}

$Target = "$Arch-pc-windows-msvc"

if ([string]::IsNullOrWhiteSpace($Version)) {
    Write-Host "Fetching latest release version..."
    $Latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $Latest.tag_name
    if ([string]::IsNullOrWhiteSpace($Version)) {
        throw "could not determine the latest release version. Set VERSION and try again."
    }
}

$Archive = "$Binary-$Version-$Target.zip"
$Checksum = "$Archive.sha256"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"
$TempDir = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())

New-Item -ItemType Directory -Path $TempDir | Out-Null

try {
    $ArchivePath = Join-Path $TempDir $Archive
    $ChecksumPath = Join-Path $TempDir $Checksum

    Write-Host "Downloading $Binary $Version for $Target..."
    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ArchivePath
    Invoke-WebRequest -Uri "$BaseUrl/$Checksum" -OutFile $ChecksumPath

    Write-Host "Verifying checksum..."
    $ExpectedHash = ((Get-Content $ChecksumPath -Raw).Trim() -split "\s+")[0].ToUpperInvariant()
    $ActualHash = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToUpperInvariant()
    if ($ActualHash -ne $ExpectedHash) {
        throw "checksum mismatch for $Archive"
    }
    Write-Host "Checksum OK."

    Write-Host "Extracting..."
    Expand-Archive -Path $ArchivePath -DestinationPath $TempDir -Force

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item `
        -Path (Join-Path $TempDir "$Binary-$Version-$Target\$Binary.exe") `
        -Destination (Join-Path $InstallDir "$Binary.exe") `
        -Force

    Write-Host ""
    Write-Host "Installed $Binary $Version -> $(Join-Path $InstallDir "$Binary.exe")"

    $PathEntries = [Environment]::GetEnvironmentVariable("Path", "User") -split ";"
    if ($PathEntries -notcontains $InstallDir) {
        Write-Host ""
        Write-Host "NOTE: $InstallDir is not in your user PATH."
        Write-Host "      Add it from Windows Settings or run:"
        Write-Host ""
        Write-Host "        [Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';$InstallDir', 'User')"
        Write-Host ""
    }
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
