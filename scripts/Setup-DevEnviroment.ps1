#Requires -Version 5.1
<#
.SYNOPSIS
  Installs Windows prerequisites for the Talos monorepo and prepares the apps/ workspace
  (Bun, Rust, Visual Studio C++ build tools for MSVC / link.exe, Windows SDK signtool for Authenticode builds,
  vcpkg libvpx 1.13.1#5 (ports overlay under scripts/vcpkg-overlays) for env-libvpx-sys, LLVM (libclang) for bindgen / yuv-sys,
  .NET SDK (dotnet) for WiX / scripts/build-installers.ps1,
  optional WSL2 + Docker Desktop, dependency install, apps/.env).

.DESCRIPTION
  Run from a normal PowerShell window (right-click: Run with PowerShell, or: powershell -File scripts/setup-dev-environment.ps1).
  Some steps require an elevated shell; Virtual Machine Platform / WSL may require a system reboot
  before Docker Desktop can run Linux containers. After a reboot, run this script again, start Docker
  Desktop, then: cd apps; bun run dev

  Use -SkipLocalContainers (or -SkipWslVmp and -SkipDocker) on hosts where containers run elsewhere
  (e.g. a separate Linux VM or Docker Offload), so this script does not reinstall WSL or Docker Desktop.

.PARAMETER SkipWslVmp
  Do not run `wsl --install --no-distribution` (enables Virtual Machine Platform if missing).

.PARAMETER SkipDocker
  Do not install Docker Desktop via winget.

.PARAMETER SkipLocalContainers
  Turns on both SkipWslVmp and SkipDocker (no local WSL/VMP enablement and no Docker Desktop install).

.PARAMETER ForceEnv
  Overwrite apps/.env from .env.example even if apps/.env already exists (saves a backup first).

.PARAMETER SkipVsBuildTools
  Do not install Visual Studio 2022 Build Tools (C++ / MSVC) via winget, and do not add the MSVC
  link.exe directory to the user PATH. Use when you already have Visual Studio or the Build Tools
  with the C++ workload.

.PARAMETER SkipVcpkgLibVpx
  Do not clone/bootstrap vcpkg under C:\vcpkg or install libvpx for x64-windows / x86-windows.
  Use when libvpx is already available at the paths expected by scripts/build-installers.ps1.

.PARAMETER SkipLLVM
  Do not install LLVM (LLVM.LLVM via winget) for libclang / bindgen. Use when LIBCLANG_PATH is
  already set or LLVM is installed in a custom location.

.PARAMETER SkipWindowsSdk
  Do not install the Windows SDK via winget or add signtool.exe (x64) to your user PATH. Use when
  signtool is already installed (e.g. full Visual Studio with Windows SDK). scripts/build-installers.ps1
  needs signtool for Authenticode signing; it ships under Windows Kits, not in MSVC Hostx64 alone.

.PARAMETER SkipDotNet
  Do not install the .NET SDK via winget. Use when dotnet is already on PATH (e.g. Visual Studio
  or a manual SDK install). Required for WiX projects under apps/installer (dotnet restore).
#>
[CmdletBinding()]
param(
  [switch] $SkipWslVmp,
  [switch] $SkipDocker,
  [switch] $SkipLocalContainers,
  [switch] $ForceEnv,
  [switch] $SkipVsBuildTools,
  [switch] $SkipVcpkgLibVpx,
  [switch] $SkipLLVM,
  [switch] $SkipWindowsSdk,
  [switch] $SkipDotNet
)

$TalosVcpkgRelease = "2026.05.25"
$TalosVcpkgCommit = "d015e31e90838a4c9dfa3eed45979bc70d9357fc"

if ($SkipLocalContainers) {
  $SkipWslVmp = $true
  $SkipDocker = $true
}

$ErrorActionPreference = "Stop"
$RequiredBunVersion = "1.3.14"
$RequiredRustVersion = "1.95.0"
$secretHelperPath = Join-Path $PSScriptRoot "DevEnvironmentSecrets.ps1"
if (-not (Test-Path -LiteralPath $secretHelperPath -PathType Leaf)) {
  throw "Missing secret generation helper: $secretHelperPath"
}
. $secretHelperPath
$vcpkgProvenanceHelperPath = Join-Path $PSScriptRoot "VcpkgProvenance.ps1"
if (-not (Test-Path -LiteralPath $vcpkgProvenanceHelperPath -PathType Leaf)) {
  throw "Missing vcpkg provenance helper: $vcpkgProvenanceHelperPath"
}
. $vcpkgProvenanceHelperPath

function Refresh-UserPath {
  $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
  $user = [Environment]::GetEnvironmentVariable("Path", "User")
  if ($null -ne $machine -or $null -ne $user) {
    $env:Path = "$machine;$user"
  }
  $env:Path = "$env:USERPROFILE\.bun\bin;$env:USERPROFILE\.cargo\bin;C:\Program Files\Docker\Docker\resources\bin;C:\Program Files\dotnet;" + $env:Path
}

function Add-DirectoryToUserPath {
  param([string] $Directory)
  if ([string]::IsNullOrWhiteSpace($Directory) -or -not (Test-Path -LiteralPath $Directory -PathType Container)) {
    return $false
  }

  $resolved = (Resolve-Path -LiteralPath $Directory).Path
  $pathParts = @($env:Path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  if (-not ($pathParts | Where-Object { $_.TrimEnd('\') -ieq $resolved.TrimEnd('\') })) {
    $env:Path = "$resolved;$env:Path"
  }

  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if ($null -eq $userPath) { $userPath = "" }
  $userPathParts = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  if (-not ($userPathParts | Where-Object { $_.TrimEnd('\') -ieq $resolved.TrimEnd('\') })) {
    $sep = if ($userPath -match ';$' -or [string]::IsNullOrWhiteSpace($userPath)) { "" } else { ";" }
    [Environment]::SetEnvironmentVariable("Path", $userPath + $sep + $resolved, "User")
  }
  return $true
}

function Set-UserEnvironmentVariable {
  param(
    [string] $Name,
    [string] $Value
  )
  if ([string]::IsNullOrWhiteSpace($Name) -or [string]::IsNullOrWhiteSpace($Value)) {
    return
  }
  Set-Item -Path "Env:$Name" -Value $Value
  [Environment]::SetEnvironmentVariable($Name, $Value, "User")
}

function Get-VsWherePath {
  $p = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if (Test-Path -LiteralPath $p) { return $p }
  return $null
}

function Get-MsvcHostX64BinPath {
  $vswhere = Get-VsWherePath
  if (-not $vswhere) { return $null }
  $prevEA = $ErrorActionPreference
  $ErrorActionPreference = "SilentlyContinue"
  $installs = @()
  $a = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
  if ($a) { $installs += $a }
  $b = & $vswhere -latest -products Microsoft.VisualStudio.Product.BuildTools -property installationPath 2>$null
  if ($b) { $installs += $b }
  $ErrorActionPreference = $prevEA
  foreach ($root in ($installs | Select-Object -Unique)) {
    if ([string]::IsNullOrWhiteSpace($root) -or -not (Test-Path -LiteralPath $root)) { continue }
    $msvcRoot = Join-Path $root "VC\Tools\MSVC"
    if (-not (Test-Path -LiteralPath $msvcRoot)) { continue }
    $latest = Get-ChildItem -LiteralPath $msvcRoot -Directory -ErrorAction SilentlyContinue |
      Sort-Object Name -Descending |
      Select-Object -First 1
    if (-not $latest) { continue }
    $bin = Join-Path $latest.FullName "bin\Hostx64\x64"
    $link = Join-Path $bin "link.exe"
    if (Test-Path -LiteralPath $link) { return (Resolve-Path -LiteralPath $bin).Path }
  }
  return $null
}

function Test-MsvcLinkAvailable {
  if (Get-Command link -ErrorAction SilentlyContinue) { return $true }
  $bin = Get-MsvcHostX64BinPath
  if ($bin) {
    $link = Join-Path $bin "link.exe"
    return (Test-Path -LiteralPath $link)
  }
  return $false
}

function Add-MsvcHostX64ToUserPath {
  $bin = Get-MsvcHostX64BinPath
  if (-not $bin) { return $false }
  if ($env:Path -notlike "*$bin*") {
    $env:Path = "$bin;" + $env:Path
  }
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if ($null -eq $userPath) { $userPath = "" }
  if ($userPath -notlike "*$bin*") {
    $sep = if ($userPath -match ';$' -or [string]::IsNullOrWhiteSpace($userPath)) { "" } else { ";" }
    [Environment]::SetEnvironmentVariable("Path", $userPath + $sep + $bin, "User")
  }
  return $true
}

# SignTool lives in the Windows SDK (Windows Kits), not in the MSVC toolchain. Multiple SDK versions
# can coexist under "Windows Kits\10\bin\<10.0.x>\x64". Prefer newest x64 signtool (same resolution as scripts/build-installers.ps1).
function Get-WindowsKitsSignToolX64Directory {
  $kitsBin = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
  if (-not (Test-Path -LiteralPath $kitsBin)) { return $null }
  $sdkDirs = Get-ChildItem -LiteralPath $kitsBin -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' }
  foreach ($d in ($sdkDirs | Sort-Object Name -Descending)) {
    $x64 = Join-Path $d.FullName "x64"
    $signtool = Join-Path $x64 "signtool.exe"
    if (Test-Path -LiteralPath $signtool) { return (Resolve-Path -LiteralPath $x64).Path }
  }
  return $null
}

function Test-SignToolResolvable {
  if (Get-Command signtool.exe -ErrorAction SilentlyContinue) { return $true }
  return $null -ne (Get-WindowsKitsSignToolX64Directory)
}

function Add-SignToolX64ToUserPath {
  $bin = Get-WindowsKitsSignToolX64Directory
  if (-not $bin) { return $false }
  if ($env:Path -notlike "*$bin*") {
    $env:Path = "$bin;" + $env:Path
  }
  $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
  if ($null -eq $userPath) { $userPath = "" }
  if ($userPath -notlike "*$bin*") {
    $sep = if ($userPath -match ';$' -or [string]::IsNullOrWhiteSpace($userPath)) { "" } else { ";" }
    [Environment]::SetEnvironmentVariable("Path", $userPath + $sep + $bin, "User")
  }
  return $true
}

function Install-WindowsSdkForSigningTools {
  if ($SkipWindowsSdk) {
    Write-Host "Skipping Windows SDK / signtool PATH (-SkipWindowsSdk)."
    return
  }
  if (Test-SignToolResolvable) {
    Write-Host "signtool.exe: already resolvable (Windows SDK or PATH)."
    $null = Add-SignToolX64ToUserPath
    return
  }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    Write-Warning "signtool.exe not found and winget is missing. Install a Windows SDK (signing tools) or add ...\Windows Kits\10\bin\<version>\x64 to PATH."
    return
  }
  # Standalone Windows SDK via winget (independent of VS 2022 vs 2025 Build Tools layout). Pick a current 10.x SDK id that includes signtool.
  Write-Host "Installing Windows SDK (signtool.exe for Authenticode / scripts/build-installers.ps1)..."
  $prevEA = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  & winget install --id "Microsoft.WindowsSDK.10.0.22621" -e --accept-package-agreements --accept-source-agreements
  $wingetExit = $LASTEXITCODE
  $ErrorActionPreference = $prevEA
  if ($wingetExit -ne 0) {
    Write-Warning "winget exit code $wingetExit installing Windows SDK. Try: winget install Microsoft.WindowsSDK.10.0.26100"
  }
  Start-Sleep -Seconds 2
  Refresh-UserPath
  if (-not (Add-SignToolX64ToUserPath) -or -not (Test-SignToolResolvable)) {
    Write-Warning "signtool.exe is still not on PATH. Open a new terminal after the installer finishes, then re-run this script."
  }
  else {
    Write-Host "Windows SDK: added signtool (x64) directory to your user PATH."
  }
}

function Install-VsBuildTools {
  if ($SkipVsBuildTools) {
    Write-Host "Skipping Visual Studio Build Tools (-SkipVsBuildTools)."
    return
  }
  if (Test-MsvcLinkAvailable) {
    Write-Host "MSVC link.exe: already available for Rust (x86_64-pc-windows-msvc)."
    $null = Add-MsvcHostX64ToUserPath
    return
  }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw "MSVC (link.exe) not found and winget is missing. Install 'Build Tools for Visual Studio 2022' with the C++ workload, or re-run on a system with Windows Package Manager."
  }
  Write-Host "Installing Visual Studio 2022 Build Tools (C++ / MSVC) - this is a large download and can take 15+ minutes."
  $override = "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  $prevEA = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  & winget install --id "Microsoft.VisualStudio.2022.BuildTools" -e --accept-package-agreements --accept-source-agreements --override $override
  $wingetExit = $LASTEXITCODE
  $ErrorActionPreference = $prevEA
  if ($wingetExit -ne 0) {
    Write-Warning "winget exit code $wingetExit. If Build Tools is already installed without C++, open Visual Studio Installer and add the 'Desktop development with C++' (or C++ build tools) workload."
  }
  Start-Sleep -Seconds 2
  Refresh-UserPath
  if (-not (Add-MsvcHostX64ToUserPath) -or -not (Test-MsvcLinkAvailable)) {
    Write-Warning "link.exe is still not on PATH. Reboot or open a new terminal after the installer finishes, then re-run this script, or add MSVC Hostx64 x64 to PATH (see: https://rust-lang.github.io/rustup/installation/windows-msvc.html)"
  } else {
    Write-Host "MSVC: added Hostx64\x64 to your user PATH. Open a new terminal for cargo to see link.exe if this session still fails."
  }
}

function Test-VcpkgVpxPresent {
  param([string] $Triplet)
  $p = "C:\vcpkg\installed\$Triplet\lib\vpx.lib"
  return (Test-Path -LiteralPath $p)
}

function Get-VcpkgVpxPkgConfigVersion {
  param([string] $Triplet)
  $pc = "C:\vcpkg\installed\$Triplet\lib\pkgconfig\vpx.pc"
  if (-not (Test-Path -LiteralPath $pc)) {
    return $null
  }
  $m = Select-String -LiteralPath $pc -Pattern "^Version:\s*(.+)$" | Select-Object -First 1
  if ($null -eq $m) { return $null }
  return $m.Matches.Groups[1].Value.Trim()
}

function Test-VcpkgLibvpxTalosPinned {
  param([string] $ExpectedVersion = "1.13.1")
  if (-not (Test-VcpkgVpxPresent -Triplet "x64-windows")) { return $false }
  if (-not (Test-VcpkgVpxPresent -Triplet "x86-windows")) { return $false }
  $vx64 = Get-VcpkgVpxPkgConfigVersion -Triplet "x64-windows"
  $vx86 = Get-VcpkgVpxPkgConfigVersion -Triplet "x86-windows"
  return ($vx64 -eq $ExpectedVersion -and $vx86 -eq $ExpectedVersion)
}

function Get-VcpkgPkgConfigPath {
  $candidates = @(
    "C:\vcpkg\installed\x64-windows\tools\pkgconf\pkgconf.exe",
    "C:\vcpkg\installed\x64-windows\tools\pkgconf\pkg-config.exe"
  )
  foreach ($candidate in $candidates) {
    if (Test-Path -LiteralPath $candidate -PathType Leaf) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }
  return $null
}

function Set-VcpkgVpxBuildEnvironment {
  $x64Root = "C:\vcpkg\installed\x64-windows"
  $x64Lib = Join-Path $x64Root "lib"
  $x64Include = Join-Path $x64Root "include"
  $pkgConfigExe = Get-VcpkgPkgConfigPath

  if (Test-Path -LiteralPath (Join-Path $x64Lib "vpx.lib") -PathType Leaf) {
    Set-UserEnvironmentVariable -Name "VPX_LIB_DIR" -Value $x64Lib
    Set-UserEnvironmentVariable -Name "VPX_INCLUDE_DIR" -Value $x64Include
    Set-UserEnvironmentVariable -Name "VPX_VERSION" -Value "1.13.0"
    Write-Host "libvpx env: set VPX_LIB_DIR, VPX_INCLUDE_DIR, and VPX_VERSION=1.13.0 (env-libvpx-sys pregenerated FFI) for current and future user sessions."
    Write-Host "Session-only (no user env writes): dot-source scripts/set-devenviroment.ps1"
  }
  else {
    Write-Warning "libvpx env: x64 vpx.lib not found at '$x64Lib\vpx.lib'; VPX_LIB_DIR was not changed."
  }

  if ($pkgConfigExe) {
    $pkgConfigDir = Split-Path -Parent $pkgConfigExe
    Set-UserEnvironmentVariable -Name "PKG_CONFIG" -Value $pkgConfigExe
    $null = Add-DirectoryToUserPath -Directory $pkgConfigDir

    $pkgConfigPath = Join-Path $x64Lib "pkgconfig"
    if (Test-Path -LiteralPath $pkgConfigPath -PathType Container) {
      Set-UserEnvironmentVariable -Name "PKG_CONFIG_PATH" -Value $pkgConfigPath
    }
    Write-Host "pkg-config env: set PKG_CONFIG and added '$pkgConfigDir' to the user PATH."
  }
  else {
    Write-Warning "pkg-config env: vcpkg pkgconf.exe was not found; scoped cargo builds may need PKG_CONFIG set manually."
  }
}

function Install-GitIfNeeded {
  if (Get-Command git -ErrorAction SilentlyContinue) { return }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw "Git is required for vcpkg but is not on PATH, and winget was not found."
  }
  Write-Host "Installing Git (required to clone vcpkg)..."
  $prevEA = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  & winget install --id Git.Git -e --accept-package-agreements --accept-source-agreements
  $ErrorActionPreference = $prevEA
  Refresh-UserPath
  if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw "Git was installed but is not on PATH. Open a new PowerShell and re-run this script."
  }
}

function Set-TalosVcpkgCheckout {
  param(
    [Parameter(Mandatory = $true)][string]$VcpkgRoot,
    [Parameter(Mandatory = $true)][string]$ExpectedCommit
  )

  $checkoutChanged = $false
  if (-not (Test-Path -LiteralPath $VcpkgRoot)) {
    Write-Host "Cloning the pinned vcpkg release to $VcpkgRoot..."
    & git clone --filter=blob:none --no-checkout "https://github.com/microsoft/vcpkg" $VcpkgRoot | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "git clone vcpkg failed (exit $LASTEXITCODE)." }
    $checkoutChanged = $true
  }

  if (-not (Test-Path -LiteralPath (Join-Path $VcpkgRoot ".git"))) {
    throw "$VcpkgRoot exists but is not a Git checkout. Move it aside and rerun so Talos can use pinned vcpkg release $TalosVcpkgRelease."
  }

  $trackedChanges = (& git -C $VcpkgRoot status --porcelain --untracked-files=no 2>$null | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) { throw "Unable to inspect the vcpkg checkout at $VcpkgRoot." }
  if ($trackedChanges) {
    throw "$VcpkgRoot has tracked local changes. Preserve or remove them before switching to pinned vcpkg release $TalosVcpkgRelease."
  }

  $currentCommit = (& git -C $VcpkgRoot rev-parse HEAD 2>$null | Out-String).Trim()
  if ($currentCommit -ne $ExpectedCommit) {
    Write-Host "Fetching pinned vcpkg release $TalosVcpkgRelease ($ExpectedCommit)..."
    & git -C $VcpkgRoot fetch --depth 1 origin $ExpectedCommit | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "git fetch for pinned vcpkg commit failed (exit $LASTEXITCODE)." }
    & git -C $VcpkgRoot checkout --detach $ExpectedCommit | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "git checkout for pinned vcpkg commit failed (exit $LASTEXITCODE)." }
    $checkoutChanged = $true
  }

  $resolvedCommit = (& git -C $VcpkgRoot rev-parse HEAD 2>$null | Out-String).Trim()
  if ($resolvedCommit -ne $ExpectedCommit) {
    throw "vcpkg checkout resolved to '$resolvedCommit', expected '$ExpectedCommit'."
  }

  $vcpkgExe = Join-Path $VcpkgRoot "vcpkg.exe"
  if ($checkoutChanged -or -not (Test-Path -LiteralPath $vcpkgExe)) {
    Write-Host "Bootstrapping pinned vcpkg release $TalosVcpkgRelease..."
    $boot = Start-Process -FilePath "cmd.exe" -ArgumentList @("/c", "bootstrap-vcpkg.bat -disableMetrics") -WorkingDirectory $VcpkgRoot -NoNewWindow -Wait -PassThru
    if ($boot.ExitCode -ne 0) { throw "bootstrap-vcpkg.bat failed with exit code $($boot.ExitCode)." }
  }

  return $checkoutChanged
}

function Install-VcpkgLibVpx {
  if ($SkipVcpkgLibVpx) {
    Write-Host "Skipping vcpkg libvpx (-SkipVcpkgLibVpx)."
    Set-VcpkgVpxBuildEnvironment
    return
  }
  $overlayPorts = Join-Path $PSScriptRoot "vcpkg-overlays"
  if (-not (Test-Path -LiteralPath (Join-Path $overlayPorts "libvpx\vcpkg.json"))) {
    throw "libvpx port overlay missing at $(Join-Path $overlayPorts 'libvpx'). Required for Talos (libvpx 1.13.1#5; upstream vcpkg baseline is newer)."
  }
  $overlayArg = "--overlay-ports=$overlayPorts"
  $expectedProvenance = Get-TalosVcpkgProvenanceRecord `
    -VcpkgCommit $TalosVcpkgCommit `
    -OverlayPath (Join-Path $overlayPorts "libvpx") `
    -LibvpxVersion "1.13.1" `
    -Triplets @("x64-windows", "x86-windows")
  Install-GitIfNeeded
  $vcpkgRoot = "C:\vcpkg"
  $vcpkgExe = Join-Path $vcpkgRoot "vcpkg.exe"
  $checkoutChanged = Set-TalosVcpkgCheckout -VcpkgRoot $vcpkgRoot -ExpectedCommit $TalosVcpkgCommit
  $baselineMarker = Join-Path $vcpkgRoot "installed\talos-vcpkg-baseline.txt"
  $baselineMatches = (-not $checkoutChanged) -and (
    Test-TalosVcpkgProvenanceRecord -MarkerPath $baselineMarker -ExpectedRecord $expectedProvenance
  )
  if ($baselineMatches -and (Test-VcpkgLibvpxTalosPinned -ExpectedVersion "1.13.1")) {
    Write-Host "libvpx: C:\vcpkg already has vpx.lib 1.13.1 (x64-windows and x86-windows) per vpx.pc."
    Set-VcpkgVpxBuildEnvironment
    return
  }
  if ($SkipVsBuildTools) {
    Write-Warning "vcpkg compiles libvpx from source and needs the MSVC C++ toolset. If install fails, install Visual Studio Build Tools (C++ workload) and re-run (omit -SkipVsBuildTools)."
  }
  if (-not (Test-Path -LiteralPath $vcpkgExe)) { throw "vcpkg.exe not found at $vcpkgExe." }
  if (-not $baselineMatches) {
    Write-Host "vcpkg: rebuilding Talos native inputs for pinned release $TalosVcpkgRelease..."
    & $vcpkgExe remove libvpx:x64-windows libvpx:x86-windows pkgconf:x64-windows --recurse 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) { Write-Warning "vcpkg baseline cleanup exited $LASTEXITCODE (packages may already be absent)." }
  }
  if (-not (Get-VcpkgPkgConfigPath)) {
    Write-Host "vcpkg: installing pkgconf for x64-windows (pkg-config compatibility for env-libvpx-sys)..."
    & $vcpkgExe install --triplet "x64-windows" pkgconf
    if ($LASTEXITCODE -ne 0) { throw "vcpkg install pkgconf x64-windows failed (exit $LASTEXITCODE)." }
  }
  if (-not (Test-VcpkgLibvpxTalosPinned -ExpectedVersion "1.13.1")) {
    Write-Host "vcpkg: replacing libvpx with overlay port 1.13.1#5 (x64-windows / x86-windows)..."
    & $vcpkgExe remove libvpx:x64-windows libvpx:x86-windows --recurse 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) { Write-Warning "vcpkg remove libvpx exited $LASTEXITCODE (may be clean if packages were absent)." }
  }
  Write-Host "vcpkg: building libvpx 1.13.1 for x64-windows from repo overlay (can take 10+ minutes)..."
  & $vcpkgExe install --triplet "x64-windows" libvpx $overlayArg
  if ($LASTEXITCODE -ne 0) { throw "vcpkg install libvpx x64-windows failed (exit $LASTEXITCODE)." }
  Write-Host "vcpkg: building libvpx 1.13.1 for x86-windows from repo overlay (can take 10+ minutes)..."
  & $vcpkgExe install --triplet "x86-windows" libvpx $overlayArg
  if ($LASTEXITCODE -ne 0) { throw "vcpkg install libvpx x86-windows failed (exit $LASTEXITCODE)." }
  New-Item -ItemType Directory -Force -Path (Split-Path -Parent $baselineMarker) | Out-Null
  Set-Content -LiteralPath $baselineMarker -Value $expectedProvenance -Encoding Ascii
  Set-VcpkgVpxBuildEnvironment
  Write-Host "libvpx: installed under C:\vcpkg\installed (1.13.1) from vcpkg $TalosVcpkgRelease and configured for env-libvpx-sys with VPX_VERSION=1.13.0."
}

function Test-LibClangDllPresent {
  $paths = @(
    "C:\Program Files\LLVM\bin\libclang.dll",
    "C:\Program Files (x86)\LLVM\bin\libclang.dll"
  )
  foreach ($p in $paths) {
    if (Test-Path -LiteralPath $p) { return $true }
  }
  return $false
}

function Install-LlvmForBindgen {
  if ($SkipLLVM) {
    Write-Host "Skipping LLVM install (-SkipLLVM). Ensure LIBCLANG_PATH points at the folder with libclang.dll for bindgen (yuv-sys)."
    return
  }
  if (Test-LibClangDllPresent) {
    Write-Host "LLVM libclang: already found under Program Files (bindgen / yuv-sys)."
    return
  }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw "LLVM (libclang) is required for yuv-sys/bindgen. Install from https://releases.llvm.org/ or set LIBCLANG_PATH, or install winget."
  }
  Write-Host "Installing LLVM (libclang for Rust bindgen, e.g. yuv-sys) via winget..."
  $prevEA = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  & winget install --id "LLVM.LLVM" -e --accept-package-agreements --accept-source-agreements
  $ErrorActionPreference = $prevEA
  if (-not (Test-LibClangDllPresent)) {
    Write-Warning "LLVM was installed or updated but libclang.dll is not in the default path yet. Open a new terminal, or set LIBCLANG_PATH to the LLVM bin directory (e.g. C:\Program Files\LLVM\bin), then re-run this script or build-installers.ps1."
  } else {
    Write-Host "LLVM: libclang.dll available. build-installers.ps1 sets LIBCLANG_PATH when you run it."
  }
}

function Test-CommandName {
  param([string] $Name)
  return $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Install-Bun {
  if (Test-CommandName "bun") {
    $installedVersion = (& bun --version).Trim()
    if ($installedVersion -eq $RequiredBunVersion) {
      Write-Host "Bun: already on PATH ($installedVersion)."
      return
    }
    Write-Host "Bun $installedVersion is installed; switching to pinned version $RequiredBunVersion..."
  } else {
    Write-Host "Installing pinned Bun $RequiredBunVersion..."
  }
  iex "& {$(irm https://bun.com/install.ps1)} -Version $RequiredBunVersion"
  Refresh-UserPath
  if (-not (Test-CommandName "bun")) { throw "Bun install finished but 'bun' was not found. Open a new terminal and re-run this script." }
  $installedVersion = (& bun --version).Trim()
  if ($installedVersion -ne $RequiredBunVersion) {
    throw "Expected Bun $RequiredBunVersion after installation, found $installedVersion."
  }
  Write-Host "Bun: OK ($installedVersion)"
}

function Install-Rust {
  if (-not (Test-CommandName "rustup") -and (Get-Command winget -ErrorAction SilentlyContinue)) {
    Write-Host "Installing Rust (rustup) via winget..."
    winget install --id Rustlang.Rustup -e --accept-package-agreements --accept-source-agreements
    Refresh-UserPath
  } elseif (-not (Test-CommandName "rustup")) {
    throw "winget is required to install rustup. Install the App Installer from the Microsoft Store, then re-run."
  }

  Write-Host "Installing pinned Rust $RequiredRustVersion with rustfmt and Clippy..."
  & rustup toolchain install $RequiredRustVersion --profile minimal --component clippy,rustfmt
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to install the pinned Rust $RequiredRustVersion toolchain."
  }

  $installedVersion = (& rustc "+$RequiredRustVersion" -V).Trim()
  if ($LASTEXITCODE -ne 0 -or $installedVersion -notmatch "^rustc $([regex]::Escape($RequiredRustVersion))\b") {
    throw "Expected pinned Rust $RequiredRustVersion after installation, found '$installedVersion'."
  }
  Write-Host "Rust: OK ($installedVersion)"
}

function Test-DotNetSdkAvailable {
  if (Get-Command dotnet -ErrorAction SilentlyContinue) { return $true }
  $exe = "C:\Program Files\dotnet\dotnet.exe"
  return (Test-Path -LiteralPath $exe)
}

function Install-DotNetSdk {
  if ($SkipDotNet) {
    Write-Host "Skipping .NET SDK (-SkipDotNet)."
    return
  }
  if (Test-DotNetSdkAvailable) {
    Refresh-UserPath
    if (Get-Command dotnet -ErrorAction SilentlyContinue) {
      Write-Host ".NET SDK: already available ($(& dotnet --version))."
    }
    else {
      Write-Host ".NET SDK: dotnet.exe found under Program Files; refreshing PATH for this session."
      $env:Path = "C:\Program Files\dotnet;" + $env:Path
      if (Get-Command dotnet -ErrorAction SilentlyContinue) {
        Write-Host ".NET SDK: OK ($(& dotnet --version))."
      }
    }
    return
  }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
    throw ".NET SDK (dotnet) not found and winget is missing. Install from https://dotnet.microsoft.com/download or ensure C:\Program Files\dotnet is on PATH."
  }
  Write-Host "Installing .NET SDK 8 (LTS) for WiX / apps/installer (dotnet restore)..."
  $prevEA = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  & winget install --id "Microsoft.DotNet.SDK.8" -e --accept-package-agreements --accept-source-agreements
  $wingetExit = $LASTEXITCODE
  $ErrorActionPreference = $prevEA
  if ($wingetExit -ne 0) {
    Write-Warning "winget exit code $wingetExit installing .NET SDK. If dotnet is installed elsewhere, add its folder to PATH."
  }
  Start-Sleep -Seconds 2
  Refresh-UserPath
  if (-not (Get-Command dotnet -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath "C:\Program Files\dotnet\dotnet.exe")) {
    $env:Path = "C:\Program Files\dotnet;" + $env:Path
  }
  if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    Write-Warning "dotnet is still not on PATH. Open a new terminal after the installer finishes, then re-run this script or add C:\Program Files\dotnet to PATH."
  }
  else {
    Write-Host ".NET SDK: OK ($(& dotnet --version))"
  }
}

function Install-WslMsi {
  if (Get-Command wsl -ErrorAction SilentlyContinue) {
    $st = & wsl --status 2>&1
    if ($LASTEXITCODE -eq 0) { return }
  }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { return }
  Write-Host "Installing Windows Subsystem for Linux (WSL) via winget..."
  winget install --id Microsoft.WSL -e --accept-package-agreements --accept-source-agreements
}

function Enable-WslVmp {
  if ($SkipWslVmp) { return }
  if (-not (Get-Command wsl -ErrorAction SilentlyContinue)) { return }
  Write-Host "Ensuring Virtual Machine Platform and WSL optional components (may require reboot)..."
  $p = Start-Process -FilePath "wsl.exe" -ArgumentList @("--install", "--no-distribution") -Wait -NoNewWindow -PassThru
  if ($p.ExitCode -ne 0) {
    Write-Warning "wsl --install --no-distribution exited with code $($p.ExitCode). You may need to run an elevated PowerShell once, or complete WSL setup from Settings -> Apps -> Optional features."
  }
}

function Install-DockerDesktop {
  if ($SkipDocker) { return }
  $docker = "C:\Program Files\Docker\Docker\resources\bin\docker.exe"
  if ((Test-CommandName "docker") -or (Test-Path $docker)) {
    Write-Host "Docker: already present."
    return
  }
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { throw "winget is required to install Docker Desktop." }
  Write-Host "Installing Docker Desktop via winget (large download)..."
  winget install --id Docker.DockerDesktop -e --accept-package-agreements --accept-source-agreements
  Refresh-UserPath
}

function Copy-AppEnvIfNeeded {
  param([string] $RepoRoot, [bool] $Force)
  $example = Join-Path $RepoRoot "apps\.env.example"
  $target = Join-Path $RepoRoot "apps\.env"
  if (-not (Test-Path $example)) { throw "Missing $example" }
  if ((Test-Path $target) -and -not $Force) {
    Write-Host "apps/.env already exists; skipping. Use -ForceEnv to replace (backup created)."
    return
  }
  if ((Test-Path $target) -and $Force) {
    Copy-Item -Force $target "$target.bak.$(Get-Date -Format 'yyyyMMddHHmmss')"
  }
  Write-Host 'Creating apps/.env for local dev (not committed; from .env.example)...'
  # Each checkout receives independent, cryptographically random credentials. Purpose prefixes
  # guarantee that credentials for distinct trust boundaries cannot accidentally be identical.
  $jwt = New-TalosRandomSecret -Purpose "jwt" -ByteCount 48
  $appEncryption = New-TalosRandomSecret -Purpose "app_encryption" -ByteCount 48
  $rmmServerKey = New-TalosRandomSecret -Purpose "rmm_server" -ByteCount 32
  $serviceKey = New-TalosRandomSecret -Purpose "service" -ByteCount 32
  $telemetryKey = New-TalosRandomSecret -Purpose "telemetry" -ByteCount 32
  $aiRunnerKey = New-TalosRandomSecret -Purpose "ai_runner" -ByteCount 32
  $agent = New-TalosRandomSecret -Purpose "agent" -ByteCount 32
  $lines = Get-Content -LiteralPath $example
  $out = foreach ($line in $lines) {
    if ($line -match "^\s*#") { $line; continue }
    if ($line -notmatch "=") { $line; continue }
    if ($line -match "^\s*JWT_SECRET=") { "JWT_SECRET=$jwt"; continue }
    if ($line -match "^\s*APP_ENCRYPTION_KEY=") { "APP_ENCRYPTION_KEY=$appEncryption"; continue }
    if ($line -match "^\s*RMM_SERVER_API_KEY=") { "RMM_SERVER_API_KEY=$rmmServerKey"; continue }
    if ($line -match "^\s*RMM_TELEMETRY_KAFKA_BROKERS=") { "RMM_TELEMETRY_KAFKA_BROKERS=redpanda-0:9092"; continue }
    if ($line -match "^\s*RMM_TELEMETRY_SNAPSHOT_TOPIC=") { "RMM_TELEMETRY_SNAPSHOT_TOPIC=rmm_telemetry_snapshots"; continue }
    if ($line -match "^\s*RMM_TELEMETRY_EVENTS_TOPIC=") { "RMM_TELEMETRY_EVENTS_TOPIC=rmm_telemetry_events"; continue }
    if ($line -match "^\s*SERVICE_KEY=") { "SERVICE_KEY=$serviceKey"; continue }
    if ($line -match "^\s*API_SERVICE_KEY=") { "API_SERVICE_KEY=$serviceKey"; continue }
    if ($line -match "^\s*RMM_TELEMETRY_SERVICE_KEY=") { "RMM_TELEMETRY_SERVICE_KEY=$telemetryKey"; continue }
    if ($line -match "^\s*TALOS_AI_RUNNER_SERVICE_KEY=") { "TALOS_AI_RUNNER_SERVICE_KEY=$aiRunnerKey"; continue }
    if ($line -match "^\s*RMM_AGENT_TOKEN=") { "RMM_AGENT_TOKEN=$agent"; continue }
    $line
  }
  Set-Content -LiteralPath $target -Value $out -Encoding utf8
  Write-Host "Wrote $target"
}

function Test-DockerEngine {
  param([int] $TimeoutMs = 20000)
  $docker = "C:\Program Files\Docker\Docker\resources\bin\docker.exe"
  if (-not (Test-Path $docker)) { return $false }
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $docker
  $psi.Arguments = "info"
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.CreateNoWindow = $true
  $proc = New-Object System.Diagnostics.Process
  $proc.StartInfo = $psi
  $null = $proc.Start()
  if (-not $proc.WaitForExit($TimeoutMs)) {
    try { $proc.Kill() } catch { }
    return $false
  }
  $s = $proc.StandardOutput.ReadToEnd() + "`n" + $proc.StandardError.ReadToEnd()
  if ($s -match "(?i)ERROR:|\b500 Internal Server Error\b|Cannot connect|Is the docker daemon") { return $false }
  if ($s -notmatch "Server:|\bServer Version:\b|Containers:|\bCPUs:") { return $false }
  return $true
}

# --- main ---
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Write-Host "Repository root: $RepoRoot"

Refresh-UserPath
Install-Bun
Install-Rust
Install-DotNetSdk
Install-VsBuildTools
Refresh-UserPath
Install-WindowsSdkForSigningTools
Refresh-UserPath
Install-VcpkgLibVpx
Refresh-UserPath
Install-LlvmForBindgen
Refresh-UserPath
Install-WslMsi
Enable-WslVmp
Install-DockerDesktop
Refresh-UserPath
Copy-AppEnvIfNeeded -RepoRoot $RepoRoot -Force:$ForceEnv

Push-Location (Join-Path $RepoRoot "apps")
try {
  Write-Host "Running frozen Bun workspace install (apps)..."
  & bun install --frozen-lockfile
} finally {
  Pop-Location
}

if (-not $SkipDocker) {
  if (Test-DockerEngine) {
    Write-Host ""
    Write-Host "Docker engine is up. To start the full stack from repo root:" -ForegroundColor Green
    Write-Host "  cd apps" -ForegroundColor Cyan
    Write-Host "  bun run dev" -ForegroundColor Cyan
  } else {
    $dockerExe = "C:\Program Files\Docker\Docker\Docker Desktop.exe"
    if (Test-Path $dockerExe) {
      Write-Host "Starting Docker Desktop (login / engine may need a moment after a fresh install or reboot)..."
      Start-Process $dockerExe -ErrorAction SilentlyContinue
    }
    Write-Host ""
    Write-Warning "Docker engine is not ready yet. Typical fixes:"
    Write-Warning "  1) Reboot if you just enabled Virtual Machine Platform (wsl --status should show WSL2 OK)."
    Write-Warning "  2) Open Docker Desktop and wait until it says 'Engine running'."
    Write-Warning "  3) Then from apps/: bun run dev"
  }
} else {
  Write-Host ""
  Write-Host "Skipped local Docker (-SkipDocker or -SkipLocalContainers). Use your remote engine / Docker Offload per your setup, then from apps/: bun run dev" -ForegroundColor DarkGray
}
