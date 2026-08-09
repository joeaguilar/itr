# Smoke-tests install.ps1 end to end under whichever PowerShell is running it.
# .github/workflows/installer.yml invokes this twice: once with
# `shell: powershell` (Windows PowerShell 5.1) and once with `shell: pwsh`
# (PowerShell 7.x). Keep this file ASCII-only and free of 7.x-only syntax.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Tool
)

$ErrorActionPreference = 'Stop'

$runtime = $PSVersionTable.PSVersion.ToString()
Write-Host "== install.ps1 smoke test on PowerShell $runtime =="

$script:failures = 0
function Assert-That {
    param([string]$Label, [bool]$Condition)
    if ($Condition) {
        Write-Host "  PASS  $Label"
    } else {
        Write-Host "::error::[PowerShell $runtime] $Label"
        $script:failures++
    }
}

$dir = Join-Path $env:RUNNER_TEMP "smoke-$Tool-$($PSVersionTable.PSVersion.Major)"
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
Set-Item -Path ('Env:' + $Tool.ToUpper() + '_INSTALL_DIR') -Value $dir

# Drive the documented `iex` path rather than `& install.ps1`. iex executes in
# this scope, which is the only place a leaked $ProgressPreference shows up --
# invoking the script directly would give it its own scope and hide the bug.
$code = [IO.File]::ReadAllText((Join-Path $PWD 'install.ps1'), [Text.Encoding]::UTF8)

$before = $ProgressPreference
$first = ''
try {
    $first = (Invoke-Expression $code) *>&1 | Out-String -Width 500
} catch {
    Write-Host "::error::[PowerShell $runtime] install.ps1 threw: $($_.Exception.Message)"
    $script:failures++
}
Write-Host $first

# No -Version pin, so this also covers release-tag resolution, which has to
# work identically on 5.1 and 7.x despite their redirect handling differing.
Assert-That "resolves the latest tag and verifies the checksum" ($first -match 'checksum verified')
Assert-That "restores `$ProgressPreference" ($ProgressPreference -eq $before)

$exe = Join-Path $dir "$Tool.exe"
Assert-That "installs $Tool.exe" (Test-Path $exe)
if (Test-Path $exe) {
    Write-Host ("  installed: " + (& $exe --version))
    Assert-That "installed binary runs" ($LASTEXITCODE -eq 0)
}

# Re-run: the directory is already on PATH now, so it must be neither warned
# about nor added a second time. Both have regressed here before.
$second = ''
try {
    $second = (Invoke-Expression $code) *>&1 | Out-String -Width 500
} catch {
    Write-Host "::error::[PowerShell $runtime] second run threw: $($_.Exception.Message)"
    $script:failures++
}
Assert-That "second run does not claim the dir is missing from PATH" ($second -notmatch 'is not in PATH')
Assert-That "second run does not re-add the PATH entry" ($second -notmatch 'to your User PATH')

$entries = @((([Environment]::GetEnvironmentVariable('Path', 'User')) -split ';') |
    Where-Object { $_ -and $_.TrimEnd('\') -ieq $dir.TrimEnd('\') })
Assert-That "leaves exactly one PATH entry (found $($entries.Count))" ($entries.Count -eq 1)

if ($script:failures -gt 0) {
    Write-Host "$($script:failures) check(s) failed on PowerShell $runtime"
    exit 1
}
Write-Host "all checks passed on PowerShell $runtime"
