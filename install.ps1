<#
.SYNOPSIS
    Install the itr CLI on Windows from the latest GitHub Release.

.DESCRIPTION
    Downloads a prebuilt itr binary matching the host architecture
    (x86_64 or arm64), verifies its SHA256 checksum, and installs it
    into a directory on PATH.

.PARAMETER Version
    Pin a specific release tag (e.g. v0.1.0). Defaults to the latest.

.PARAMETER InstallDir
    Install location. Defaults to $env:LOCALAPPDATA\Programs\itr.

.PARAMETER Repo
    GitHub repo slug. Defaults to joeaguilar/itr.

.EXAMPLE
    iwr -useb https://raw.githubusercontent.com/joeaguilar/itr/main/install.ps1 | iex

.EXAMPLE
    .\install.ps1 -Version v0.1.0 -InstallDir C:\tools\itr

.NOTES
    Manual checksum verification:
    - In Windows PowerShell 5.1 and PowerShell 7, run against a test release
      that contains the itr zip asset but no .sha256 asset. The installer should
      warn "Checksum file not available" and continue.
    - Run against a test release with an incorrect .sha256 asset. The installer
      should fail with "Checksum mismatch" before extraction or installation.
#>

[CmdletBinding()]
param(
    [string]$Version = $env:ITR_VERSION,
    [string]$InstallDir = $env:ITR_INSTALL_DIR,
    [string]$Repo = $(if ($env:ITR_REPO) { $env:ITR_REPO } else { 'joeaguilar/itr' })
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Info    { param([string]$m) Write-Host "i $m" -ForegroundColor Blue }
function Write-Ok      { param([string]$m) Write-Host "+ $m" -ForegroundColor Green }
function Write-Warn    { param([string]$m) Write-Host "! $m" -ForegroundColor Yellow }
function Write-Err     { param([string]$m) Write-Host "x $m" -ForegroundColor Red }

function Get-Target {
    $arch = $env:PROCESSOR_ARCHITECTURE
    if (-not $arch) { $arch = (Get-CimInstance Win32_Processor).Architecture }
    switch -Regex ($arch) {
        '^(AMD64|x86_64|9)$' { return 'x86_64-pc-windows-msvc' }
        '^(ARM64|12)$'       { return 'aarch64-pc-windows-msvc' }
        default {
            throw "Unsupported architecture: $arch"
        }
    }
}

function Resolve-LatestTag {
    param([string]$Repo)
    # Follow the /releases/latest redirect to avoid the API rate limit.
    $url = "https://github.com/$Repo/releases/latest"
    $resp = Invoke-WebRequest -Uri $url -MaximumRedirection 0 -ErrorAction SilentlyContinue
    if ($resp.StatusCode -ne 302 -and $resp.StatusCode -ne 301) {
        # PowerShell 7 may have followed the redirect; pull from the final URI.
        if ($resp.BaseResponse.RequestMessage.RequestUri) {
            $final = $resp.BaseResponse.RequestMessage.RequestUri.AbsoluteUri
            return ($final -split '/')[-1]
        }
        throw "Could not resolve latest release tag from $url"
    }
    $location = $resp.Headers.Location
    return ($location -split '/')[-1]
}

function Add-ToUserPath {
    param([string]$Dir)
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $current) { $current = '' }
    $parts = $current -split ';' | Where-Object { $_ -ne '' }
    if ($parts -contains $Dir) { return $false }
    $new = (@($Dir) + $parts) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $new, 'User')
    # Make it visible to the current session too.
    $env:Path = "$Dir;$env:Path"
    return $true
}

function Test-InPath {
    param([string]$Dir)
    $parts = $env:Path -split ';' | Where-Object { $_ -ne '' }
    return ($parts -contains $Dir)
}

# ---- Main ------------------------------------------------------------------

Write-Host ''
Write-Info 'Installing itr — the zero-config issue tracker CLI'
Write-Host ''

$target = Get-Target
Write-Info "Detected target: $target"

if (-not $Version) {
    $Version = Resolve-LatestTag -Repo $Repo
}
Write-Info "Release: $Version"

if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\itr'
}
$InstallDir = [Environment]::ExpandEnvironmentVariables($InstallDir)

$assetBase = "itr-$Version-$target"
$zipUrl    = "https://github.com/$Repo/releases/download/$Version/$assetBase.zip"
$sumUrl    = "$zipUrl.sha256"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    $zipPath = Join-Path $tmp "$assetBase.zip"
    $sumPath = Join-Path $tmp "$assetBase.zip.sha256"

    Write-Info "Downloading $assetBase.zip"
    Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing

    $hasChecksum = $true
    try {
        Invoke-WebRequest -Uri $sumUrl -OutFile $sumPath -UseBasicParsing -ErrorAction Stop
    } catch {
        $statusCode = $null
        if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
            $statusCode = [int]$_.Exception.Response.StatusCode
        }
        if ($statusCode -eq 404) {
            $hasChecksum = $false
            Write-Warn "Checksum file not available (HTTP 404) — skipping verification."
        } else {
            throw
        }
    }

    if ($hasChecksum) {
        $expected = (Get-Content $sumPath -Raw).Trim().Split()[0].ToLower()
        $actual   = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
        if ($expected -ne $actual) {
            throw "Checksum mismatch: expected $expected, got $actual"
        }
        Write-Ok 'Checksum verified.'
    }

    Write-Info 'Extracting…'
    Expand-Archive -Path $zipPath -DestinationPath $tmp -Force

    $binSrc = Join-Path $tmp "$assetBase\itr.exe"
    if (-not (Test-Path $binSrc)) {
        throw "Extracted archive is missing itr.exe"
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    }
    $binDst = Join-Path $InstallDir 'itr.exe'
    Copy-Item -Force $binSrc $binDst
    Write-Ok "Installed $binDst"

    if (-not (Test-InPath $InstallDir)) {
        $added = Add-ToUserPath -Dir $InstallDir
        if ($added) {
            Write-Ok "Added $InstallDir to your User PATH (restart your shell to pick it up)."
        } else {
            Write-Warn "$InstallDir is not in PATH; add it manually if needed."
        }
    }

    Write-Host ''
    try { & $binDst --version } catch { }
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Ok 'Done.'
Write-Host ''
Write-Info 'Quick start:'
Write-Host '  itr init              # initialize an issue tracker in the current dir'
Write-Host '  itr add "My task"     # create an issue'
Write-Host '  itr ready             # list unblocked issues'
Write-Host '  itr --help            # all commands'
Write-Host ''
