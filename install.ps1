<#
.SYNOPSIS
    Install or update the itr CLI on Windows from the latest GitHub Release.

.DESCRIPTION
    Downloads a prebuilt itr binary matching the host architecture
    (x86_64 or arm64), verifies its SHA256 checksum, and installs it
    into a directory on PATH.

    When -Update is supplied (or the positional `update` argument), the
    installer prefers an existing itr.exe already on PATH and replaces it
    in-place rather than always writing to $InstallDir. This mirrors the
    `install.sh --update` behavior on Unix.

.PARAMETER Version
    Pin a specific release tag (e.g. v0.1.0). Defaults to the latest.

.PARAMETER InstallDir
    Install location. Defaults to $env:LOCALAPPDATA\Programs\itr. When
    -Update is set and an existing itr.exe is found on PATH, that location
    wins over the default so the shell keeps resolving the same binary.

.PARAMETER Repo
    GitHub repo slug. Defaults to joeaguilar/itr.

.PARAMETER Update
    Update an existing itr install if found on PATH; otherwise install it.
    The positional value `update` (or `install`) is also accepted, e.g.
    `.\install.ps1 update`.

.EXAMPLE
    iwr -useb https://raw.githubusercontent.com/joeaguilar/itr/main/install.ps1 | iex

.EXAMPLE
    .\install.ps1 -Version v0.1.0 -InstallDir C:\tools\itr

.EXAMPLE
    .\install.ps1 -Update

.EXAMPLE
    .\install.ps1 update

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
    [string]$Repo = $(if ($env:ITR_REPO) { $env:ITR_REPO } else { 'joeaguilar/itr' }),
    [switch]$Update,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Action
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Write-Info    { param([string]$m) Write-Host "i $m" -ForegroundColor Blue }
function Write-Ok      { param([string]$m) Write-Host "+ $m" -ForegroundColor Green }
function Write-Warn    { param([string]$m) Write-Host "! $m" -ForegroundColor Yellow }
function Write-Err     { param([string]$m) Write-Host "x $m" -ForegroundColor Red }

function Get-ItrPowerShellRuntime {
    $major = [int]$PSVersionTable.PSVersion.Major
    $minor = [int]$PSVersionTable.PSVersion.Minor
    if ($major -lt 5 -or ($major -eq 5 -and $minor -lt 1)) {
        throw "PowerShell 5.1 or newer is required. Current version: $($PSVersionTable.PSVersion)"
    }
    if ($major -ge 7) {
        return 'powershell-7'
    }
    return 'windows-powershell-5.1'
}

function Initialize-ItrPowerShellRuntime {
    param([string]$Runtime)
    if ($Runtime -eq 'windows-powershell-5.1') {
        # Windows PowerShell 5.1 can default to older TLS settings on some hosts.
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    }
}

function Invoke-ItrWebRequestWindowsPowerShell51 {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [string]$OutFile,
        [int]$MaximumRedirection = -1,
        [switch]$AllowHttpErrorStatus
    )
    $params = @{
        Uri = $Uri
        UseBasicParsing = $true
        ErrorAction = if ($AllowHttpErrorStatus) { 'SilentlyContinue' } else { 'Stop' }
    }
    if ($OutFile) { $params.OutFile = $OutFile }
    if ($MaximumRedirection -ge 0) { $params.MaximumRedirection = $MaximumRedirection }
    return Invoke-WebRequest @params
}

function Invoke-ItrWebRequestPowerShell7 {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [string]$OutFile,
        [int]$MaximumRedirection = -1,
        [switch]$AllowHttpErrorStatus
    )
    $params = @{
        Uri = $Uri
        ErrorAction = if ($AllowHttpErrorStatus) { 'SilentlyContinue' } else { 'Stop' }
    }
    if ($OutFile) { $params.OutFile = $OutFile }
    if ($MaximumRedirection -ge 0) { $params.MaximumRedirection = $MaximumRedirection }
    return Invoke-WebRequest @params
}

function Invoke-ItrWebRequest {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [string]$OutFile,
        [int]$MaximumRedirection = -1,
        [switch]$AllowHttpErrorStatus
    )
    $params = @{
        Uri = $Uri
        MaximumRedirection = $MaximumRedirection
        AllowHttpErrorStatus = $AllowHttpErrorStatus
    }
    if ($OutFile) { $params.OutFile = $OutFile }
    if ($script:ItrPowerShellRuntime -eq 'powershell-7') {
        return Invoke-ItrWebRequestPowerShell7 @params
    }
    return Invoke-ItrWebRequestWindowsPowerShell51 @params
}

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
    $tag = $null

    # HttpWebRequest with redirects disabled is the one mechanism that behaves
    # identically on Windows PowerShell 5.1 and PowerShell 7+. Invoke-WebRequest
    # cannot cover both here: 5.1 needs -UseBasicParsing to avoid the IE parser,
    # while 7+ throws "Response status code does not indicate success: 302
    # (Found)" on -MaximumRedirection 0 unless given -SkipHttpErrorCheck -- a
    # parameter that does not exist on 5.1. -ErrorAction does not suppress it.
    try {
        $req = [System.Net.HttpWebRequest]::Create($url)
        $req.AllowAutoRedirect = $false
        $req.UserAgent = 'itr-installer'
        $resp = $req.GetResponse()
        try {
            $location = $resp.Headers['Location']
            if ($location) { $tag = ($location -split '/')[-1] }
        } finally {
            $resp.Close()
        }
    } catch {
        $tag = $null
    }

    # Fallback: follow the redirect and read the final URI. BaseResponse is an
    # HttpWebResponse on 5.1 (ResponseUri) but an HttpResponseMessage on 7+
    # (RequestMessage.RequestUri), so probe for both rather than assuming.
    if (-not $tag) {
        $resp = Invoke-ItrWebRequest -Uri $url
        $requestUri = $null
        if ($resp.BaseResponse) {
            if ($resp.BaseResponse | Get-Member -Name RequestMessage -MemberType Property) {
                $requestUri = $resp.BaseResponse.RequestMessage.RequestUri
            } elseif ($resp.BaseResponse | Get-Member -Name ResponseUri -MemberType Property) {
                $requestUri = $resp.BaseResponse.ResponseUri
            }
        }
        if (-not $requestUri) {
            throw "Could not resolve latest release tag from $url"
        }
        $tag = ($requestUri.AbsoluteUri -split '/')[-1]
    }
    # When a repo has no published releases, GitHub redirects /releases/latest
    # to /releases, so the last URL segment is the literal string "releases"
    # rather than a real tag. Detect that and fail loudly instead of building
    # a bogus download URL.
    if (-not $tag -or $tag -eq 'releases') {
        throw "No published releases found at https://github.com/$Repo/releases. Publish a release (e.g. v0.1.0) with the Windows zip + sha256 assets, or pin a tag with -Version."
    }
    return $tag
}

function Get-PathEntries {
    param([string]$Scope)
    $v = [Environment]::GetEnvironmentVariable('Path', $Scope)
    if (-not $v) { return @() }
    return @($v -split ';' | Where-Object { $_ -ne '' })
}

# Compare against both User and Machine PATH so a directory already on the
# system PATH is never duplicated into the user scope. Entries are matched
# case-insensitively and ignoring a trailing separator.
function Test-OnPersistentPath {
    param([string]$Dir)
    $target = $Dir.TrimEnd('\')
    foreach ($e in (@(Get-PathEntries 'User') + @(Get-PathEntries 'Machine'))) {
        if ($e.TrimEnd('\') -ieq $target) { return $true }
    }
    return $false
}

# Make itr resolvable for the rest of this session as well.
function Add-ToSessionPath {
    param([string]$Dir)
    $target = $Dir.TrimEnd('\')
    foreach ($e in ($env:Path -split ';' | Where-Object { $_ -ne '' })) {
        if ($e.TrimEnd('\') -ieq $target) { return }
    }
    $env:Path = "$Dir;$env:Path"
}

function Add-ToUserPath {
    param([string]$Dir)
    if (Test-OnPersistentPath $Dir) { return $false }
    [Environment]::SetEnvironmentVariable(
        'Path', ((@($Dir) + (Get-PathEntries 'User')) -join ';'), 'User')
    return $true
}

function Get-ExistingItrPath {
    # Resolve the itr.exe that the current shell would invoke, if any.
    $cmd = Get-Command itr -ErrorAction SilentlyContinue
    if ($cmd -and $cmd.Path -and (Test-Path $cmd.Path)) {
        return $cmd.Path
    }
    return $null
}

function Get-ExistingItrInDir {
    param([string]$Dir)
    if (-not $Dir) { return $null }
    $candidate = Join-Path $Dir 'itr.exe'
    if (Test-Path $candidate) { return $candidate }
    return Get-ExistingItrPath
}

function Show-ExistingVersion {
    param([string]$BinPath)
    if (-not $BinPath) { return }
    try {
        $ver = & $BinPath --version 2>$null
        if ($ver) {
            Write-Info "Current install: $ver ($BinPath)"
        } else {
            Write-Info "Current install: $BinPath"
        }
    } catch {
        Write-Info "Current install: $BinPath"
    }
}

# ---- Argument normalization ------------------------------------------------

# Accept both `-Update` switch and a positional `update`/`install` token to
# mirror install.sh's `--update` / `update` / `--install` / `install` parsing.
$ActionMode = if ($Update) { 'update' } else { 'install' }
if ($Action) {
    foreach ($arg in $Action) {
        switch -Regex ($arg) {
            '^(--update|update)$'   { $ActionMode = 'update' }
            '^(--install|install)$' { $ActionMode = 'install' }
            '^(-h|--help)$' {
                $scriptPath = $MyInvocation.MyCommand.Path
                if ($scriptPath -and (Test-Path $scriptPath)) {
                    Get-Help $scriptPath -Detailed
                } else {
                    Write-Host 'Usage:'
                    Write-Host '  .\install.ps1 [-Update] [-Version <tag>] [-InstallDir <path>] [-Repo <slug>]'
                    Write-Host '  .\install.ps1 update      # positional form, mirrors install.sh'
                    Write-Host ''
                    Write-Host 'Environment overrides:'
                    Write-Host '  ITR_VERSION       Pin a specific release tag (defaults to latest).'
                    Write-Host '  ITR_INSTALL_DIR   Override the install directory.'
                    Write-Host '  ITR_REPO          Override the GitHub repo slug.'
                }
                exit 0
            }
            default {
                Write-Err "Unknown argument: $arg"
                exit 1
            }
        }
    }
}

# ---- Main ------------------------------------------------------------------

Write-Host ''
$script:ItrPowerShellRuntime = Get-ItrPowerShellRuntime
Initialize-ItrPowerShellRuntime -Runtime $script:ItrPowerShellRuntime
Write-Info "PowerShell runtime: $script:ItrPowerShellRuntime ($($PSVersionTable.PSVersion))"

if ($ActionMode -eq 'update') {
    Write-Info 'Updating itr - the zero-config issue tracker CLI'
} else {
    Write-Info 'Installing itr - the zero-config issue tracker CLI'
}
Write-Host ''

$target = Get-Target
Write-Info "Detected target: $target"

if (-not $Version) {
    $Version = Resolve-LatestTag -Repo $Repo
}
Write-Info "Release: $Version"

# Install-dir selection mirrors install.sh::choose_install_dir:
#   1. Explicit -InstallDir / $env:ITR_INSTALL_DIR wins.
#   2. Otherwise, if an existing itr.exe is on PATH, replace it in-place so
#      the shell keeps resolving the same binary (especially for updates,
#      but also for plain installs where the user already has itr).
#   3. Otherwise fall back to $env:LOCALAPPDATA\Programs\itr.
$existingItr = Get-ExistingItrPath
if (-not $InstallDir) {
    if ($existingItr) {
        $InstallDir = Split-Path -Parent $existingItr
        if ($ActionMode -eq 'install') {
            Write-Info "Existing itr.exe found on PATH - installing alongside it at $InstallDir"
        }
    } else {
        $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\itr'
    }
}
$InstallDir = [Environment]::ExpandEnvironmentVariables($InstallDir)

$assetBase = "itr-$Version-$target"
$zipUrl    = "https://github.com/$Repo/releases/download/$Version/$assetBase.zip"
$sumUrl    = "$zipUrl.sha256"

$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

# Rendering the progress bar throttles Invoke-WebRequest badly on Windows
# PowerShell 5.1, so suppress it for the transfer. The caller's value is
# restored below: `iwr | iex` runs this in the user's own session, and leaving
# progress disabled there would be a surprising side effect.
$prevProgress = $ProgressPreference
$ProgressPreference = 'SilentlyContinue'

try {
    $zipPath = Join-Path $tmp "$assetBase.zip"
    $sumPath = Join-Path $tmp "$assetBase.zip.sha256"

    Write-Info "Downloading $assetBase.zip"
    Invoke-ItrWebRequest -Uri $zipUrl -OutFile $zipPath

    $hasChecksum = $true
    try {
        Invoke-ItrWebRequest -Uri $sumUrl -OutFile $sumPath
    } catch {
        $statusCode = $null
        if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
            $statusCode = [int]$_.Exception.Response.StatusCode
        }
        if ($statusCode -eq 404) {
            $hasChecksum = $false
            Write-Warn "Checksum file not available (HTTP 404) - skipping verification."
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

    Write-Info 'Extracting...'
    Expand-Archive -Path $zipPath -DestinationPath $tmp -Force

    $binSrc = Join-Path $tmp "$assetBase\itr.exe"
    if (-not (Test-Path $binSrc)) {
        throw "Extracted archive is missing itr.exe"
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    }
    $binDst = Join-Path $InstallDir 'itr.exe'

    $existingBefore = Get-ExistingItrInDir -Dir $InstallDir
    Show-ExistingVersion -BinPath $existingBefore

    if ($existingBefore) {
        Write-Info "Updating $binDst"
    } else {
        Write-Info "Installing to $InstallDir"
    }
    Copy-Item -Force $binSrc $binDst
    if ($existingBefore) {
        Write-Ok "Updated $binDst"
    } else {
        Write-Ok "Installed $binDst"
    }

    if (Add-ToUserPath -Dir $InstallDir) {
        Write-Ok "Added $InstallDir to your User PATH (restart your shell to pick it up)."
    }
    Add-ToSessionPath -Dir $InstallDir

    Write-Host ''
    try { & $binDst --version } catch { }
} finally {
    $ProgressPreference = $prevProgress
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
