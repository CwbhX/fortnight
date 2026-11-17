# Fails if a built Windows executable imports anything but system DLLs.
# PLAN.md section 3.1. The C runtime is linked statically (.cargo/config.toml),
# so vcruntime*.dll, msvcp*.dll, ucrtbase.dll and the api-ms-win-crt-* forwarders
# must NOT appear.
#
#   ci/check-deps.ps1 <path\to\binary.exe>
#
# Needs `dumpbin` on PATH (run from a Developer PowerShell, or after
# ilammy/msvc-dev-cmd in GitHub Actions).
param(
    [Parameter(Mandatory = $true)][string]$Binary
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $Binary)) {
    # Write-Host, not Write-Error: under 'Stop', Write-Error ends the script
    # with exit code 1 before `exit 2` runs.
    Write-Host "check-deps: $Binary does not exist"
    exit 2
}

# Windows system DLLs that exist on every install. Lower case.
$allowed = @(
    'kernel32.dll', 'kernelbase.dll', 'ntdll.dll', 'user32.dll', 'gdi32.dll',
    'advapi32.dll', 'shell32.dll', 'ole32.dll', 'oleaut32.dll', 'combase.dll',
    'ws2_32.dll', 'mswsock.dll', 'bcrypt.dll', 'ncrypt.dll', 'crypt32.dll',
    'secur32.dll', 'userenv.dll', 'dwmapi.dll', 'shcore.dll', 'comdlg32.dll',
    'comctl32.dll', 'imm32.dll', 'winmm.dll', 'uxtheme.dll', 'version.dll',
    'setupapi.dll', 'cfgmgr32.dll', 'propsys.dll', 'rpcrt4.dll', 'iphlpapi.dll',
    'dnsapi.dll', 'winspool.drv', 'opengl32.dll', 'd3d11.dll', 'd3d12.dll',
    'dxgi.dll', 'd3dcompiler_47.dll', 'dcomp.dll', 'ntmarta.dll', 'wldap32.dll',
    'normaliz.dll', 'shlwapi.dll', 'psapi.dll', 'powrprof.dll', 'synchronization.dll'
)

$out = & dumpbin /dependents $Binary
if ($LASTEXITCODE -ne 0) {
    Write-Host "check-deps: dumpbin failed on $Binary"
    exit 2
}

# dumpbin lists dependents as indented "name.dll" lines between the
# "Image has the following dependencies:" header and the "Summary" section.
$deps = @()
$inList = $false
foreach ($line in $out) {
    if ($line -match 'Image has the following dependencies') { $inList = $true; continue }
    if ($line -match '^\s*Summary') { break }
    if ($inList -and $line -match '^\s+(\S+\.(dll|drv))\s*$') { $deps += $Matches[1] }
}

if ($deps.Count -eq 0) {
    Write-Host "check-deps: could not read any dependents from dumpbin output for $Binary"
    exit 2
}

$bad = @()
foreach ($d in $deps) {
    $name = $d.ToLowerInvariant()
    # Anything not on the allow-list fails, including the C runtime DLLs.
    if ($allowed -notcontains $name) { $bad += $d }
}

if ($bad.Count -ne 0) {
    Write-Host "check-deps: FAIL: $Binary has dependencies outside the Windows allow-list:"
    foreach ($b in $bad) { Write-Host "  - $b" }
    Write-Host "vcruntime/msvcp/ucrt imports mean +crt-static did not take effect (is RUSTFLAGS set?)."
    exit 1
}

Write-Host "check-deps: OK: $Binary ($($deps -join ', '))"
