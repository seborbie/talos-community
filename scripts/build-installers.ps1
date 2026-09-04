param(
    [ValidateSet("dev", "release")]
    [string]$BuildProfile = "dev",
    [string]$CertificateThumbprint = "",
    [string]$ManifestCertificateThumbprint = "",
    # Optional PFX input for manifest signing. When supplied, this takes precedence over
    # ManifestCertificateThumbprint and is loaded ephemerally rather than imported into a store.
    [string]$ManifestCertificatePath = "",
    [System.Security.SecureString]$ManifestCertificatePassword = $null,
    [string]$TimestampServer = "http://timestamp.digicert.com",
    # Optional PowerShell adapter for an HSM or remote Authenticode signing service. The adapter
    # receives only artifact paths, the expected public certificate thumbprint, and timestamp URL;
    # this build still verifies every returned signature locally and never receives a private key.
    [string]$ExternalAuthenticodeSignerPath = "",
    # Build only selected cargo-built client binaries and then stop. Omit to preserve the
    # full behavior. Accepts crate names or short aliases, e.g. -Part talos_worker.
    # Worker and helper can be scoped by ISA, e.g. talos_worker_x86_64_v3 or talos_worker_helper_i386.
    [Alias("Part", "Component")]
    [ValidateSet(
        "all",
        "agent",
        "worker",
        "helper",
        "worker-helper",
        "updater",
        "supervisor",
        "viewer",
        "viewer-updater",
        "agent-chat",
        "worker-chat",
        "talos_worker",
        "talos_worker_all",
        "talos_worker_i386",
        "talos_worker_x86_64_v1",
        "talos_worker_x86_64_v2",
        "talos_worker_x86_64_v3",
        "talos_worker_x86_64_v4",
        "talos_worker_helper",
        "talos_worker_helper_all",
        "talos_worker_helper_i386",
        "talos_worker_helper_x86_64_v1",
        "talos_worker_helper_x86_64_v2",
        "talos_worker_helper_x86_64_v3",
        "talos_worker_helper_x86_64_v4",
        "talos_supervisor",
        "talos_viewer",
        "talos_viewer_updater",
        "talos_worker_chat",
        "talos_linux",
        "linux",
        "talos_windows_installers",
        "windows",
        "windows-installers",
        "wix"
    )]
    [string[]]$BuildPart = @("all"),
    # Deprecated: dev and release builds now always produce the full worker architecture matrix.
    [switch] $BuildX86,
    # Full builds publish installer artifacts by default. Scoped -BuildPart runs build binaries only
    # unless this switch is present.
    [switch] $BuildInstallers,
    [Alias("LinuxArchitecture", "LinuxTarget")]
    [ValidateSet("linux-x64", "linux-x86", "linux-arm64", "linux-arm")]
    [string[]]$LinuxArch = @("linux-x64"),
    [bool]$AllowX86FallbackInNonRelease = $false,
    [string]$SevenZipPath = "",
    [string]$SfxStubPath = "",
    # Authenticode-sign cargo-built EXEs before staging, final WiX MSIs before Burn embeds them,
    # and the completed Burn bundle before publication. Omit for unsigned local builds.
    [switch] $SignAuthenticodeBinaries,
    # Skips signtool.exe Authenticode signing of Windows binaries and installers (overrides
    # -SignAuthenticodeBinaries). Use only when an intentionally unsigned build is acceptable.
    [switch] $SkipAuthenticodeSigning
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$script:BuildInstallersOriginalLocation = (Get-Location).ProviderPath
$script:TalosVcpkgCommit = "d015e31e90838a4c9dfa3eed45979bc70d9357fc"
$script:RequiredNasmVersion = "3.01"
$vcpkgProvenanceHelperPath = Join-Path $PSScriptRoot "VcpkgProvenance.ps1"
if (-not (Test-Path -LiteralPath $vcpkgProvenanceHelperPath -PathType Leaf)) {
    throw "Missing vcpkg provenance helper: $vcpkgProvenanceHelperPath"
}
. $vcpkgProvenanceHelperPath

function Restore-BuildInstallersOriginalLocation {
    if ([string]::IsNullOrWhiteSpace($script:BuildInstallersOriginalLocation)) {
        return
    }

    try {
        if (Test-Path -LiteralPath $script:BuildInstallersOriginalLocation -PathType Container) {
            Set-Location -LiteralPath $script:BuildInstallersOriginalLocation
        }
    }
    catch {
        Write-Warning "Unable to restore original location '$script:BuildInstallersOriginalLocation': $_"
    }
}

trap {
    Restore-BuildInstallersOriginalLocation
    break
}

if ($BuildProfile -eq "release") {
    $releaseSigningRequested = [bool]$SignAuthenticodeBinaries
    $releaseUnsignedRequested = [bool]$SkipAuthenticodeSigning
    if ($releaseSigningRequested -eq $releaseUnsignedRequested) {
        throw "-BuildProfile release requires exactly one explicit Authenticode choice: -SignAuthenticodeBinaries or -SkipAuthenticodeSigning."
    }
}

function Write-Step([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message"
}

function Invoke-Checked([string]$Description, [scriptblock]$Command) {
    Write-Step $Description
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

function Test-IsWindowsHost {
    return [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
        [System.Runtime.InteropServices.OSPlatform]::Windows
    )
}

function Normalize-CertificateThumbprint([string]$Thumbprint, [string]$Label) {
    $normalized = ($Thumbprint -replace "[\s:]", "").ToUpperInvariant()
    if ($normalized -notmatch "^[0-9A-F]{40}$") {
        throw "$Label must be an explicit 40-character SHA-1 certificate thumbprint."
    }
    return $normalized
}

function Find-CertificateByThumbprint([string]$Thumbprint, [string]$Label) {
    $normalized = Normalize-CertificateThumbprint -Thumbprint $Thumbprint -Label $Label
    $cert = Get-ChildItem Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
        Where-Object { (($_.Thumbprint -replace "\s", "").ToUpperInvariant()) -eq $normalized } |
        Select-Object -First 1
    if (-not $cert) {
        $cert = Get-ChildItem Cert:\CurrentUser\My -ErrorAction SilentlyContinue |
            Where-Object { (($_.Thumbprint -replace "\s", "").ToUpperInvariant()) -eq $normalized } |
            Select-Object -First 1
    }
    if (-not $cert) {
        throw "$Label certificate '$Thumbprint' was not found in Cert:\LocalMachine\My or Cert:\CurrentUser\My."
    }
    return $cert
}

function Get-CodeSigningCert([string]$Thumbprint) {
    $normalized = Normalize-CertificateThumbprint -Thumbprint $Thumbprint -Label "-CertificateThumbprint"
    $cert = Find-CertificateByThumbprint -Thumbprint $normalized -Label "-CertificateThumbprint"
    if (-not $cert.HasPrivateKey) {
        throw "Authenticode certificate '$normalized' does not have an accessible private key."
    }
    $now = Get-Date
    if ($now -lt $cert.NotBefore -or $now -gt $cert.NotAfter) {
        throw "Authenticode certificate '$normalized' is not currently within its validity period."
    }
    $enhancedKeyUsage = $cert.Extensions |
        Where-Object { $_.Oid.Value -eq "2.5.29.37" } |
        Select-Object -First 1
    if ($enhancedKeyUsage) {
        $codeSigningOid = "1.3.6.1.5.5.7.3.3"
        $permitsCodeSigning = @(
            ([System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]$enhancedKeyUsage).EnhancedKeyUsages |
                Where-Object { $_.Value -eq $codeSigningOid }
        ).Count -gt 0
        if (-not $permitsCodeSigning) {
            throw "Authenticode certificate '$normalized' does not permit code signing."
        }
    }
    return $cert
}

function Get-ExternalAuthenticodeSignerPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $null
    }

    $resolvedPath = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    if (-not (Test-Path -LiteralPath $resolvedPath -PathType Leaf)) {
        throw "External Authenticode signer adapter not found at '$resolvedPath'."
    }
    if ([System.IO.Path]::GetExtension($resolvedPath) -ine ".ps1") {
        throw "-ExternalAuthenticodeSignerPath must name a PowerShell .ps1 adapter."
    }
    return $resolvedPath
}

function Assert-ManifestSigningCertificate(
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$Cert,
    [string]$SourceDescription
) {
    if (-not $Cert.HasPrivateKey) {
        throw "Manifest signing certificate from $SourceDescription does not have a private key."
    }

    $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($Cert)
    if (-not $rsa) {
        throw "Manifest signing certificate from $SourceDescription does not have an RSA private key."
    }
    try {
        # talos_update_common verifies RSA PKCS#1 v1.5 SHA-256 with ring's 2048..8192-bit policy.
        if ($rsa.KeySize -lt 2048 -or $rsa.KeySize -gt 8192) {
            throw "Manifest signing certificate from $SourceDescription has an unsupported $($rsa.KeySize)-bit RSA key; expected 2048 through 8192 bits."
        }
    }
    finally {
        $rsa.Dispose()
    }

    $keyUsageExtension = $Cert.Extensions |
        Where-Object { $_.Oid.Value -eq "2.5.29.15" } |
        Select-Object -First 1
    if ($keyUsageExtension) {
        $keyUsage = [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension]$keyUsageExtension
        $digitalSignature = [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature
        if (($keyUsage.KeyUsages -band $digitalSignature) -eq 0) {
            throw "Manifest signing certificate from $SourceDescription does not permit digital signatures."
        }
    }
}

function Get-ManifestSigningCert(
    [string]$CertificatePath,
    [System.Security.SecureString]$CertificatePassword,
    [string]$Thumbprint,
    [string]$ForbiddenRoot
) {
    if (-not [string]::IsNullOrWhiteSpace($CertificatePath)) {
        if ($null -eq $CertificatePassword) {
            throw "-ManifestCertificatePassword (a SecureString) is required with -ManifestCertificatePath."
        }

        $resolvedPath = (Resolve-Path -LiteralPath $CertificatePath -ErrorAction Stop).Path
        if (-not (Test-Path -LiteralPath $resolvedPath -PathType Leaf)) {
            throw "Manifest signing PFX not found at '$resolvedPath'."
        }
        $normalizedForbiddenRoot = [System.IO.Path]::GetFullPath($ForbiddenRoot).TrimEnd([char[]]@(
            [System.IO.Path]::DirectorySeparatorChar,
            [System.IO.Path]::AltDirectorySeparatorChar
        )) + [System.IO.Path]::DirectorySeparatorChar
        if ($resolvedPath.StartsWith($normalizedForbiddenRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Manifest signing PFX must remain outside the Talos repository: '$resolvedPath'."
        }

        try {
            # Keep the Community signing key out of the Windows certificate store. Only the
            # derived public key is copied into installer/tmp and embedded in updater clients.
            $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
                $resolvedPath,
                $CertificatePassword,
                [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
            )
        }
        catch {
            throw "Unable to load manifest signing PFX '$resolvedPath': $($_.Exception.Message)"
        }

        try {
            Assert-ManifestSigningCertificate -Cert $cert -SourceDescription "PFX '$resolvedPath'"
            return $cert
        }
        catch {
            $cert.Dispose()
            throw
        }
    }

    if ([string]::IsNullOrWhiteSpace($Thumbprint)) {
        throw "Manifest signing requires -ManifestCertificatePath or -ManifestCertificateThumbprint."
    }

    $cert = Find-CertificateByThumbprint `
        -Thumbprint $Thumbprint `
        -Label "-ManifestCertificateThumbprint"
    Assert-ManifestSigningCertificate -Cert $cert -SourceDescription "certificate store thumbprint '$Thumbprint'"
    return $cert
}

function Ensure-RustTarget([string]$TargetTriple) {
    $installedTargets = rustup target list --installed
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to query installed Rust targets."
    }
    if (-not ($installedTargets -contains $TargetTriple)) {
        Invoke-Checked "Installing Rust target '$TargetTriple'" { rustup target add $TargetTriple }
    }
}

function Ensure-NasmAvailable() {
    $nasmCmd = Get-Command nasm -ErrorAction SilentlyContinue
    if (-not $nasmCmd) {
        $candidateDirs = @(
            "C:\Program Files\NASM",
            "C:\Program Files (x86)\NASM"
        )
        foreach ($dir in $candidateDirs) {
            $exe = Join-Path $dir "nasm.exe"
            if (Test-Path -Path $exe -PathType Leaf) {
                $env:PATH = "$dir;$($env:PATH)"
                $nasmCmd = Get-Command nasm -ErrorAction SilentlyContinue
                if ($nasmCmd) {
                    break
                }
            }
        }
    }

    if (-not $nasmCmd) {
        throw "NASM $($script:RequiredNasmVersion) is required for reproducible Windows x86/x64 aws-lc builds but was not found. Install that exact reviewed version and ensure 'nasm.exe' is on PATH."
    }

    $versionOutput = (& $nasmCmd.Source -v 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $versionOutput -notmatch '^NASM version (?<version>[0-9]+(?:\.[0-9]+)+)(?:\s|$)') {
        throw "Unable to verify the NASM version from '$($nasmCmd.Source)': $versionOutput"
    }
    $actualVersion = $Matches.version
    if ($actualVersion -ne $script:RequiredNasmVersion) {
        throw "NASM version '$actualVersion' is installed at '$($nasmCmd.Source)'; release builds require exact version '$($script:RequiredNasmVersion)'."
    }
    Write-Step "Using pinned NASM $actualVersion from $($nasmCmd.Source)"
}

function Get-SevenZipExecutablePath([string]$ExplicitPath) {
    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        $candidates += $ExplicitPath
    }

    $commandCandidates = @("7z", "7za")
    foreach ($commandName in $commandCandidates) {
        $resolved = Get-Command $commandName -ErrorAction SilentlyContinue
        if ($resolved -and -not [string]::IsNullOrWhiteSpace($resolved.Source)) {
            $candidates += $resolved.Source
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($env:SEVEN_ZIP_EXE)) {
        $candidates += $env:SEVEN_ZIP_EXE
    }

    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $candidates += (Join-Path $env:ProgramFiles "7-Zip\7z.exe")
    }
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidates += (Join-Path ${env:ProgramFiles(x86)} "7-Zip\7z.exe")
    }

    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) {
            continue
        }
        try {
            $resolvedPath = (Resolve-Path $candidate -ErrorAction Stop).Path
            if (Test-Path -Path $resolvedPath -PathType Leaf) {
                return $resolvedPath
            }
        }
        catch {
            continue
        }
    }

    throw "7-Zip CLI executable not found. Provide -SevenZipPath or set SEVEN_ZIP_EXE. Expected command '7z' or a valid path to 7z.exe."
}

function Get-SfxStubSourcePath([string]$ExplicitPath, [string]$ArtifactRoot) {
    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        $candidates += $ExplicitPath
    }
    if (-not [string]::IsNullOrWhiteSpace($env:RMM_INSTALLER_SFX_STUB_SOURCE_PATH)) {
        $candidates += $env:RMM_INSTALLER_SFX_STUB_SOURCE_PATH
    }
    $candidates += (Join-Path $ArtifactRoot "sfx\7zSD.sfx")
    $candidates += (Join-Path $ArtifactRoot "sfx\7zsd.sfx")

    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) {
            continue
        }
        try {
            $resolvedPath = (Resolve-Path $candidate -ErrorAction Stop).Path
            if (Test-Path -Path $resolvedPath -PathType Leaf) {
                return $resolvedPath
            }
        }
        catch {
            continue
        }
    }

    throw "7z SFX stub not found. Provide -SfxStubPath or set RMM_INSTALLER_SFX_STUB_SOURCE_PATH. Default expected path: $ArtifactRoot\\sfx\\7zSD.sfx"
}

function Assert-ExpectedSha256(
    [string]$Path,
    [string]$ExpectedSha256,
    [string]$Label
) {
    if ([string]::IsNullOrWhiteSpace($ExpectedSha256)) {
        return
    }

    $normalizedExpected = ($ExpectedSha256 -replace "\s", "").ToLowerInvariant()
    if ($normalizedExpected -notmatch "^[0-9a-f]{64}$") {
        throw "Invalid expected SHA-256 for ${Label}: '$ExpectedSha256'."
    }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
    if ($actual -ne $normalizedExpected) {
        throw "$Label SHA-256 mismatch for '$Path'. Expected $normalizedExpected; got $actual."
    }
}

function Get-BunExecutablePath {
    $bunCommand = Get-Command bun -ErrorAction SilentlyContinue
    if (-not $bunCommand -or [string]::IsNullOrWhiteSpace($bunCommand.Source)) {
        throw "Bun is required to verify and acquire pinned third-party source/build inputs."
    }
    return $bunCommand.Source
}

function Get-InstallerAcquisitionExtractor([string]$ExplicitSevenZipPath) {
    if (-not [string]::IsNullOrWhiteSpace($ExplicitSevenZipPath)) {
        return [pscustomobject]@{
            Path = Get-SevenZipExecutablePath -ExplicitPath $ExplicitSevenZipPath
            Kind = "7zip"
        }
    }

    $tarCommand = Get-Command tar -ErrorAction SilentlyContinue
    if ($tarCommand -and -not [string]::IsNullOrWhiteSpace($tarCommand.Source)) {
        return [pscustomobject]@{
            Path = $tarCommand.Source
            Kind = "tar"
        }
    }

    return [pscustomobject]@{
        Path = Get-SevenZipExecutablePath -ExplicitPath ""
        Kind = "7zip"
    }
}

function Resolve-AcquiredRepoFile(
    [string]$RepoRoot,
    [string]$RelativePath,
    [string]$Label
) {
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [System.IO.Path]::IsPathRooted($RelativePath)) {
        throw "$Label did not return a repository-relative path."
    }
    $normalizedRoot = [System.IO.Path]::GetFullPath($RepoRoot).TrimEnd([char[]]@(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )) + [System.IO.Path]::DirectorySeparatorChar
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $RelativePath))
    if (-not $candidate.StartsWith($normalizedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label escaped the repository root: $RelativePath"
    }
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        throw "$Label is missing after acquisition: $candidate"
    }
    return $candidate
}

function Get-PinnedInstallerInputs(
    [string]$RepoRoot,
    [string]$AcquisitionScriptPath,
    [string]$PolicyPath,
    [string]$ExplicitSevenZipPath
) {
    if (-not (Test-Path -LiteralPath $AcquisitionScriptPath -PathType Leaf)) {
        throw "Pinned third-party acquisition script is missing: $AcquisitionScriptPath"
    }
    if (-not (Test-Path -LiteralPath $PolicyPath -PathType Leaf)) {
        throw "Pinned third-party acquisition policy is missing: $PolicyPath"
    }

    $bunExecutable = Get-BunExecutablePath
    $extractor = Get-InstallerAcquisitionExtractor -ExplicitSevenZipPath $ExplicitSevenZipPath
    Write-Step "Acquiring and verifying pinned 7-Zip 26.00 and WiX 6.0.0 inputs"
    $resultText = (& $bunExecutable $AcquisitionScriptPath installer `
        --repo-root $RepoRoot `
        --extractor $extractor.Path `
        --extractor-kind $extractor.Kind | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Pinned installer input acquisition failed with exit code $LASTEXITCODE."
    }
    try {
        $result = $resultText | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
        throw "Pinned installer input acquisition returned invalid metadata: $resultText"
    }

    $sevenZipExecutable = Resolve-AcquiredRepoFile -RepoRoot $RepoRoot -RelativePath $result.sevenZipExecutable -Label "Pinned 7za.exe"
    $sfxStub = Resolve-AcquiredRepoFile -RepoRoot $RepoRoot -RelativePath $result.sfxStub -Label "Pinned 7zSD.sfx"
    $nugetConfig = Resolve-AcquiredRepoFile -RepoRoot $RepoRoot -RelativePath $result.nugetConfig -Label "Pinned WiX NuGet.Config"
    $manifest = Resolve-AcquiredRepoFile -RepoRoot $RepoRoot -RelativePath $result.manifest -Label "Third-party acquisition manifest"

    $policy = Get-Content -LiteralPath $PolicyPath -Raw | ConvertFrom-Json -ErrorAction Stop
    $sevenZipExpected = ($policy.sevenZip.members | Where-Object { $_.path -eq "7za.exe" } | Select-Object -First 1).sha256
    $sfxExpected = ($policy.sevenZip.members | Where-Object { $_.path -eq "bin/7zSD.sfx" } | Select-Object -First 1).sha256
    Assert-ExpectedSha256 -Path $sevenZipExecutable -ExpectedSha256 $sevenZipExpected -Label "Pinned 7za.exe"
    Assert-ExpectedSha256 -Path $sfxStub -ExpectedSha256 $sfxExpected -Label "Pinned 7zSD.sfx"

    return [pscustomobject]@{
        SevenZipExecutable = $sevenZipExecutable
        SfxStub = $sfxStub
        SfxSha256 = $sfxExpected
        NuGetConfig = $nugetConfig
        Manifest = $manifest
        Policy = $PolicyPath
    }
}

function Assert-MicrosoftAuthenticodeSignature([string]$Path, [string]$Label) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path -ErrorAction Stop
    if (-not $signature -or $signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        $status = if ($signature) { $signature.Status.ToString() } else { "Missing" }
        throw "$Label must have a valid Microsoft Authenticode signature; '$Path' reported $status."
    }
    if (-not $signature.SignerCertificate) {
        throw "$Label has no Authenticode signer certificate: '$Path'."
    }
    $subject = $signature.SignerCertificate.Subject
    if ($subject -notmatch "(^|,\s*)O=Microsoft Corporation(\s*,|$)") {
        throw "$Label Authenticode signer is not Microsoft Corporation: '$subject'."
    }
}

function Assert-DownloadedFile(
    [string]$Path,
    [string]$Label,
    [string]$ExpectedSha256,
    [bool]$RequireMicrosoftAuthenticode
) {
    Assert-ExpectedSha256 -Path $Path -ExpectedSha256 $ExpectedSha256 -Label $Label
    if ($RequireMicrosoftAuthenticode) {
        Assert-MicrosoftAuthenticodeSignature -Path $Path -Label $Label
    }
}

function Ensure-DownloadedFile(
    [string]$Url,
    [string]$DestinationPath,
    [string]$Label,
    [string]$ExpectedSha256 = "",
    [bool]$RequireMicrosoftAuthenticode = $false
) {
    if (Test-Path -LiteralPath $DestinationPath -PathType Leaf) {
        Write-Step "Verifying cached $Label at $DestinationPath"
        Assert-DownloadedFile -Path $DestinationPath -Label $Label -ExpectedSha256 $ExpectedSha256 -RequireMicrosoftAuthenticode $RequireMicrosoftAuthenticode
        return
    }

    $destinationDir = Split-Path -Parent $DestinationPath
    if (-not [string]::IsNullOrWhiteSpace($destinationDir)) {
        New-Item -ItemType Directory -Force -Path $destinationDir | Out-Null
    }

    $temporaryName = ".$(Split-Path -Leaf $DestinationPath).$([guid]::NewGuid().ToString('N')).download.exe"
    $temporaryPath = Join-Path $destinationDir $temporaryName
    try {
        Write-Step "Downloading $Label"
        Invoke-WebRequest -Uri $Url -OutFile $temporaryPath -UseBasicParsing
        Assert-DownloadedFile -Path $temporaryPath -Label $Label -ExpectedSha256 $ExpectedSha256 -RequireMicrosoftAuthenticode $RequireMicrosoftAuthenticode
        Move-Item -Force -LiteralPath $temporaryPath -Destination $DestinationPath
    }
    finally {
        Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath $temporaryPath
    }
}

function Set-VpxEnvForTarget([string]$TargetTriple) {
    $baselineMarker = "C:\vcpkg\installed\talos-vcpkg-baseline.txt"
    if (-not (Test-Path -LiteralPath $baselineMarker -PathType Leaf)) {
        throw "Pinned vcpkg provenance marker is missing at '$baselineMarker'. Re-run scripts/Setup-DevEnviroment.ps1 before building release artifacts."
    }
    $overlayPath = Join-Path $PSScriptRoot "vcpkg-overlays\libvpx"
    $expectedProvenance = Get-TalosVcpkgProvenanceRecord `
        -VcpkgCommit $script:TalosVcpkgCommit `
        -OverlayPath $overlayPath `
        -LibvpxVersion "1.13.1" `
        -Triplets @("x64-windows", "x86-windows")
    if (-not (Test-TalosVcpkgProvenanceRecord -MarkerPath $baselineMarker -ExpectedRecord $expectedProvenance)) {
        throw "vcpkg native-input provenance does not match the pinned commit, libvpx overlay, version, and triplets. Re-run scripts/Setup-DevEnviroment.ps1."
    }

    $x64Lib = "C:\vcpkg\installed\x64-windows\lib"
    $x64Include = "C:\vcpkg\installed\x64-windows\include"
    $x86Lib = "C:\vcpkg\installed\x86-windows\lib"
    $x86Include = "C:\vcpkg\installed\x86-windows\include"

    if ($TargetTriple -eq "i686-pc-windows-msvc") {
        if (Test-Path (Join-Path $x86Lib "vpx.lib")) {
            $env:VPX_LIB_DIR = $x86Lib
            $env:VPX_INCLUDE_DIR = $x86Include
            if ([string]::IsNullOrWhiteSpace($env:VPX_VERSION)) {
                $env:VPX_VERSION = "1.13.0"
            }
            Write-Step "Using x86 VPX libs from $x86Lib"
            return
        }
        throw "Missing x86 libvpx at '$x86Lib\\vpx.lib'. Install with: C:\\vcpkg\\vcpkg.exe install libvpx:x86-windows"
    }

    if (Test-Path (Join-Path $x64Lib "vpx.lib")) {
        $env:VPX_LIB_DIR = $x64Lib
        $env:VPX_INCLUDE_DIR = $x64Include
        if ([string]::IsNullOrWhiteSpace($env:VPX_VERSION)) {
            $env:VPX_VERSION = "1.13.0"
        }
        Write-Step "Using x64 VPX libs from $x64Lib"
        return
    }
    throw "Missing x64 libvpx at '$x64Lib\\vpx.lib'. env-libvpx-sys needs VPX_LIB_DIR or pkg-config; install with: C:\\vcpkg\\vcpkg.exe install libvpx:x64-windows (see scripts/Setup-DevEnviroment.ps1)"
}

function Get-LibClangBinDirectory {
    if (-not [string]::IsNullOrWhiteSpace($env:LIBCLANG_PATH)) {
        $dll = Join-Path $env:LIBCLANG_PATH "libclang.dll"
        if (Test-Path -LiteralPath $dll) {
            return (Resolve-Path -LiteralPath $env:LIBCLANG_PATH).Path
        }
    }
    $candidates = [System.Collections.Generic.List[string]]::new()
    $candidates.Add("C:\Program Files\LLVM\bin")
    $candidates.Add("C:\Program Files (x86)\LLVM\bin")
    $btLlvm = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin"
    if (Test-Path -LiteralPath (Join-Path $btLlvm "libclang.dll")) {
        $candidates.Insert(0, $btLlvm)
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere) {
        $prevEA = $ErrorActionPreference
        $ErrorActionPreference = "SilentlyContinue"
        $inst = & $vswhere -latest -products * -property installationPath 2>$null | Select-Object -First 1
        $ErrorActionPreference = $prevEA
        if (-not [string]::IsNullOrWhiteSpace($inst)) {
            $llvmBin = Join-Path $inst.Trim() "VC\Tools\Llvm\x64\bin"
            if (Test-Path -LiteralPath (Join-Path $llvmBin "libclang.dll")) {
                $candidates.Insert(0, $llvmBin)
            }
        }
    }
    foreach ($d in $candidates) {
        if ([string]::IsNullOrWhiteSpace($d)) { continue }
        $dll = Join-Path $d "libclang.dll"
        if (Test-Path -LiteralPath $dll) {
            return (Resolve-Path -LiteralPath $d).Path
        }
    }
    return $null
}

function Set-LibClangEnv {
    $bin = Get-LibClangBinDirectory
    if (-not $bin) {
        throw "libclang.dll not found (bindgen / yuv-sys). Install LLVM: winget install LLVM.LLVM, or set LIBCLANG_PATH to the directory containing libclang.dll (see scripts/Setup-DevEnviroment.ps1)."
    }
    $env:LIBCLANG_PATH = $bin
    Write-Step "Using LIBCLANG_PATH for bindgen: $bin"
}

function Get-CargoBuildArgs([string]$Profile, [string]$TargetTriple) {
    $args = @("build", "--locked", "--target", $TargetTriple)
    switch ($Profile) {
        "release" { return @($args[0], "--release") + $args[1..($args.Length - 1)] }
        "debug" { return $args }
        "dev" { return $args }
        default { return @($args[0], "--profile", $Profile) + $args[1..($args.Length - 1)] }
    }
}

function Get-CargoBuildShellArgs([string]$Profile, [string]$TargetTriple, [string[]]$Packages) {
    $args = [System.Collections.Generic.List[string]]::new()
    $args.Add("build")
    $args.Add("--locked")
    if ($Profile -eq "release") {
        $args.Add("--release")
    }
    elseif (($Profile -ne "dev") -and ($Profile -ne "debug")) {
        $args.Add("--profile")
        $args.Add($Profile)
    }
    $args.Add("--target")
    $args.Add($TargetTriple)
    foreach ($package in $Packages) {
        $args.Add("-p")
        $args.Add($package)
    }
    return ($args | ForEach-Object { "'" + ($_ -replace "'", "'\''") + "'" }) -join " "
}

function Get-CargoProfileDirName([string]$Profile) {
    switch ($Profile) {
        "dev" { return "debug" }
        default { return $Profile }
    }
}

function Ensure-LinuxDockerBuilderImage([string]$ImageName) {
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $imagePlatform = (& docker image inspect $ImageName --format "{{.Os}}/{{.Architecture}}" 2>$null | Select-Object -First 1)
        $inspectExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }

    if ($inspectExitCode -eq 0 -and $imagePlatform -eq "linux/amd64") {
        Write-Step "Using cached Linux Docker builder image: $ImageName"
        return
    }
    if ($inspectExitCode -eq 0) {
        Write-Step "Rebuilding Linux Docker builder image because cached platform '$imagePlatform' is not linux/amd64"
    }

    $dockerBuildContext = Join-Path $env:TEMP "talos-linux-builder-context-$([guid]::NewGuid().ToString("N"))"
    New-Item -ItemType Directory -Force -Path $dockerBuildContext | Out-Null
    $dockerfilePath = Join-Path $dockerBuildContext "Dockerfile"
    $dockerfile = @"
FROM rockylinux:8@sha256:9794037624aaa6212aeada1d28861ef5e0a935adaf93e4ef79837119f2a2d04c
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:`$PATH
RUN dnf install -y \
        ca-certificates \
        clang \
        clang-devel \
        cmake \
        curl \
        gcc \
        gcc-c++ \
        git \
        make \
        openssl-devel \
        perl \
        pkgconf-pkg-config \
        tar \
        xz \
    && dnf clean all \
    && rm -rf /var/cache/dnf
RUN curl --proto '=https' --tlsv1.2 -sSfL 'https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init' -o /tmp/rustup-init \
    && echo '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c  /tmp/rustup-init' | sha256sum -c - \
    && chmod 0755 /tmp/rustup-init \
    && /tmp/rustup-init -y --profile minimal --default-toolchain 1.95.0 --target x86_64-unknown-linux-gnu \
    && rm -f /tmp/rustup-init \
    && chmod -R a+w `$RUSTUP_HOME `$CARGO_HOME
"@
    [System.IO.File]::WriteAllText($dockerfilePath, $dockerfile, [System.Text.UTF8Encoding]::new($false))

    try {
        Invoke-Checked "Building cached Linux Docker builder image: $ImageName" {
            & docker build --platform linux/amd64 -t $ImageName -f $dockerfilePath $dockerBuildContext
        }
    }
    finally {
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $dockerBuildContext
    }
}

function Test-LinuxBuildContextPathIsSensitive([string]$RelativePath) {
    $normalized = $RelativePath.Replace('\', '/')
    if ($normalized -like 'apps/certs/*' -or $normalized -match '(?i)\.(pem|pfx|p12|key)$') {
        return $true
    }
    if ($normalized -match '(^|/)\.env($|\.)') {
        return -not ($normalized -match '(?i)\.(example|sample|template)$')
    }
    return $false
}

function New-LinuxSanitizedBuildContext(
    [string]$RepoRoot,
    [string]$ManifestPublicKeyDerPath
) {
    $git = Get-Command git -ErrorAction SilentlyContinue
    if (-not $git) {
        throw "git is required to create the sanitized Linux Docker build context."
    }

    $contextRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("talos-linux-source-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $contextRoot | Out-Null
    try {
        $relativePaths = @(& git -C $RepoRoot ls-files --cached --others --exclude-standard)
        if ($LASTEXITCODE -ne 0) {
            throw "git ls-files failed while creating the sanitized Linux Docker build context."
        }

        foreach ($relativePath in $relativePaths) {
            if ([string]::IsNullOrWhiteSpace($relativePath) -or (Test-LinuxBuildContextPathIsSensitive -RelativePath $relativePath)) {
                continue
            }
            $platformRelativePath = $relativePath.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
            $sourcePath = Join-Path $RepoRoot $platformRelativePath
            if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
                continue
            }
            $destinationPath = Join-Path $contextRoot $platformRelativePath
            $destinationDirectory = Split-Path -Parent $destinationPath
            New-Item -ItemType Directory -Force -Path $destinationDirectory | Out-Null
            Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
        }

        if (-not [string]::IsNullOrWhiteSpace($ManifestPublicKeyDerPath)) {
            if (-not (Test-Path -LiteralPath $ManifestPublicKeyDerPath -PathType Leaf)) {
                throw "Manifest public key DER not found: $ManifestPublicKeyDerPath"
            }
            $publicKeyPath = Join-Path $contextRoot "apps\installer\tmp\manifest_public_key.der"
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $publicKeyPath) | Out-Null
            Copy-Item -LiteralPath $ManifestPublicKeyDerPath -Destination $publicKeyPath -Force
        }

        return $contextRoot
    }
    catch {
        Remove-Item -LiteralPath $contextRoot -Recurse -Force -ErrorAction SilentlyContinue
        throw
    }
}

function Invoke-LinuxCargoBuildInDocker(
    [string]$RepoRoot,
    [string]$TargetTriple,
    [string]$Profile,
    [string[]]$Packages,
    [string]$ManifestPublicKeyDerPath
) {
    $docker = Get-Command docker -ErrorAction SilentlyContinue
    if (-not $docker) {
        throw "Docker is required to build Linux binaries from Windows. Install/start Docker Desktop, or run this script on Linux."
    }

    $builderImageName = "talos-linux-builder:rust-1.95-rustup1.28.2-rockylinux8-glibc2.28"
    Ensure-LinuxDockerBuilderImage -ImageName $builderImageName

    $cargoArgs = Get-CargoBuildShellArgs -Profile $Profile -TargetTriple $TargetTriple -Packages $Packages
    $profileDirName = Get-CargoProfileDirName -Profile $Profile
    $targetCacheVolume = "talos-linux-target-$TargetTriple"
    $copyCommands = ($Packages | ForEach-Object {
        "cp /cargo-target/$TargetTriple/$profileDirName/$_ /talos-build-output/$_"
    }) -join "`n"
    $manifestEnvArgs = @()
    if (-not [string]::IsNullOrWhiteSpace($ManifestPublicKeyDerPath)) {
        $manifestEnvArgs = @("-e", "RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH=/workspace/apps/installer/tmp/manifest_public_key.der")
    }

$containerScript = @"
set -euo pipefail
export PATH="/usr/local/cargo/bin:`$PATH"
rustup target add '$TargetTriple'
cargo $cargoArgs
$copyCommands
"@
    $containerScript = $containerScript -replace "`r`n", "`n" -replace "`r", "`n"

    $sourceContext = $null
    $outputRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("talos-linux-output-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
    try {
        $sourceContext = New-LinuxSanitizedBuildContext `
            -RepoRoot $RepoRoot `
            -ManifestPublicKeyDerPath $ManifestPublicKeyDerPath
        $sourceMount = "${sourceContext}:/workspace:ro"
        $outputMount = "${outputRoot}:/talos-build-output"

        Invoke-Checked "Building Linux $($Packages -join ", ") for $TargetTriple in Docker" {
            & docker run --rm `
                --platform linux/amd64 `
                -v $sourceMount `
                -v $outputMount `
                -v talos-linux-cargo-registry:/usr/local/cargo/registry `
                -v talos-linux-cargo-git:/usr/local/cargo/git `
                -v "${targetCacheVolume}:/cargo-target" `
                -w /workspace/apps `
                -e CARGO_TARGET_DIR=/cargo-target `
                -e CARGO_BUILD_JOBS=2 `
                @manifestEnvArgs `
                $builderImageName `
                bash -lc $containerScript
        }

        $hostOutputDirectory = Join-Path $RepoRoot "apps\target\$TargetTriple\$profileDirName"
        New-Item -ItemType Directory -Force -Path $hostOutputDirectory | Out-Null
        foreach ($package in $Packages) {
            $sourceOutput = Join-Path $outputRoot $package
            if (-not (Test-Path -LiteralPath $sourceOutput -PathType Leaf)) {
                throw "Expected Linux Docker build output not found: $sourceOutput"
            }
            Copy-Item -LiteralPath $sourceOutput -Destination (Join-Path $hostOutputDirectory $package) -Force
        }
    }
    finally {
        if (-not [string]::IsNullOrWhiteSpace($sourceContext)) {
            Remove-Item -LiteralPath $sourceContext -Recurse -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath $outputRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Join-RustFlags([string[]]$Flags) {
    return @($Flags | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }) -join " "
}

function Get-WorkerRustFlags([string]$WorkerArch) {
    switch ($WorkerArch) {
        "x64-v1" { return "" }
        "x64-v2" { return "-Ctarget-feature=+sse3,+ssse3,+sse4.1,+sse4.2,+popcnt" }
        "x64-v3" { return "-Ctarget-feature=+sse3,+ssse3,+sse4.1,+sse4.2,+popcnt,+avx,+avx2,+bmi1,+bmi2,+fma" }
        "x64-v4" { return "-Ctarget-feature=+sse3,+ssse3,+sse4.1,+sse4.2,+popcnt,+avx,+avx2,+bmi1,+bmi2,+fma,+avx512f,+avx512bw,+avx512cd,+avx512dq,+avx512vl" }
        "x86" { return "" }
        default { throw "Unknown worker architecture '$WorkerArch'." }
    }
}

function Get-SignToolExecutablePath() {
    $resolved = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($resolved -and -not [string]::IsNullOrWhiteSpace($resolved.Source)) {
        return $resolved.Source
    }

    # Prefer newest SDK under Windows Kits\10\bin\<version>\x64 (multiple VS/SDK years install side by side).
    # Avoid blind recurse + FullName sort (could pick arm64/x86).
    $kitsBin = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kitsBin) {
        $sdkDirs = Get-ChildItem -LiteralPath $kitsBin -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' }
        foreach ($d in ($sdkDirs | Sort-Object Name -Descending)) {
            $candidate = Join-Path $d.FullName "x64\signtool.exe"
            if (Test-Path -LiteralPath $candidate) {
                return (Resolve-Path -LiteralPath $candidate).Path
            }
        }
    }

    throw "signtool.exe was not found. Install the Windows SDK signing tools (run scripts/Setup-DevEnviroment.ps1 without -SkipWindowsSdk) or add ...\Windows Kits\10\bin\<version>\x64 to PATH."
}

function Test-BinaryHasExpectedSignature([string]$Path, [string]$ExpectedThumbprint) {
    if (-not (Test-Path -Path $Path -PathType Leaf)) {
        return $false
    }

    $signature = Get-AuthenticodeSignature -FilePath $Path
    if (-not $signature) {
        return $false
    }

    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        return $false
    }

    if (-not $signature.SignerCertificate) {
        return $false
    }

    $actualThumbprint = ($signature.SignerCertificate.Thumbprint -replace "\s", "").ToUpperInvariant()
    $normalizedExpectedThumbprint = ($ExpectedThumbprint -replace "\s", "").ToUpperInvariant()
    return $actualThumbprint -eq $normalizedExpectedThumbprint
}

function Sign-Binaries(
    [string[]]$Paths,
    $Cert,
    [string]$ExpectedThumbprint,
    [string]$TimestampUrl,
    [string]$ExternalSignerPath
) {
    $uniquePaths = @($Paths | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
    if ($uniquePaths.Count -eq 0) {
        throw "Authenticode signing was requested, but no Windows binaries or installers were selected."
    }
    $normalizedExpectedThumbprint = Normalize-CertificateThumbprint `
        -Thumbprint $ExpectedThumbprint `
        -Label "-CertificateThumbprint"

    $pendingPaths = @()
    foreach ($path in $uniquePaths) {
        if (-not (Test-Path -Path $path -PathType Leaf)) {
            throw "Cannot sign missing file: $path"
        }

        if (Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint) {
            Write-Host "Skipping already-signed binary: $path"
            continue
        }

        $pendingPaths += $path
    }

    if ($pendingPaths.Count -eq 0) {
        Write-Host "All binaries already have a valid signature."
    }
    else {
        if (-not [string]::IsNullOrWhiteSpace($ExternalSignerPath)) {
            # The adapter owns provider-specific authentication (for example an HSM or signing
            # service). Its stable Talos contract contains no private-key or password argument.
            & $ExternalSignerPath `
                -FilePath $pendingPaths `
                -ExpectedCertificateThumbprint $normalizedExpectedThumbprint `
                -TimestampServer $TimestampUrl
            if (-not $?) {
                throw "External Authenticode signer adapter failed."
            }
        }
        else {
            if (-not $Cert) {
                throw "A local Authenticode certificate is required when no external signer adapter is configured."
            }
            $signToolPath = Get-SignToolExecutablePath
            # /sm = LocalMachine (Personal). If the private key is only associated in CurrentUser,
            # try without /sm.
            & $signToolPath sign /sha1 $normalizedExpectedThumbprint /sm /s My /fd SHA256 /td SHA256 /tr $TimestampUrl @($pendingPaths)
            if ($LASTEXITCODE -ne 0) {
                Write-Warning "Signtool failed with /sm (LocalMachine). Retrying without /sm (CurrentUser certificate)..."
                & $signToolPath sign /sha1 $normalizedExpectedThumbprint /s My /fd SHA256 /td SHA256 /tr $TimestampUrl @($pendingPaths)
            }
            if ($LASTEXITCODE -ne 0) {
                throw "Authenticode signing failed with signtool.exe (exit code $LASTEXITCODE). Use -SkipAuthenticodeSigning only for an intentionally unsigned build, or fix the certificate/private-key access."
            }
        }
    }

    # Do not trust signtool's exit code alone. A signed-release path fails closed unless each final
    # file has a currently valid signature from the exact selected certificate.
    foreach ($path in $uniquePaths) {
        if (-not (Test-BinaryHasExpectedSignature -Path $path -ExpectedThumbprint $normalizedExpectedThumbprint)) {
            throw "Authenticode verification failed after signing '$path' with certificate '$normalizedExpectedThumbprint'."
        }
        Write-Host "Verified Authenticode signature: $path"
    }
}

function Sign-BurnBundle(
    [string]$BundlePath,
    [string]$RepoRoot,
    [string]$WixToolManifestPath,
    [string]$NuGetConfigPath,
    $Cert,
    [string]$ExpectedThumbprint,
    [string]$TimestampUrl,
    [string]$ExternalSignerPath
) {
    if (-not (Test-Path -LiteralPath $BundlePath -PathType Leaf)) {
        throw "Cannot sign missing Burn bundle: $BundlePath"
    }
    if (-not (Test-Path -LiteralPath $WixToolManifestPath -PathType Leaf)) {
        throw "Pinned WiX CLI tool manifest not found: $WixToolManifestPath"
    }
    if (-not (Test-Path -LiteralPath $NuGetConfigPath -PathType Leaf)) {
        throw "Pinned WiX NuGet configuration not found: $NuGetConfigPath"
    }

    # Burn caches and later elevates its extracted engine, so signing only the outer bundle is not
    # sufficient. Follow WiX's required detach -> sign engine -> reattach -> sign bundle sequence.
    $temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("talos-burn-sign-" + [guid]::NewGuid().ToString("N"))
    $detachedEnginePath = Join-Path $temporaryRoot "burn-engine.exe"
    $reattachedBundlePath = Join-Path $temporaryRoot "reattached-bundle.exe"
    $verificationEnginePath = Join-Path $temporaryRoot "verification-engine.exe"
    New-Item -ItemType Directory -Force -Path $temporaryRoot | Out-Null

    Push-Location $RepoRoot
    try {
        Invoke-Checked "Restoring pinned WiX CLI 6.0.0" {
            & dotnet tool restore --tool-manifest $WixToolManifestPath --configfile $NuGetConfigPath
        }

        $wixVersion = (& dotnet tool run wix -- --version | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) {
            throw "Unable to query the pinned WiX CLI version (exit code $LASTEXITCODE)."
        }
        if ($wixVersion -notmatch '^6\.0\.0(?:[+-].*)?$') {
            throw "Expected pinned WiX CLI 6.0.0, found '$wixVersion'."
        }

        Invoke-Checked "Detaching the WiX Burn engine for Authenticode signing" {
            & dotnet tool run wix -- burn detach $BundlePath -engine $detachedEnginePath
        }
        Sign-Binaries -Paths @($detachedEnginePath) -Cert $Cert -ExpectedThumbprint $ExpectedThumbprint -TimestampUrl $TimestampUrl -ExternalSignerPath $ExternalSignerPath

        Invoke-Checked "Reattaching the signed WiX Burn engine" {
            & dotnet tool run wix -- burn reattach $BundlePath -engine $detachedEnginePath -o $reattachedBundlePath
        }
        if (-not (Test-Path -LiteralPath $reattachedBundlePath -PathType Leaf)) {
            throw "WiX did not produce the reattached Burn bundle: $reattachedBundlePath"
        }
        Copy-Item -LiteralPath $reattachedBundlePath -Destination $BundlePath -Force

        # Sign and verify the full compressed bundle only after the signed engine is reattached.
        Sign-Binaries -Paths @($BundlePath) -Cert $Cert -ExpectedThumbprint $ExpectedThumbprint -TimestampUrl $TimestampUrl -ExternalSignerPath $ExternalSignerPath

        # Prove the final outer signature did not hide an unsigned cached/elevated engine.
        Invoke-Checked "Verifying the signed engine embedded in the final Burn bundle" {
            & dotnet tool run wix -- burn detach $BundlePath -engine $verificationEnginePath
        }
        if (-not (Test-BinaryHasExpectedSignature -Path $verificationEnginePath -ExpectedThumbprint $ExpectedThumbprint)) {
            throw "The final Burn bundle does not contain a valid engine signature from certificate '$ExpectedThumbprint'."
        }
        Write-Host "Verified embedded Burn engine Authenticode signature: $BundlePath"
    }
    finally {
        Pop-Location -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Get-ArtifactMetadata([string]$Path) {
    if (-not (Test-Path -Path $Path -PathType Leaf)) {
        throw "Cannot build metadata for missing file: $Path"
    }

    $item = Get-Item -Path $Path -ErrorAction Stop
    $hash = (Get-FileHash -Path $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
    return @{
        fileName = $item.Name
        sizeBytes = [int64]$item.Length
        sha256 = $hash
    }
}

function Get-BuildSourceMetadata([string]$RepoRoot) {
    $revision = "unknown"
    $trackedSourceDirty = $null
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $revisionOutput = (& git -C $RepoRoot rev-parse HEAD 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $revisionOutput -match "^[0-9a-fA-F]{40}$") {
            $revision = $revisionOutput.ToLowerInvariant()
        }

        $statusOutput = (& git -C $RepoRoot status --porcelain=v1 --untracked-files=no 2>$null | Out-String).Trim()
        if ($LASTEXITCODE -eq 0) {
            $trackedSourceDirty = -not [string]::IsNullOrWhiteSpace($statusOutput)
        }
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }

    return [ordered]@{
        revision = $revision
        trackedSourceDirty = $trackedSourceDirty
    }
}

function Write-UnsignedArtifactNotice([string]$Path) {
    $notice = @"
IMPORTANT: UNSIGNED COMMUNITY BINARIES

The Windows executables and installers in this artifact set do not carry a Talos publisher
Authenticode signature. Windows may show an Unknown publisher, SmartScreen, or reputation warning.
That warning is expected for this unsigned build, but it is not proof that a particular download is
safe. Verify the file against SHA256SUMS obtained from the same release, confirm the release source,
and follow your organisation's software-approval policy. Do not disable SmartScreen, antivirus,
application control, or signature enforcement globally.

Updater manifest signatures are separate. They authorize update metadata for clients containing
the matching pinned public key; they do not give Windows a publisher identity.
"@
    [System.IO.File]::WriteAllText($Path, $notice.Trim() + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
}

function Write-Utf8NoBomJson([string]$Path, [string]$Json) {
    [System.IO.File]::WriteAllText(
        $Path,
        $Json + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Write-ArtifactChecksums([string]$ArtifactDirectory, [string]$OutputPath) {
    $outputName = [System.IO.Path]::GetFileName($OutputPath)
    $lines = @(
        Get-ChildItem -LiteralPath $ArtifactDirectory -File |
            Where-Object { $_.Name -ne $outputName } |
            Sort-Object -Property Name |
            ForEach-Object {
                $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
                "$hash  $($_.Name)"
            }
    )
    if ($lines.Count -eq 0) {
        throw "Cannot write checksums for an empty artifact directory: $ArtifactDirectory"
    }
    [System.IO.File]::WriteAllLines($OutputPath, $lines, [System.Text.UTF8Encoding]::new($false))
}

function Encode-DerLength([int]$Length) {
    if ($Length -lt 0) {
        throw "DER length cannot be negative."
    }

    if ($Length -lt 128) {
        return [byte[]]@([byte]$Length)
    }

    $bytes = [System.Collections.Generic.List[byte]]::new()
    $remaining = $Length
    while ($remaining -gt 0) {
        $bytes.Insert(0, [byte]($remaining -band 0xFF))
        $remaining = [math]::Floor($remaining / 256)
    }

    $prefix = [byte](0x80 -bor $bytes.Count)
    $result = [System.Collections.Generic.List[byte]]::new()
    $result.Add($prefix)
    $result.AddRange([byte[]]$bytes.ToArray())
    return $result.ToArray()
}

function Encode-DerInteger([byte[]]$Value) {
    if (-not $Value -or $Value.Length -eq 0) {
        throw "DER INTEGER requires at least one byte."
    }

    $trimmed = $Value
    while ($trimmed.Length -gt 1 -and $trimmed[0] -eq 0) {
        $trimmed = $trimmed[1..($trimmed.Length - 1)]
    }

    $content = [System.Collections.Generic.List[byte]]::new()
    if (($trimmed[0] -band 0x80) -ne 0) {
        $content.Add(0)
    }
    $content.AddRange([byte[]]$trimmed)

    $result = [System.Collections.Generic.List[byte]]::new()
    $result.Add(0x02)
    $result.AddRange([byte[]](Encode-DerLength -Length $content.Count))
    $result.AddRange([byte[]]$content.ToArray())
    return $result.ToArray()
}

function Convert-RsaParametersToPkcs1PublicKeyDer([System.Security.Cryptography.RSAParameters]$Parameters) {
    if (-not $Parameters.Modulus -or -not $Parameters.Exponent) {
        throw "RSA public key parameters are incomplete."
    }

    $modulusInteger = Encode-DerInteger -Value $Parameters.Modulus
    $exponentInteger = Encode-DerInteger -Value $Parameters.Exponent

    $sequenceContent = [System.Collections.Generic.List[byte]]::new()
    $sequenceContent.AddRange([byte[]]$modulusInteger)
    $sequenceContent.AddRange([byte[]]$exponentInteger)

    $sequence = [System.Collections.Generic.List[byte]]::new()
    $sequence.Add(0x30)
    $sequence.AddRange([byte[]](Encode-DerLength -Length $sequenceContent.Count))
    $sequence.AddRange([byte[]]$sequenceContent.ToArray())
    return $sequence.ToArray()
}

function Export-PublicKeyDer([System.Security.Cryptography.X509Certificates.X509Certificate2]$Cert, [string]$DestinationPath) {
    $destinationDir = Split-Path -Parent $DestinationPath
    if (-not [string]::IsNullOrWhiteSpace($destinationDir)) {
        New-Item -ItemType Directory -Force -Path $destinationDir | Out-Null
    }

    $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey($Cert)
    if (-not $rsa) {
        throw "Certificate '$($Cert.Thumbprint)' does not have an RSA public key."
    }
    try {
        # ring::signature::RSA_PKCS1_* expects a PKCS#1 RSAPublicKey blob, not SPKI.
        $bytes = Convert-RsaParametersToPkcs1PublicKeyDer -Parameters ($rsa.ExportParameters($false))
        [System.IO.File]::WriteAllBytes($DestinationPath, $bytes)
    }
    finally {
        $rsa.Dispose()
    }
}

function Sign-ManifestFile(
    [string]$ManifestPath,
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$Cert,
    [string]$SignaturePath
) {
    if (-not (Test-Path -Path $ManifestPath -PathType Leaf)) {
        throw "Cannot sign missing manifest: $ManifestPath"
    }

    $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($Cert)
    if (-not $rsa) {
        throw "Certificate '$($Cert.Thumbprint)' does not have an RSA private key."
    }

    try {
        $bytes = [System.IO.File]::ReadAllBytes($ManifestPath)
        $signature = $rsa.SignData(
            $bytes,
            [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
        )
        [System.IO.File]::WriteAllText($SignaturePath, [Convert]::ToBase64String($signature), [System.Text.UTF8Encoding]::new($false))
    }
    finally {
        $rsa.Dispose()
    }
}

function Get-CargoPackageVersion([string]$CargoTomlPath) {
    if (-not (Test-Path -Path $CargoTomlPath -PathType Leaf)) {
        throw "Cargo.toml not found at $CargoTomlPath"
    }
    $content = Get-Content -Path $CargoTomlPath
    foreach ($line in $content) {
        if ($line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $Matches[1]
        }
    }
    throw "Unable to find package version in $CargoTomlPath"
}

function Get-WindowsInstallerProductVersion([string]$CargoTomlPath, [string]$ProductName) {
    $version = Get-CargoPackageVersion -CargoTomlPath $CargoTomlPath
    $versionMatch = [regex]::Match(
        $version,
        '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
    )
    if (-not $versionMatch.Success) {
        throw "$ProductName Cargo package version '$version' is not a Windows Installer major.minor.build version. Pre-release/build metadata and fourth components are not supported."
    }

    $major = 0
    $minor = 0
    $build = 0
    if (
        -not [int]::TryParse($versionMatch.Groups[1].Value, [ref]$major) -or
        -not [int]::TryParse($versionMatch.Groups[2].Value, [ref]$minor) -or
        -not [int]::TryParse($versionMatch.Groups[3].Value, [ref]$build) -or
        $major -gt 255 -or
        $minor -gt 255 -or
        $build -gt 65535
    ) {
        throw "$ProductName Cargo package version '$version' exceeds Windows Installer limits (major/minor <= 255, build <= 65535)."
    }

    return $version
}

function Test-IsNonReleaseProfile([string]$Profile) {
    return $Profile -eq "dev"
}

function Get-NormalizedBuildParts([string[]]$Parts) {
    $normalized = [System.Collections.Generic.List[string]]::new()
    foreach ($part in $Parts) {
        switch ($part) {
            "all" { $normalized.Add("all") }
            "agent" { $normalized.Add("talos_worker") }
            "worker" { $normalized.Add("talos_worker") }
            "helper" { $normalized.Add("talos_worker_helper") }
            "worker-helper" { $normalized.Add("talos_worker_helper") }
            "updater" { $normalized.Add("talos_supervisor") }
            "supervisor" { $normalized.Add("talos_supervisor") }
            "viewer" { $normalized.Add("talos_viewer") }
            "viewer-updater" { $normalized.Add("talos_viewer_updater") }
            "agent-chat" { $normalized.Add("talos_worker_chat") }
            "worker-chat" { $normalized.Add("talos_worker_chat") }
            "linux" { $normalized.Add("talos_linux") }
            "windows" { $normalized.Add("talos_windows_installers") }
            "windows-installers" { $normalized.Add("talos_windows_installers") }
            "wix" { $normalized.Add("talos_windows_installers") }
            default { $normalized.Add($part) }
        }
    }

    return @($normalized | Select-Object -Unique)
}

function Test-BuildPartSelected([string[]]$NormalizedParts, [string]$Part) {
    return ($NormalizedParts -contains "all") -or ($NormalizedParts -contains $Part)
}

function Select-OrderedWorkerArchitectures([string[]]$Architectures, [string[]]$AllWorkerArchitectures) {
    $selected = [System.Collections.Generic.List[string]]::new()
    foreach ($workerArch in $AllWorkerArchitectures) {
        if ($Architectures -contains $workerArch) {
            $selected.Add($workerArch)
        }
    }

    return @($selected)
}

function Get-WorkerBuildPartArchitectures([string]$Part, [string]$Prefix, [string[]]$AllWorkerArchitectures) {
    if (($Part -eq $Prefix) -or ($Part -eq "${Prefix}_all")) {
        return @($AllWorkerArchitectures)
    }

    if ($Part -eq "${Prefix}_i386") {
        return @("x86")
    }

    $escapedPrefix = [regex]::Escape($Prefix)
    if ($Part -match "^${escapedPrefix}_x86_64_v([1-4])$") {
        return @("x64-v$($Matches[1])")
    }

    return @()
}

function Get-WorkerPackageArchitectures(
    [string[]]$NormalizedParts,
    [string]$Prefix,
    [string[]]$AllWorkerArchitectures
) {
    if (($NormalizedParts -contains "all") -or ($NormalizedParts -contains "talos_windows_installers")) {
        return @($AllWorkerArchitectures)
    }

    $architectures = [System.Collections.Generic.List[string]]::new()
    foreach ($part in $NormalizedParts) {
        foreach ($workerArch in (Get-WorkerBuildPartArchitectures -Part $part -Prefix $Prefix -AllWorkerArchitectures $AllWorkerArchitectures)) {
            $architectures.Add($workerArch)
        }
    }

    return Select-OrderedWorkerArchitectures -Architectures @($architectures) -AllWorkerArchitectures $AllWorkerArchitectures
}

function Test-StringArrayEqual([string[]]$Left, [string[]]$Right) {
    if ($Left.Count -ne $Right.Count) {
        return $false
    }

    for ($i = 0; $i -lt $Left.Count; $i++) {
        if ($Left[$i] -ne $Right[$i]) {
            return $false
        }
    }

    return $true
}

function Get-LinuxTargetTriple([string]$Arch) {
    switch ($Arch) {
        "linux-x64" { return "x86_64-unknown-linux-gnu" }
        "linux-x86" { return "i686-unknown-linux-gnu" }
        "linux-arm64" { return "aarch64-unknown-linux-gnu" }
        "linux-arm" { return "armv7-unknown-linux-gnueabihf" }
        default { throw "Unsupported Linux architecture: $Arch" }
    }
}

function Convert-ArchToManifestSuffix([string]$Arch) {
    return (($Arch -split "-") | ForEach-Object {
        $_.Substring(0, 1).ToUpperInvariant() + $_.Substring(1)
    }) -join ""
}

function Get-SevenZipCompressionLevel([string]$Profile) {
    if (Test-IsNonReleaseProfile -Profile $Profile) {
        return 1
    }

    return 9
}

function Get-LatestWriteTimeUtc([string[]]$Paths) {
    if (-not $Paths -or $Paths.Count -eq 0) {
        return [datetime]::MinValue
    }

    $latestWriteTimeUtc = [datetime]::MinValue
    foreach ($path in $Paths) {
        if ([string]::IsNullOrWhiteSpace($path)) {
            continue
        }

        if (-not (Test-Path -Path $path)) {
            throw "Required input path not found: $path"
        }

        $item = Get-Item -Path $path -ErrorAction Stop
        if ($item.LastWriteTimeUtc -gt $latestWriteTimeUtc) {
            $latestWriteTimeUtc = $item.LastWriteTimeUtc
        }
    }

    return $latestWriteTimeUtc
}

function Test-OutputUpToDate([string]$OutputPath, [string[]]$InputPaths) {
    if (-not (Test-Path -Path $OutputPath -PathType Leaf)) {
        return $false
    }

    if (-not $InputPaths -or $InputPaths.Count -eq 0) {
        return $true
    }

    $outputWriteTimeUtc = (Get-Item -Path $OutputPath -ErrorAction Stop).LastWriteTimeUtc
    $latestInputWriteTimeUtc = Get-LatestWriteTimeUtc -Paths $InputPaths
    return $outputWriteTimeUtc -ge $latestInputWriteTimeUtc
}

function Copy-ItemIfChanged([string]$SourcePath, [string]$DestinationPath) {
    if (-not (Test-Path -Path $SourcePath -PathType Leaf)) {
        throw "Cannot copy missing file: $SourcePath"
    }

    $destinationDir = Split-Path -Parent $DestinationPath
    if (-not [string]::IsNullOrWhiteSpace($destinationDir)) {
        New-Item -ItemType Directory -Force -Path $destinationDir | Out-Null
    }

    $sourceItem = Get-Item -Path $SourcePath -ErrorAction Stop
    if (Test-Path -Path $DestinationPath -PathType Leaf) {
        $destinationItem = Get-Item -Path $DestinationPath -ErrorAction Stop
        if (
            $destinationItem.Length -eq $sourceItem.Length -and
            $destinationItem.LastWriteTimeUtc -eq $sourceItem.LastWriteTimeUtc
        ) {
            return $false
        }
        if ($destinationItem.Length -eq $sourceItem.Length) {
            $sourceHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $SourcePath).Hash
            $destinationHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $DestinationPath).Hash
            if ($sourceHash -eq $destinationHash) {
                try {
                    $destinationItem.LastWriteTimeUtc = $sourceItem.LastWriteTimeUtc
                }
                catch {
                    Write-Warning "Payload already matches but timestamp could not be updated: $DestinationPath ($($_.Exception.Message))"
                }
                return $false
            }
        }
    }

    $destinationLeaf = Split-Path -Leaf $DestinationPath
    $tempPath = Join-Path $destinationDir (".$destinationLeaf.$([guid]::NewGuid().ToString("N")).tmp")
    Copy-Item -Force -LiteralPath $SourcePath -Destination $tempPath
    (Get-Item -LiteralPath $tempPath -ErrorAction Stop).LastWriteTimeUtc = $sourceItem.LastWriteTimeUtc

    $maxAttempts = 8
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        try {
            Move-Item -Force -LiteralPath $tempPath -Destination $DestinationPath
            return $true
        }
        catch {
            if ($attempt -ge $maxAttempts) {
                Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath $tempPath
                throw "Failed to replace '$DestinationPath' after $maxAttempts attempts. Close any process using that file and retry. Last error: $($_.Exception.Message)"
            }
            Start-Sleep -Milliseconds (250 * $attempt)
        }
    }

    return $true
}

function Start-LoggedProcess(
    [string]$Description,
    [string]$WorkingDirectory,
    [string]$FilePath,
    [string[]]$Arguments
) {
    $logRoot = Join-Path $env:TEMP "build-installers-logs"
    New-Item -ItemType Directory -Force -Path $logRoot | Out-Null

    $safeName = ($Description -replace "[^A-Za-z0-9\.-]+", "-").Trim("-")
    if ([string]::IsNullOrWhiteSpace($safeName)) {
        $safeName = "process"
    }

    $logSuffix = [guid]::NewGuid().ToString("N")
    $stdoutPath = Join-Path $logRoot "$safeName.$logSuffix.stdout.log"
    $stderrPath = Join-Path $logRoot "$safeName.$logSuffix.stderr.log"

    $process = Start-Process `
        -FilePath $FilePath `
        -ArgumentList $Arguments `
        -WorkingDirectory $WorkingDirectory `
        -PassThru `
        -WindowStyle Hidden `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath

    return [pscustomobject]@{
        Description = $Description
        Process = $process
        StdoutPath = $stdoutPath
        StderrPath = $stderrPath
    }
}

function Wait-LoggedProcesses([object[]]$ProcessInfos) {
    foreach ($processInfo in $ProcessInfos) {
        $null = $processInfo.Process.WaitForExit()
    }

    foreach ($processInfo in $ProcessInfos) {
        if ($processInfo.Process.ExitCode -eq 0) {
            Remove-Item -Force -ErrorAction SilentlyContinue $processInfo.StdoutPath
            Remove-Item -Force -ErrorAction SilentlyContinue $processInfo.StderrPath
            continue
        }

        Write-Host ""
        Write-Host "Output from $($processInfo.Description):"
        if (Test-Path -Path $processInfo.StdoutPath -PathType Leaf) {
            Get-Content -Path $processInfo.StdoutPath
        }
        if (Test-Path -Path $processInfo.StderrPath -PathType Leaf) {
            Get-Content -Path $processInfo.StderrPath
        }

        throw "$($processInfo.Description) failed with exit code $($processInfo.Process.ExitCode)."
    }
}

# Script lives in <repo>/scripts; Rust/WiX inputs are under <repo>/apps
$scriptsDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = (Resolve-Path (Join-Path $scriptsDir "..")).Path
$appsRoot = (Resolve-Path (Join-Path $repoRoot "apps")).Path

$installerRoot = Join-Path $appsRoot "installer"
$installerBuildTargetsPath = Join-Path $installerRoot "Directory.Build.targets"
$agentInstallerCargoTomlPath = Join-Path $appsRoot "talos_worker\Cargo.toml"
$viewerInstallerCargoTomlPath = Join-Path $appsRoot "talos_viewer\src-tauri\Cargo.toml"
$wixToolManifestPath = Join-Path $repoRoot ".config\dotnet-tools.json"
$thirdPartyAcquisitionPolicyPath = Join-Path $repoRoot ".config\third-party-acquisition.json"
$thirdPartyAcquisitionScriptPath = Join-Path $appsRoot "scripts\third-party-acquisition.ts"
$payloadRoot = Join-Path $installerRoot "payload"
$payloadX64 = Join-Path $payloadRoot "x64"
$payloadX86 = Join-Path $payloadRoot "x86"
$allWorkerArchitectures = @("x64-v1", "x64-v2", "x64-v3", "x64-v4", "x86")
$linuxArchitectures = @($LinuxArch | Select-Object -Unique)
$linuxPayloadDirs = @{}
foreach ($linuxArchName in $linuxArchitectures) {
    $linuxPayloadDirs[$linuxArchName] = Join-Path $payloadRoot "linux\$linuxArchName"
}
$viewerPayloadX64 = Join-Path $payloadRoot "viewer\x64"
$artifactRoot = Join-Path $installerRoot "artifacts"
$artifactProfileDir = Join-Path $artifactRoot $BuildProfile
$workerBuildCacheRoot = Join-Path $installerRoot "tmp\worker-builds\$BuildProfile"
$prereqRoot = Join-Path $installerRoot "prereqs"
$vcRedistX86Path = Join-Path $prereqRoot "vc_redist.x86.exe"
$vcRedistX64Path = Join-Path $prereqRoot "vc_redist.x64.exe"
$webView2RuntimePath = Join-Path $prereqRoot "MicrosoftEdgeWebView2RuntimeInstallerX64.exe"
# Resolved from Microsoft's official VS 17 permalinks on 2026-08-17. The content digests are
# embedded in these immutable Microsoft Download paths and independently verified below.
$vcRedistX86Url = "https://download.visualstudio.microsoft.com/download/pr/9d270333-8b7b-4f96-9458-6fcdb2ec0b25/0C09F2611660441084CE0DF425C51C11E147E6447963C3690F97E0B25C55ED64/VC_redist.x86.exe"
$vcRedistX86Sha256 = "0c09f2611660441084ce0df425c51c11e147e6447963c3690f97e0b25c55ed64"
$vcRedistX64Url = "https://download.visualstudio.microsoft.com/download/pr/9d270333-8b7b-4f96-9458-6fcdb2ec0b25/CC0FF0EB1DC3F5188AE6300FAEF32BF5BEEBA4BDD6E8E445A9184072096B713B/VC_redist.x64.exe"
$vcRedistX64Sha256 = "cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b"
$webView2RuntimeUrl = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"

$targetTripleX64 = "x86_64-pc-windows-msvc"
$targetTripleX86 = "i686-pc-windows-msvc"
$normalizedBuildParts = Get-NormalizedBuildParts -Parts $BuildPart
$isScopedBinaryBuild = -not ($normalizedBuildParts -contains "all")
$shouldBuildInstallers = (-not $isScopedBinaryBuild) -or $BuildInstallers.IsPresent
$isBinaryOnlyBuild = ($BuildProfile -eq "debug") -or (-not $shouldBuildInstallers)
$buildLinux = Test-BuildPartSelected -NormalizedParts $normalizedBuildParts -Part "talos_linux"
$buildWindowsInstallerAlias = $normalizedBuildParts -contains "talos_windows_installers"
$buildLinuxInDocker = $buildLinux -and (Test-IsWindowsHost)
$isLinuxOnlyInstallerBuild =
    $shouldBuildInstallers -and
    $isScopedBinaryBuild -and
    ($normalizedBuildParts.Count -eq 1) -and
    $buildLinux
$agentWorkerArchitectures = Get-WorkerPackageArchitectures -NormalizedParts $normalizedBuildParts -Prefix "talos_worker" -AllWorkerArchitectures $allWorkerArchitectures
$helperWorkerArchitectures = Get-WorkerPackageArchitectures -NormalizedParts $normalizedBuildParts -Prefix "talos_worker_helper" -AllWorkerArchitectures $allWorkerArchitectures
$buildAgent = $agentWorkerArchitectures.Count -gt 0
$buildHelper = $helperWorkerArchitectures.Count -gt 0
$buildUpdater = (Test-BuildPartSelected -NormalizedParts $normalizedBuildParts -Part "talos_supervisor") -or $buildWindowsInstallerAlias
$buildViewer = (Test-BuildPartSelected -NormalizedParts $normalizedBuildParts -Part "talos_viewer") -or $buildWindowsInstallerAlias
$buildViewerUpdater = (Test-BuildPartSelected -NormalizedParts $normalizedBuildParts -Part "talos_viewer_updater") -or $buildWindowsInstallerAlias
$buildAgentChat = (Test-BuildPartSelected -NormalizedParts $normalizedBuildParts -Part "talos_worker_chat") -or $buildWindowsInstallerAlias
$chatWorkerArchitectures = @()
if ($buildAgentChat) {
    $selectedWorkerArchitectures = @($agentWorkerArchitectures + $helperWorkerArchitectures)
    if ($selectedWorkerArchitectures.Count -gt 0) {
        $chatWorkerArchitectures = Select-OrderedWorkerArchitectures -Architectures $selectedWorkerArchitectures -AllWorkerArchitectures $allWorkerArchitectures
    }
    elseif (-not $isBinaryOnlyBuild) {
        $chatWorkerArchitectures = @($allWorkerArchitectures)
    }
}
$workerArchitectures = Select-OrderedWorkerArchitectures -Architectures @($agentWorkerArchitectures + $helperWorkerArchitectures + $chatWorkerArchitectures) -AllWorkerArchitectures $allWorkerArchitectures
$workerPayloadDirs = @{}
foreach ($workerArch in $workerArchitectures) {
    $workerPayloadDirs[$workerArch] = Join-Path $payloadRoot "worker\$workerArch"
}
$workerPayloadX64 = $workerPayloadDirs["x64-v1"]
$workerPayloadX86 = $workerPayloadDirs["x86"]
$buildWindowsInstallerArtifacts = $shouldBuildInstallers -and (-not $isLinuxOnlyInstallerBuild)
$buildWorkerUpdateArtifacts =
    $buildWindowsInstallerArtifacts -and
    ((-not $isScopedBinaryBuild) -or $buildWindowsInstallerAlias -or ($buildAgent -and $buildHelper -and $buildAgentChat))
$buildSupervisorArtifacts =
    $buildWindowsInstallerArtifacts -and
    ((-not $isScopedBinaryBuild) -or $buildWindowsInstallerAlias -or $buildUpdater)
$buildViewerArtifacts =
    $buildWindowsInstallerArtifacts -and
    ((-not $isScopedBinaryBuild) -or $buildWindowsInstallerAlias -or ($buildViewer -and $buildViewerUpdater))
$requiresWindowsMediaBuildPrereqs =
    $buildAgent -or
    $buildHelper -or
    $buildViewer -or
    $buildViewerUpdater -or
    $buildAgentChat
$requiresWindowsX86Target =
    $buildUpdater -or
    (($buildAgent -or $buildHelper -or $buildAgentChat) -and ($workerArchitectures -contains "x86"))
$buildWorkerMatrix = $workerArchitectures.Count -gt 0

$cargoProfileDirName = Get-CargoProfileDirName -Profile $BuildProfile
$releaseX64 = Join-Path $appsRoot "target\$targetTripleX64\$cargoProfileDirName"
$releaseX86 = Join-Path $appsRoot "target\$targetTripleX86\$cargoProfileDirName"
$workerBuildOutputs = @{}
$linuxBuildOutputs = @{}

$agentExe = "talos_worker.exe"
$helperExe = "talos_worker_helper.exe"
$updaterExe = "talos_supervisor.exe"
$supervisorExe = "talos_supervisor.exe"
$workerExe = "talos_worker.exe"
$workerHelperExe = "talos_worker_helper.exe"
$workerChatExe = "talos_worker_chat.exe"
$viewerExe = "talos_viewer.exe"
$viewerUpdaterExe = "talos_viewer_updater.exe"
$agentChatExe = "talos_worker_chat.exe"
$linuxWorkerBinary = "talos_worker"
$linuxSupervisorBinary = "talos_supervisor"
$linuxInstallerBinaryName = "talos-rmm-agent-linux-x64"

Write-Step "Using apps root: $appsRoot"
Write-Step "Using installer root: $installerRoot"
Write-Step "Using build profile: $BuildProfile"
Write-Step "Worker architectures: $($workerArchitectures -join ", ")"
if ($buildLinux) {
    Write-Step "Linux update architectures: $($linuxArchitectures -join ", ")"
    if ($buildLinuxInDocker) {
        Write-Step "Linux builds will run in Docker because the host shell is Windows"
    }
}
if ($isScopedBinaryBuild) {
    Write-Step "Scoped binary build parts: $($normalizedBuildParts -join ", ")"
}
if ($isBinaryOnlyBuild) {
    $debugSignNote = if ($SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning) {
        "Authenticode will run on cargo outputs; "
    }
    else {
        ""
    }
    if ($isScopedBinaryBuild) {
        Write-Step "Scoped build selected: building requested binaries only; ${debugSignNote}skipping WiX, archives, manifests, and installer publishing"
    }
    else {
        Write-Step "Debug profile selected: building x64 binaries only; ${debugSignNote}skipping WiX, archives, manifests, and installer publishing"
    }
    if ($BuildX86) {
        Write-Warning "-BuildX86 is deprecated; full dev/release builds always build all worker architectures."
    }
    if ($buildLinux -and -not $BuildInstallers.IsPresent) {
        Write-Warning "Scoped/debug talos_linux builds only emit cargo binaries. Add -BuildInstallers to publish Linux installer artifacts and manifests."
    }
}
elseif ($isScopedBinaryBuild -and $BuildInstallers.IsPresent -and -not $isLinuxOnlyInstallerBuild) {
    if (-not ($buildWorkerUpdateArtifacts -or $buildSupervisorArtifacts -or $buildViewerArtifacts)) {
        throw "-BuildInstallers with scoped -BuildPart requires a complete installer target. Use -BuildPart wix for all Windows WiX artifacts, -BuildPart talos_linux for Linux artifacts, talos_supervisor for Windows supervisor/MSI artifacts, matching talos_worker_<isa>,talos_worker_helper_<isa>,talos_worker_chat for Windows worker update artifacts, or talos_viewer,talos_viewer_updater for viewer artifacts."
    }
    if (($buildAgent -or $buildHelper -or $buildAgentChat) -and -not ($buildAgent -and $buildHelper -and $buildAgentChat)) {
        throw "Windows worker update artifacts require talos_worker_<isa>, talos_worker_helper_<isa>, and talos_worker_chat together."
    }
    if ($buildWorkerUpdateArtifacts -and -not (Test-StringArrayEqual -Left $agentWorkerArchitectures -Right $helperWorkerArchitectures)) {
        throw "Windows worker update artifacts require matching worker/helper ISA selections. Use matching pairs such as talos_worker_x86_64_v3,talos_worker_helper_x86_64_v3,talos_worker_chat or talos_worker_all,talos_worker_helper_all,talos_worker_chat."
    }
    if (($buildViewer -or $buildViewerUpdater) -and -not ($buildViewer -and $buildViewerUpdater)) {
        throw "Windows viewer installer artifacts require talos_viewer and talos_viewer_updater together."
    }
}

if (-not (Test-Path -Path $installerRoot -PathType Container)) {
    throw "Installer root not found at $installerRoot"
}

$bunExecutable = Get-BunExecutablePath
Invoke-Checked "Preparing the pinned vpx-encode 0.6.2 source" {
    & $bunExecutable $thirdPartyAcquisitionScriptPath vpx --repo-root $repoRoot
}

$pinnedInstallerInputs = $null
if (-not $isBinaryOnlyBuild) {
    $pinnedInstallerInputs = Get-PinnedInstallerInputs `
        -RepoRoot $repoRoot `
        -AcquisitionScriptPath $thirdPartyAcquisitionScriptPath `
        -PolicyPath $thirdPartyAcquisitionPolicyPath `
        -ExplicitSevenZipPath $SevenZipPath
}

$agentInstallerVersion = $null
if ($buildSupervisorArtifacts) {
    $agentInstallerVersion = Get-WindowsInstallerProductVersion `
        -CargoTomlPath $agentInstallerCargoTomlPath `
        -ProductName "Talos Agent"
    Write-Step "Talos Agent MSI/Burn product version: $agentInstallerVersion (talos_worker Cargo package)"
}

$viewerInstallerVersion = $null
if ($buildViewerArtifacts) {
    $viewerInstallerVersion = Get-WindowsInstallerProductVersion `
        -CargoTomlPath $viewerInstallerCargoTomlPath `
        -ProductName "Talos Viewer"
    Write-Step "Talos Viewer MSI product version: $viewerInstallerVersion (talos_viewer Cargo package)"
}

$cert = $null
$manifestCert = $null
$manifestPublicKeyDerPath = $null
$authenticodeExpectedThumbprint = $null
$externalAuthenticodeSignerPath = $null
$authenticodeSigningEnabled = $SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning
if ($authenticodeSigningEnabled) {
    $authenticodeExpectedThumbprint = Normalize-CertificateThumbprint `
        -Thumbprint $CertificateThumbprint `
        -Label "-CertificateThumbprint"
    $externalAuthenticodeSignerPath = Get-ExternalAuthenticodeSignerPath `
        -Path $ExternalAuthenticodeSignerPath
    if ($externalAuthenticodeSignerPath) {
        Write-Step "Using external Authenticode signer adapter: $externalAuthenticodeSignerPath"
        Write-Host "Expected Authenticode certificate thumbprint: $authenticodeExpectedThumbprint"
    }
    else {
        $cert = Get-CodeSigningCert -Thumbprint $authenticodeExpectedThumbprint
        Write-Step "Using Authenticode signing certificate: $($cert.Subject) [$($cert.Thumbprint)]"
    }
}
elseif ($SignAuthenticodeBinaries -and $SkipAuthenticodeSigning) {
    Write-Warning "-SkipAuthenticodeSigning overrides -SignAuthenticodeBinaries; selected Windows binaries and installers will not be Authenticode signed."
}

if (-not $isBinaryOnlyBuild) {
    $manifestCert = Get-ManifestSigningCert `
        -CertificatePath $ManifestCertificatePath `
        -CertificatePassword $ManifestCertificatePassword `
        -Thumbprint $ManifestCertificateThumbprint `
        -ForbiddenRoot $repoRoot
    $manifestPublicKeyDerPath = Join-Path $installerRoot "tmp\manifest_public_key.der"
    Export-PublicKeyDer -Cert $manifestCert -DestinationPath $manifestPublicKeyDerPath
    $manifestSource = if (-not [string]::IsNullOrWhiteSpace($ManifestCertificatePath)) {
        "explicit PFX"
    }
    else {
        "certificate store"
    }
    Write-Step "Using manifest signing certificate from ${manifestSource}: $($manifestCert.Subject) [$($manifestCert.Thumbprint)]"
    $manifestPublicKeySha256 = (Get-FileHash -LiteralPath $manifestPublicKeyDerPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "Manifest public key SHA-256: $manifestPublicKeySha256"
}

Push-Location $appsRoot
try {
    Ensure-RustTarget -TargetTriple $targetTripleX64
    if ($requiresWindowsX86Target) {
        Ensure-RustTarget -TargetTriple $targetTripleX86
    }
    if ($buildLinux -and -not $buildLinuxInDocker) {
        foreach ($linuxArchName in $linuxArchitectures) {
            Ensure-RustTarget -TargetTriple (Get-LinuxTargetTriple -Arch $linuxArchName)
        }
    }
    if ($requiresWindowsMediaBuildPrereqs) {
        Ensure-NasmAvailable
    }

    $previousRustFlags = $env:RUSTFLAGS
    $previousVpxLibDir = $env:VPX_LIB_DIR
    $previousVpxIncludeDir = $env:VPX_INCLUDE_DIR
    $previousVpxVersion = $env:VPX_VERSION
    $previousManifestPublicKeyPath = $env:RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH
    $previousLibClangPath = $env:LIBCLANG_PATH
    $usedX86Fallback = $false
    $x64AgentPath = $null
    $x64HelperPath = $null
    $x64UpdaterPath = $null
    $x86AgentPath = $null
    $x86HelperPath = $null
    $x86UpdaterPath = $null
    $x86AgentChatPath = $null
    $x64ViewerPath = $null
    $x64ViewerUpdaterPath = $null
    $x64AgentChatPath = $null
    try {
        if (-not $isBinaryOnlyBuild) {
            $env:RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH = $manifestPublicKeyDerPath
        }
        if ($requiresWindowsMediaBuildPrereqs) {
            Set-LibClangEnv
        }
        $cargoArgsX64 = Get-CargoBuildArgs -Profile $BuildProfile -TargetTriple $targetTripleX64
        $x64AgentPath = Join-Path $releaseX64 $agentExe
        $x64HelperPath = Join-Path $releaseX64 $helperExe
        $x64UpdaterPath = $null
        $x86UpdaterPath = Join-Path $releaseX86 $updaterExe
        $x64ViewerPath = Join-Path $releaseX64 $viewerExe
        $x64ViewerUpdaterPath = Join-Path $releaseX64 $viewerUpdaterExe
        $x64AgentChatPath = Join-Path $releaseX64 $agentChatExe

        if ($buildHelper -and -not $buildWorkerMatrix) {
            $helperUpdaterPackages = @()
            if ($buildHelper) { $helperUpdaterPackages += "talos_worker_helper" }
            if ($buildHelper) {
                Set-VpxEnvForTarget -TargetTriple $targetTripleX64
            }
            $helperUpdaterCargoPackageArgs = @($helperUpdaterPackages | ForEach-Object { @("-p", $_) })
            Invoke-Checked "Building $BuildProfile $($helperUpdaterPackages -join ", ") for $targetTripleX64" {
                & cargo @cargoArgsX64 @helperUpdaterCargoPackageArgs
            }
            $helperUpdaterExpectedPaths = @()
            if ($buildHelper) { $helperUpdaterExpectedPaths += $x64HelperPath }
            foreach ($path in $helperUpdaterExpectedPaths) {
                if (-not (Test-Path -Path $path -PathType Leaf)) {
                    throw "Expected x64 build output not found: $path"
                }
            }
        }
        if ($buildUpdater) {
            $cargoArgsX86 = Get-CargoBuildArgs -Profile $BuildProfile -TargetTriple $targetTripleX86
            Invoke-Checked "Building $BuildProfile supervisor binary for $targetTripleX86" {
                & cargo @cargoArgsX86 -p talos_supervisor
            }
            if (-not (Test-Path -Path $x86UpdaterPath -PathType Leaf)) {
                throw "Expected x86 supervisor build output not found: $x86UpdaterPath"
            }
        }
        if ($buildAgent -and -not $buildWorkerMatrix) {
            Set-VpxEnvForTarget -TargetTriple $targetTripleX64
            Invoke-Checked "Building $BuildProfile agent binary for $targetTripleX64" {
                & cargo @cargoArgsX64 -p talos_worker --features windows-resource
            }
            if (-not (Test-Path -Path $x64AgentPath -PathType Leaf)) {
                throw "Expected x64 build output not found: $x64AgentPath"
            }
        }

        if ($buildViewer -or $buildViewerUpdater) {
            $rmmViewerAppDir = Join-Path $appsRoot "talos_viewer"
            Invoke-Checked "Installing talos_viewer UI dependencies (vite; required by src-tauri/build.rs)" {
                Push-Location $appsRoot
                try {
                    & bun install --frozen-lockfile --filter talos_viewer
                }
                finally {
                    Pop-Location
                }
            }

            Set-VpxEnvForTarget -TargetTriple $targetTripleX64
            $viewerPackages = @()
            if ($buildViewer) { $viewerPackages += "talos_viewer" }
            if ($buildViewerUpdater) { $viewerPackages += "talos_viewer_updater" }
            $viewerCargoPackageArgs = @($viewerPackages | ForEach-Object { @("-p", $_) })
            Invoke-Checked "Building $BuildProfile $($viewerPackages -join ", ") for $targetTripleX64" {
                & cargo @cargoArgsX64 @viewerCargoPackageArgs
            }
            $viewerExpectedPaths = @()
            if ($buildViewer) { $viewerExpectedPaths += $x64ViewerPath }
            if ($buildViewerUpdater) { $viewerExpectedPaths += $x64ViewerUpdaterPath }
            foreach ($path in $viewerExpectedPaths) {
                if (-not (Test-Path -Path $path -PathType Leaf)) {
                    throw "Expected viewer build output not found: $path"
                }
            }
        }

        if ($buildAgentChat) {
            $rmmAgentChatAppDir = Join-Path $appsRoot "talos_worker_chat"
            Invoke-Checked "Installing talos_worker_chat UI dependencies (vite; required by src-tauri/build.rs)" {
                Push-Location $appsRoot
                try {
                    & bun install --frozen-lockfile --filter talos_worker_chat
                }
                finally {
                    Pop-Location
                }
            }
        }

        if ($buildAgentChat -and -not $buildWorkerMatrix) {
            Set-VpxEnvForTarget -TargetTriple $targetTripleX64
            Invoke-Checked "Building $BuildProfile talos_worker_chat binary for $targetTripleX64" {
                & cargo @cargoArgsX64 -p talos_worker_chat
            }
            if (-not (Test-Path -Path $x64AgentChatPath -PathType Leaf)) {
                throw "Expected x64 talos_worker_chat build output not found: $x64AgentChatPath"
            }
        }

        if ($buildWorkerMatrix) {
            foreach ($workerArch in $workerArchitectures) {
                $targetTriple = if ($workerArch -eq "x86") { $targetTripleX86 } else { $targetTripleX64 }
                $releaseDir = if ($workerArch -eq "x86") { $releaseX86 } else { $releaseX64 }
                $cargoArgs = Get-CargoBuildArgs -Profile $BuildProfile -TargetTriple $targetTriple
                $workerRustFlags = Get-WorkerRustFlags -WorkerArch $workerArch
                $env:RUSTFLAGS = Join-RustFlags @($previousRustFlags, $workerRustFlags)
                Set-VpxEnvForTarget -TargetTriple $targetTriple

                $workerPackages = @()
                $buildHelperForArch = $helperWorkerArchitectures -contains $workerArch
                $buildAgentChatForArch = $chatWorkerArchitectures -contains $workerArch
                $buildAgentForArch = $agentWorkerArchitectures -contains $workerArch
                if ($buildHelperForArch) { $workerPackages += "talos_worker_helper" }
                if ($buildAgentChatForArch) { $workerPackages += "talos_worker_chat" }
                if ($workerPackages.Count -gt 0) {
                    $workerPackageArgs = @($workerPackages | ForEach-Object { @("-p", $_) })
                    Invoke-Checked "Building $BuildProfile $($workerPackages -join ", ") for $workerArch ($targetTriple)" {
                        & cargo @cargoArgs @workerPackageArgs
                    }
                }
                if ($buildAgentForArch) {
                    Invoke-Checked "Building $BuildProfile agent binary for $workerArch ($targetTriple)" {
                        & cargo @cargoArgs -p talos_worker --features windows-resource
                    }
                }

                $workerOutput = [ordered]@{}
                $workerCacheDir = Join-Path $workerBuildCacheRoot $workerArch
                New-Item -ItemType Directory -Force -Path $workerCacheDir | Out-Null
                if ($buildAgentForArch) {
                    $path = Join-Path $releaseDir $agentExe
                    if (-not (Test-Path -Path $path -PathType Leaf)) {
                        throw "Expected $workerArch agent build output not found: $path"
                    }
                    $cachedPath = Join-Path $workerCacheDir $agentExe
                    [void](Copy-ItemIfChanged -SourcePath $path -DestinationPath $cachedPath)
                    $workerOutput.Agent = $cachedPath
                }
                if ($buildHelperForArch) {
                    $path = Join-Path $releaseDir $helperExe
                    if (-not (Test-Path -Path $path -PathType Leaf)) {
                        throw "Expected $workerArch helper build output not found: $path"
                    }
                    $cachedPath = Join-Path $workerCacheDir $helperExe
                    [void](Copy-ItemIfChanged -SourcePath $path -DestinationPath $cachedPath)
                    $workerOutput.Helper = $cachedPath
                }
                if ($buildAgentChatForArch) {
                    $path = Join-Path $releaseDir $agentChatExe
                    if (-not (Test-Path -Path $path -PathType Leaf)) {
                        throw "Expected $workerArch worker chat build output not found: $path"
                    }
                    $cachedPath = Join-Path $workerCacheDir $agentChatExe
                    [void](Copy-ItemIfChanged -SourcePath $path -DestinationPath $cachedPath)
                    $workerOutput.Chat = $cachedPath
                }
                $workerBuildOutputs[$workerArch] = [pscustomobject]$workerOutput
            }

            if ($workerBuildOutputs.ContainsKey("x86")) {
                $x86AgentPath = $workerBuildOutputs["x86"].Agent
                $x86HelperPath = $workerBuildOutputs["x86"].Helper
                $x86AgentChatPath = $workerBuildOutputs["x86"].Chat
            }
            if ($workerBuildOutputs.ContainsKey("x64-v1")) {
                $x64AgentPath = $workerBuildOutputs["x64-v1"].Agent
                $x64HelperPath = $workerBuildOutputs["x64-v1"].Helper
                $x64AgentChatPath = $workerBuildOutputs["x64-v1"].Chat
            }
        }

        if ($BuildX86 -and -not $isBinaryOnlyBuild) {
            Write-Warning "-BuildX86 is deprecated; this build already produced real x86 worker binaries."
        }

        if ($buildLinux) {
            foreach ($linuxArchName in $linuxArchitectures) {
                if ($buildLinuxInDocker -and $linuxArchName -ne "linux-x64") {
                    throw "Docker-based Linux builds currently support linux-x64 only. Run on a native Linux builder or extend the container toolchain before using $linuxArchName."
                }
                $linuxTargetTriple = Get-LinuxTargetTriple -Arch $linuxArchName
                $linuxReleaseDir = Join-Path $appsRoot "target\$linuxTargetTriple\$cargoProfileDirName"
                $linuxCargoArgs = Get-CargoBuildArgs -Profile $BuildProfile -TargetTriple $linuxTargetTriple
                $linuxPackages = @()
                $linuxPackages += "talos_worker"
                $linuxPackages += "talos_supervisor"
                if ($linuxPackages.Count -eq 0) {
                    continue
                }

                $env:RUSTFLAGS = $previousRustFlags
                $env:VPX_LIB_DIR = $null
                $env:VPX_INCLUDE_DIR = $null
                $env:VPX_VERSION = $null
                if ($buildLinuxInDocker) {
                    Invoke-LinuxCargoBuildInDocker `
                        -RepoRoot $repoRoot `
                        -TargetTriple $linuxTargetTriple `
                        -Profile $BuildProfile `
                        -Packages $linuxPackages `
                        -ManifestPublicKeyDerPath $manifestPublicKeyDerPath
                }
                else {
                    $linuxPackageArgs = @($linuxPackages | ForEach-Object { @("-p", $_) })
                    Invoke-Checked "Building $BuildProfile Linux $($linuxPackages -join ", ") for $linuxArchName ($linuxTargetTriple)" {
                        & cargo @linuxCargoArgs @linuxPackageArgs
                    }
                }

                $linuxOutput = [ordered]@{}
                $linuxWorkerPath = Join-Path $linuxReleaseDir $linuxWorkerBinary
                if (-not (Test-Path -Path $linuxWorkerPath -PathType Leaf)) {
                    throw "Expected Linux worker build output not found: $linuxWorkerPath"
                }
                $linuxOutput.Worker = $linuxWorkerPath
                $linuxSupervisorPath = Join-Path $linuxReleaseDir $linuxSupervisorBinary
                if (-not (Test-Path -Path $linuxSupervisorPath -PathType Leaf)) {
                    throw "Expected Linux supervisor build output not found: $linuxSupervisorPath"
                }
                $linuxOutput.Supervisor = $linuxSupervisorPath
                $linuxBuildOutputs[$linuxArchName] = [pscustomobject]$linuxOutput
            }
        }
    }
    finally {
        $env:RUSTFLAGS = $previousRustFlags
        $env:VPX_LIB_DIR = $previousVpxLibDir
        $env:VPX_INCLUDE_DIR = $previousVpxIncludeDir
        $env:VPX_VERSION = $previousVpxVersion
        $env:RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH = $previousManifestPublicKeyPath
        $env:LIBCLANG_PATH = $previousLibClangPath
    }

    if ($isBinaryOnlyBuild) {
        $builtBinaryOutputs = @()
        if ($buildWorkerMatrix) {
            foreach ($workerArch in $workerArchitectures) {
                if (-not $workerBuildOutputs.ContainsKey($workerArch)) {
                    continue
                }

                $workerOutput = $workerBuildOutputs[$workerArch]
                if ($workerOutput.Agent) {
                    $builtBinaryOutputs += [pscustomobject]@{ Label = "Agent binary ($workerArch)"; Path = $workerOutput.Agent }
                }
                if ($workerOutput.Helper) {
                    $builtBinaryOutputs += [pscustomobject]@{ Label = "Agent helper binary ($workerArch)"; Path = $workerOutput.Helper }
                }
                if ($workerOutput.Chat) {
                    $builtBinaryOutputs += [pscustomobject]@{ Label = "Agent chat binary ($workerArch)"; Path = $workerOutput.Chat }
                }
            }
        }
        elseif ($buildAgent) {
            $builtBinaryOutputs += [pscustomobject]@{ Label = "Agent binary"; Path = $x64AgentPath }
        }
        if (-not $buildWorkerMatrix -and $buildHelper) {
            $builtBinaryOutputs += [pscustomobject]@{ Label = "Agent helper binary"; Path = $x64HelperPath }
        }
        if ($buildUpdater) {
            $builtBinaryOutputs += [pscustomobject]@{ Label = "Supervisor binary"; Path = $x86UpdaterPath }
        }
        if ($buildViewer) {
            $builtBinaryOutputs += [pscustomobject]@{ Label = "Viewer binary"; Path = $x64ViewerPath }
        }
        if ($buildViewerUpdater) {
            $builtBinaryOutputs += [pscustomobject]@{ Label = "Viewer updater binary"; Path = $x64ViewerUpdaterPath }
        }
        if (-not $buildWorkerMatrix -and $buildAgentChat) {
            $builtBinaryOutputs += [pscustomobject]@{ Label = "Agent chat binary"; Path = $x64AgentChatPath }
        }
        if ($buildLinux) {
            foreach ($linuxArchName in $linuxArchitectures) {
                if (-not $linuxBuildOutputs.ContainsKey($linuxArchName)) {
                    continue
                }
                $linuxOutput = $linuxBuildOutputs[$linuxArchName]
                if ($linuxOutput.Worker) {
                    $builtBinaryOutputs += [pscustomobject]@{ Label = "Linux worker binary ($linuxArchName)"; Path = $linuxOutput.Worker }
                }
                if ($linuxOutput.Supervisor) {
                    $builtBinaryOutputs += [pscustomobject]@{ Label = "Linux supervisor binary ($linuxArchName)"; Path = $linuxOutput.Supervisor }
                }
            }
        }

        Write-Host ""
        Write-Host "Binary build complete."
        foreach ($output in $builtBinaryOutputs) {
            Write-Host "$($output.Label):"
            Write-Host $output.Path
        }
        if ($SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning) {
            Write-Step "Signing cargo-built executables (pre-stage; profile folder: $cargoProfileDirName)"
            Sign-Binaries -Paths @($builtBinaryOutputs | ForEach-Object { $_.Path } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }) -Cert $cert -ExpectedThumbprint $authenticodeExpectedThumbprint -TimestampUrl $TimestampServer -ExternalSignerPath $externalAuthenticodeSignerPath
        }
        return
    }

    if ($isLinuxOnlyInstallerBuild) {
        Write-Step "Publishing scoped Linux installer artifacts"
        New-Item -ItemType Directory -Force -Path $artifactProfileDir | Out-Null

        foreach ($linuxArchName in $linuxArchitectures) {
            if (-not $linuxBuildOutputs.ContainsKey($linuxArchName)) {
                throw "Missing Linux build outputs for $linuxArchName."
            }
            $linuxPayloadDir = $linuxPayloadDirs[$linuxArchName]
            New-Item -ItemType Directory -Force -Path $linuxPayloadDir | Out-Null
            $linuxOutput = $linuxBuildOutputs[$linuxArchName]
            [void](Copy-ItemIfChanged -SourcePath $linuxOutput.Worker -DestinationPath (Join-Path $linuxPayloadDir $linuxWorkerBinary))
            [void](Copy-ItemIfChanged -SourcePath $linuxOutput.Supervisor -DestinationPath (Join-Path $linuxPayloadDir $linuxSupervisorBinary))
        }

        $publishedLinuxAgentBinary = Join-Path $artifactProfileDir $linuxInstallerBinaryName
        if ($linuxBuildOutputs.ContainsKey("linux-x64")) {
            [void](Copy-ItemIfChanged -SourcePath $linuxBuildOutputs["linux-x64"].Supervisor -DestinationPath $publishedLinuxAgentBinary)
        }

        $linuxWorkerUpdateZips = @{}
        $linuxWorkerUpdateManifests = @{}
        $linuxWorkerUpdateSignatures = @{}
        $linuxSupervisorUpdateZips = @{}
        $linuxSupervisorUpdateManifests = @{}
        $linuxSupervisorUpdateSignatures = @{}
        foreach ($linuxArchName in $linuxArchitectures) {
            $linuxWorkerUpdateZips[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Worker.$linuxArchName.Update.zip"
            $linuxWorkerUpdateManifests[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Worker.$linuxArchName.Update.manifest.json"
            $linuxWorkerUpdateSignatures[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Worker.$linuxArchName.Update.manifest.sig"
            $linuxSupervisorUpdateZips[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Supervisor.$linuxArchName.Update.zip"
            $linuxSupervisorUpdateManifests[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Supervisor.$linuxArchName.Update.manifest.json"
            $linuxSupervisorUpdateSignatures[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Supervisor.$linuxArchName.Update.manifest.sig"
        }

        $agentVersion = Get-CargoPackageVersion -CargoTomlPath (Join-Path $appsRoot "talos_worker\Cargo.toml")
        $supervisorVersion = Get-CargoPackageVersion -CargoTomlPath (Join-Path $appsRoot "talos_supervisor\Cargo.toml")
        $sevenZipExe = $pinnedInstallerInputs.SevenZipExecutable
        Write-Step "Using 7-Zip CLI: $sevenZipExe"
        $sevenZipCompressionLevel = Get-SevenZipCompressionLevel -Profile $BuildProfile
        Write-Step "Using 7-Zip compression level: mx=$sevenZipCompressionLevel"

        foreach ($linuxArchName in $linuxArchitectures) {
            $linuxPayloadDir = $linuxPayloadDirs[$linuxArchName]
            $linuxWorkerUpdateZip = $linuxWorkerUpdateZips[$linuxArchName]
            Push-Location $linuxPayloadDir
            try {
                if (Test-OutputUpToDate -OutputPath $linuxWorkerUpdateZip -InputPaths @(
                    (Join-Path $linuxPayloadDir $linuxWorkerBinary)
                )) {
                    Write-Step "Skipping Linux worker $linuxArchName updater package archive (up to date)"
                }
                else {
                    Remove-Item -Force -ErrorAction SilentlyContinue $linuxWorkerUpdateZip
                    Invoke-Checked "Building Linux worker $linuxArchName updater package archive" {
                        & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $linuxWorkerUpdateZip ".\$linuxWorkerBinary"
                    }
                }
            }
            finally {
                Pop-Location
            }

            $linuxSupervisorUpdateZip = $linuxSupervisorUpdateZips[$linuxArchName]
            Push-Location $linuxPayloadDir
            try {
                if (Test-OutputUpToDate -OutputPath $linuxSupervisorUpdateZip -InputPaths @(
                    (Join-Path $linuxPayloadDir $linuxSupervisorBinary)
                )) {
                    Write-Step "Skipping Linux supervisor $linuxArchName updater package archive (up to date)"
                }
                else {
                    Remove-Item -Force -ErrorAction SilentlyContinue $linuxSupervisorUpdateZip
                    Invoke-Checked "Building Linux supervisor $linuxArchName updater package archive" {
                        & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $linuxSupervisorUpdateZip ".\$linuxSupervisorBinary"
                    }
                }
            }
            finally {
                Pop-Location
            }
        }

        Write-Step "Writing signed Linux updater manifests"
        $generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")
        foreach ($linuxArchName in $linuxArchitectures) {
            $linuxWorkerUpdatePayload = @{
                product = "worker"
                platform = "linux"
                arch = $linuxArchName
                channel = "stable"
                version = $agentVersion
                minimumSupportedVersion = $agentVersion
                severity = "normal"
                publishedAtUtc = $generatedAtUtc
                rolloutPercentage = 100
                package = Get-ArtifactMetadata -Path $linuxWorkerUpdateZips[$linuxArchName]
                contents = @($linuxWorkerBinary)
                requiresRestart = $true
                installMode = "silent"
            }
            $linuxWorkerUpdateJson = $linuxWorkerUpdatePayload | ConvertTo-Json -Depth 8
            Write-Utf8NoBomJson -Path $linuxWorkerUpdateManifests[$linuxArchName] -Json $linuxWorkerUpdateJson
            Sign-ManifestFile -ManifestPath $linuxWorkerUpdateManifests[$linuxArchName] -Cert $manifestCert -SignaturePath $linuxWorkerUpdateSignatures[$linuxArchName]

            $linuxSupervisorUpdatePayload = @{
                product = "supervisor"
                platform = "linux"
                arch = $linuxArchName
                channel = "stable"
                version = $supervisorVersion
                minimumSupportedVersion = $supervisorVersion
                severity = "normal"
                publishedAtUtc = $generatedAtUtc
                rolloutPercentage = 100
                package = Get-ArtifactMetadata -Path $linuxSupervisorUpdateZips[$linuxArchName]
                contents = @($linuxSupervisorBinary)
                requiresRestart = $true
                installMode = "silent"
            }
            $linuxSupervisorUpdateJson = $linuxSupervisorUpdatePayload | ConvertTo-Json -Depth 8
            Write-Utf8NoBomJson -Path $linuxSupervisorUpdateManifests[$linuxArchName] -Json $linuxSupervisorUpdateJson
            Sign-ManifestFile -ManifestPath $linuxSupervisorUpdateManifests[$linuxArchName] -Cert $manifestCert -SignaturePath $linuxSupervisorUpdateSignatures[$linuxArchName]
        }

        $manifestPath = Join-Path $artifactProfileDir "manifest.json"
        $manifest = @{
            profile = $BuildProfile
            generatedAtUtc = $generatedAtUtc
            linux = @{}
            updates = @{}
        }
        if (Test-Path -Path $publishedLinuxAgentBinary -PathType Leaf) {
            $manifest.linux.agentBinary = Get-ArtifactMetadata -Path $publishedLinuxAgentBinary
        }
        foreach ($linuxArchName in $linuxArchitectures) {
            $manifestSuffix = Convert-ArchToManifestSuffix -Arch $linuxArchName
            $manifest.updates["worker$manifestSuffix"] = @{
                manifest = Get-ArtifactMetadata -Path $linuxWorkerUpdateManifests[$linuxArchName]
                signature = Get-ArtifactMetadata -Path $linuxWorkerUpdateSignatures[$linuxArchName]
                package = Get-ArtifactMetadata -Path $linuxWorkerUpdateZips[$linuxArchName]
            }
            $manifest.updates["supervisor$manifestSuffix"] = @{
                manifest = Get-ArtifactMetadata -Path $linuxSupervisorUpdateManifests[$linuxArchName]
                signature = Get-ArtifactMetadata -Path $linuxSupervisorUpdateSignatures[$linuxArchName]
                package = Get-ArtifactMetadata -Path $linuxSupervisorUpdateZips[$linuxArchName]
            }
        }
        $manifestJson = $manifest | ConvertTo-Json -Depth 8
        Write-Utf8NoBomJson -Path $manifestPath -Json $manifestJson

        Write-Host ""
        Write-Host "Linux installer artifact build complete."
        if (Test-Path -Path $publishedLinuxAgentBinary -PathType Leaf) {
            Write-Host "Linux installer agent binary (linux-x64):"
            Write-Host $publishedLinuxAgentBinary
        }
        foreach ($linuxArchName in $linuxArchitectures) {
            Write-Host "Linux worker update package ($linuxArchName):"
            Write-Host $linuxWorkerUpdateZips[$linuxArchName]
            Write-Host "Linux worker update manifest ($linuxArchName):"
            Write-Host $linuxWorkerUpdateManifests[$linuxArchName]
            Write-Host "Linux supervisor update package ($linuxArchName):"
            Write-Host $linuxSupervisorUpdateZips[$linuxArchName]
            Write-Host "Linux supervisor update manifest ($linuxArchName):"
            Write-Host $linuxSupervisorUpdateManifests[$linuxArchName]
        }
        Write-Host "Manifest path:"
        Write-Host $manifestPath
        return
    }

    if ($SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning) {
        Write-Step "Signing cargo-built executables (pre-stage; profile folder: $cargoProfileDirName)"
        $signPaths = @()
        if ($buildUpdater) { $signPaths += $x86UpdaterPath }
        if ($buildViewer) { $signPaths += $x64ViewerPath }
        if ($buildViewerUpdater) { $signPaths += $x64ViewerUpdaterPath }
        foreach ($workerArch in $workerArchitectures) {
            if ($workerBuildOutputs.ContainsKey($workerArch)) {
                $signPaths += $workerBuildOutputs[$workerArch].Agent
                $signPaths += $workerBuildOutputs[$workerArch].Helper
                $signPaths += $workerBuildOutputs[$workerArch].Chat
            }
        }
        $signPaths = @($signPaths | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
        Sign-Binaries -Paths $signPaths -Cert $cert -ExpectedThumbprint $authenticodeExpectedThumbprint -TimestampUrl $TimestampServer -ExternalSignerPath $externalAuthenticodeSignerPath
    }
    elseif ($SkipAuthenticodeSigning) {
        Write-Warning "Skipping Authenticode signing of cargo EXEs and final WiX installers (-SkipAuthenticodeSigning)."
    }

    Write-Step "Staging binaries for installer payload"
    if ($buildSupervisorArtifacts) {
        New-Item -ItemType Directory -Force -Path $payloadX64 | Out-Null
        New-Item -ItemType Directory -Force -Path $payloadX86 | Out-Null
    }
    if ($buildWorkerUpdateArtifacts) {
        foreach ($workerArch in $workerArchitectures) {
            New-Item -ItemType Directory -Force -Path $workerPayloadDirs[$workerArch] | Out-Null
        }
    }
    if ($buildLinux) {
        foreach ($linuxArchName in $linuxArchitectures) {
            New-Item -ItemType Directory -Force -Path $linuxPayloadDirs[$linuxArchName] | Out-Null
        }
    }
    if ($buildViewerArtifacts) {
        New-Item -ItemType Directory -Force -Path $viewerPayloadX64 | Out-Null
    }

    $obsoletePayloadFiles = @()
    if ($buildSupervisorArtifacts) {
        $obsoletePayloadFiles += @(
            (Join-Path $payloadX64 "rmm_agent.exe"),
            (Join-Path $payloadX64 "rmm_agent_helper.exe"),
            (Join-Path $payloadX64 "rmm_agent_chat.exe"),
            (Join-Path $payloadX64 "updater.exe"),
            (Join-Path $payloadX86 "rmm_agent.exe"),
            (Join-Path $payloadX86 "rmm_agent_helper.exe"),
            (Join-Path $payloadX86 "rmm_agent_chat.exe"),
            (Join-Path $payloadX86 "updater.exe")
        )
    }
    if ($buildViewerArtifacts) {
        $obsoletePayloadFiles += @(
            (Join-Path $viewerPayloadX64 "rmm_viewer.exe"),
            (Join-Path $viewerPayloadX64 "rmm_viewer_updater.exe")
        )
    }
    if ($obsoletePayloadFiles.Count -gt 0) {
        Remove-Item -Force -ErrorAction SilentlyContinue $obsoletePayloadFiles
    }

    if ($buildSupervisorArtifacts) {
        [void](Copy-ItemIfChanged -SourcePath $x86UpdaterPath -DestinationPath (Join-Path $payloadX64 $supervisorExe))
        [void](Copy-ItemIfChanged -SourcePath $x86UpdaterPath -DestinationPath (Join-Path $payloadX86 $supervisorExe))
    }
    if ($buildWorkerUpdateArtifacts) {
        foreach ($workerArch in $workerArchitectures) {
            if (-not $workerBuildOutputs.ContainsKey($workerArch)) {
                throw "Missing worker build outputs for $workerArch."
            }
            $workerOutput = $workerBuildOutputs[$workerArch]
            $workerPayloadDir = $workerPayloadDirs[$workerArch]
            [void](Copy-ItemIfChanged -SourcePath $workerOutput.Agent -DestinationPath (Join-Path $workerPayloadDir $workerExe))
            [void](Copy-ItemIfChanged -SourcePath $workerOutput.Helper -DestinationPath (Join-Path $workerPayloadDir $workerHelperExe))
            [void](Copy-ItemIfChanged -SourcePath $workerOutput.Chat -DestinationPath (Join-Path $workerPayloadDir $workerChatExe))
        }
    }
    if ($buildLinux) {
        foreach ($linuxArchName in $linuxArchitectures) {
            if (-not $linuxBuildOutputs.ContainsKey($linuxArchName)) {
                throw "Missing Linux build outputs for $linuxArchName."
            }
            $linuxOutput = $linuxBuildOutputs[$linuxArchName]
            $linuxPayloadDir = $linuxPayloadDirs[$linuxArchName]
            [void](Copy-ItemIfChanged -SourcePath $linuxOutput.Worker -DestinationPath (Join-Path $linuxPayloadDir $linuxWorkerBinary))
            [void](Copy-ItemIfChanged -SourcePath $linuxOutput.Supervisor -DestinationPath (Join-Path $linuxPayloadDir $linuxSupervisorBinary))
        }
    }
    if ($buildViewerArtifacts) {
        [void](Copy-ItemIfChanged -SourcePath $x64ViewerPath -DestinationPath (Join-Path $viewerPayloadX64 $viewerExe))
        [void](Copy-ItemIfChanged -SourcePath $x64ViewerUpdaterPath -DestinationPath (Join-Path $viewerPayloadX64 $viewerUpdaterExe))
    }

    $_warnUnsignedPayload = -not $SignAuthenticodeBinaries -and -not $SkipAuthenticodeSigning
    if ($_warnUnsignedPayload) {
        Write-Warning "Cargo EXEs and final WiX installers will not be Authenticode-signed. Pass -SignAuthenticodeBinaries for production, or -SkipAuthenticodeSigning to silence this warning for local builds."
    }

}
finally {
    Pop-Location -ErrorAction SilentlyContinue
    Restore-BuildInstallersOriginalLocation
}

if ($buildSupervisorArtifacts) {
    Ensure-DownloadedFile -Url $webView2RuntimeUrl -DestinationPath $webView2RuntimePath -Label "Microsoft Edge WebView2 Runtime bootstrapper" -RequireMicrosoftAuthenticode $true
    Ensure-DownloadedFile -Url $vcRedistX86Url -DestinationPath $vcRedistX86Path -Label "Visual C++ Redistributable x86 (2015-2022)" -ExpectedSha256 $vcRedistX86Sha256 -RequireMicrosoftAuthenticode $true
    Ensure-DownloadedFile -Url $vcRedistX64Url -DestinationPath $vcRedistX64Path -Label "Visual C++ Redistributable x64 (2015-2022)" -ExpectedSha256 $vcRedistX64Sha256 -RequireMicrosoftAuthenticode $true
}

$bundleExe = Join-Path $installerRoot "bundle\bin\Release\Talos.Agent.Setup.exe"
$msiX64 = Join-Path $installerRoot "msi\bin\Release\Talos.Agent.x64.msi"
$msiX86 = Join-Path $installerRoot "msi\bin\Release\Talos.Agent.x86.msi"
$viewerMsiX64 = Join-Path $installerRoot "msi\bin\Release\Talos.Viewer.x64.msi"

$msiProjectX86 = ".\apps\installer\msi\Talos.Agent.x86.wixproj"
$msiProjectX64 = ".\apps\installer\msi\Talos.Agent.x64.wixproj"
$viewerMsiProjectX64 = ".\apps\installer\msi\Talos.Viewer.x64.wixproj"
$bundleProject = ".\apps\installer\bundle\Talos.Agent.Bundle.wixproj"

$needsMsiX86 = $false
$needsMsiX64 = $false
$needsViewerMsiX64 = $false
if ($buildSupervisorArtifacts) {
    $needsMsiX86 = -not (Test-OutputUpToDate -OutputPath $msiX86 -InputPaths @(
        $installerBuildTargetsPath,
        $thirdPartyAcquisitionPolicyPath,
        $pinnedInstallerInputs.Manifest,
        $agentInstallerCargoTomlPath,
        (Join-Path $installerRoot "msi\Talos.Agent.x86.wixproj"),
        (Join-Path $installerRoot "msi\Agent.x86.wxs"),
        (Join-Path $payloadX86 $supervisorExe)
    ))
    $needsMsiX64 = -not (Test-OutputUpToDate -OutputPath $msiX64 -InputPaths @(
        $installerBuildTargetsPath,
        $thirdPartyAcquisitionPolicyPath,
        $pinnedInstallerInputs.Manifest,
        $agentInstallerCargoTomlPath,
        (Join-Path $installerRoot "msi\Talos.Agent.x64.wixproj"),
        (Join-Path $installerRoot "msi\Agent.x64.wxs"),
        (Join-Path $payloadX64 $supervisorExe)
    ))
}
if ($buildViewerArtifacts) {
    $needsViewerMsiX64 = -not (Test-OutputUpToDate -OutputPath $viewerMsiX64 -InputPaths @(
        $installerBuildTargetsPath,
        $thirdPartyAcquisitionPolicyPath,
        $pinnedInstallerInputs.Manifest,
        $viewerInstallerCargoTomlPath,
        (Join-Path $installerRoot "msi\Talos.Viewer.x64.wixproj"),
        (Join-Path $installerRoot "msi\Viewer.x64.wxs"),
        (Join-Path $installerRoot "msi\viewer-license.rtf"),
        (Join-Path $viewerPayloadX64 $viewerExe),
        (Join-Path $viewerPayloadX64 $viewerUpdaterExe)
    ))
}

Push-Location $repoRoot
try {
    $wixBuildsToRun = @()

    if ($needsMsiX86) {
        Invoke-Checked "Restoring WiX MSI x86" { dotnet restore $msiProjectX86 --configfile $pinnedInstallerInputs.NuGetConfig }
        $wixBuildsToRun += [pscustomobject]@{
            Description = "Building WiX MSI x86"
            ProjectPath = $msiProjectX86
            ProductVersion = $agentInstallerVersion
        }
    }
    elseif ($buildSupervisorArtifacts) {
        Write-Step "Skipping WiX MSI x86 (up to date)"
    }

    if ($needsMsiX64) {
        Invoke-Checked "Restoring WiX MSI x64" { dotnet restore $msiProjectX64 --configfile $pinnedInstallerInputs.NuGetConfig }
        $wixBuildsToRun += [pscustomobject]@{
            Description = "Building WiX MSI x64"
            ProjectPath = $msiProjectX64
            ProductVersion = $agentInstallerVersion
        }
    }
    elseif ($buildSupervisorArtifacts) {
        Write-Step "Skipping WiX MSI x64 (up to date)"
    }

    if ($needsViewerMsiX64) {
        Invoke-Checked "Restoring WiX Viewer MSI x64" { dotnet restore $viewerMsiProjectX64 --configfile $pinnedInstallerInputs.NuGetConfig }
        $wixBuildsToRun += [pscustomobject]@{
            Description = "Building WiX Viewer MSI x64"
            ProjectPath = $viewerMsiProjectX64
            ProductVersion = $viewerInstallerVersion
        }
    }
    elseif ($buildViewerArtifacts) {
        Write-Step "Skipping WiX Viewer MSI x64 (up to date)"
    }

    if ($wixBuildsToRun.Count -gt 0) {
        Write-Step "Building stale WiX MSI projects"
        foreach ($wixBuild in $wixBuildsToRun) {
            Invoke-Checked $wixBuild.Description {
                dotnet build $wixBuild.ProjectPath -c Release --no-restore "-p:ProductVersion=$($wixBuild.ProductVersion)"
            }
        }
    }

    # Burn copies its chained MSI packages into the bundle. Sign the final MSI outputs first so
    # the bundle contains the exact signed packages that are subsequently published and hashed.
    if ($authenticodeSigningEnabled -and ($buildSupervisorArtifacts -or $buildViewerArtifacts)) {
        $wixMsiPaths = @()
        if ($buildSupervisorArtifacts) {
            $wixMsiPaths += @($msiX86, $msiX64)
        }
        if ($buildViewerArtifacts) {
            $wixMsiPaths += $viewerMsiX64
        }
        Write-Step "Signing final WiX MSI outputs before building the Burn bundle"
        Sign-Binaries -Paths $wixMsiPaths -Cert $cert -ExpectedThumbprint $authenticodeExpectedThumbprint -TimestampUrl $TimestampServer -ExternalSignerPath $externalAuthenticodeSignerPath
    }

    if ($buildSupervisorArtifacts) {
        # A signed build always reconstructs Burn after the MSI signing phase. Do not let a stale
        # or externally preserved timestamp make an older bundle appear to contain these MSIs.
        $needsBundle = $authenticodeSigningEnabled -or -not (Test-OutputUpToDate -OutputPath $bundleExe -InputPaths @(
            $installerBuildTargetsPath,
            $thirdPartyAcquisitionPolicyPath,
            $pinnedInstallerInputs.Manifest,
            $agentInstallerCargoTomlPath,
            (Join-Path $installerRoot "bundle\Talos.Agent.Bundle.wixproj"),
            (Join-Path $installerRoot "bundle\Bundle.wxs"),
            $msiX86,
            $msiX64,
            $webView2RuntimePath,
            $vcRedistX86Path,
            $vcRedistX64Path
        ))
        if ($needsBundle) {
            Invoke-Checked "Restoring WiX Burn bundle" { dotnet restore $bundleProject --configfile $pinnedInstallerInputs.NuGetConfig }
            Invoke-Checked "Building WiX Burn bundle" {
                dotnet build $bundleProject -c Release --no-restore --no-incremental "-p:ProductVersion=$agentInstallerVersion"
            }
        }
        else {
            Write-Step "Skipping WiX Burn bundle (up to date)"
        }
    }
}
finally {
    Pop-Location
}

if ($buildSupervisorArtifacts -and -not (Test-Path -Path $bundleExe -PathType Leaf)) {
    throw "Bundle EXE not found after build: $bundleExe"
}
if ($buildSupervisorArtifacts -and -not (Test-Path -Path $msiX64 -PathType Leaf)) {
    throw "x64 MSI not found after build: $msiX64"
}
if ($buildSupervisorArtifacts -and -not (Test-Path -Path $msiX86 -PathType Leaf)) {
    throw "x86 MSI not found after build: $msiX86"
}
if ($buildViewerArtifacts -and -not (Test-Path -Path $viewerMsiX64 -PathType Leaf)) {
    throw "Viewer x64 MSI not found after build: $viewerMsiX64"
}

# The bundle must be signed after Burn has embedded the already-signed MSIs, but before any copy,
# archive, update manifest, or artifact-manifest hash can observe it. Sign-BurnBundle verifies both
# the outer bundle and its embedded engine, making a signed release fail closed.
if ($authenticodeSigningEnabled -and $buildSupervisorArtifacts) {
    Write-Step "Signing final WiX Burn bundle before publication"
    Sign-BurnBundle -BundlePath $bundleExe -RepoRoot $repoRoot -WixToolManifestPath $wixToolManifestPath -NuGetConfigPath $pinnedInstallerInputs.NuGetConfig -Cert $cert -ExpectedThumbprint $authenticodeExpectedThumbprint -TimestampUrl $TimestampServer -ExternalSignerPath $externalAuthenticodeSignerPath
}

Write-Step "Publishing installer artifacts to profile folder"
New-Item -ItemType Directory -Force -Path $artifactProfileDir | Out-Null
if ($buildSupervisorArtifacts) {
    [void](Copy-ItemIfChanged -SourcePath $bundleExe -DestinationPath (Join-Path $artifactProfileDir "Talos.Agent.Setup.exe"))
    [void](Copy-ItemIfChanged -SourcePath $msiX64 -DestinationPath (Join-Path $artifactProfileDir "Talos.Agent.x64.msi"))
    [void](Copy-ItemIfChanged -SourcePath $msiX86 -DestinationPath (Join-Path $artifactProfileDir "Talos.Agent.x86.msi"))
}
if ($buildViewerArtifacts) {
    [void](Copy-ItemIfChanged -SourcePath $viewerMsiX64 -DestinationPath (Join-Path $artifactProfileDir "Talos.Viewer.x64.msi"))
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $artifactProfileDir "Talos.Viewer.Setup.exe")
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $artifactProfileDir "Talos.Viewer.Setup.7z")
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $artifactProfileDir "_viewer-installer")
}

$publishedBundleExe = Join-Path $artifactProfileDir "Talos.Agent.Setup.exe"
$publishedMsiX64 = Join-Path $artifactProfileDir "Talos.Agent.x64.msi"
$publishedMsiX86 = Join-Path $artifactProfileDir "Talos.Agent.x86.msi"
$publishedArchive = Join-Path $artifactProfileDir "Talos.Agent.Setup.7z"
$publishedSfxStub = Join-Path $artifactProfileDir "7zSD.sfx"
$publishedViewerMsiX64 = Join-Path $artifactProfileDir "Talos.Viewer.x64.msi"
$publishedLinuxAgentBinary = Join-Path $artifactProfileDir $linuxInstallerBinaryName
$manifestPath = Join-Path $artifactProfileDir "manifest.json"
$buildProvenancePath = Join-Path $artifactProfileDir "build-provenance.json"
$checksumPath = Join-Path $artifactProfileDir "SHA256SUMS"
$unsignedNoticePath = Join-Path $artifactProfileDir "UNSIGNED-BINARIES.txt"
if ($buildLinux -and $linuxBuildOutputs.ContainsKey("linux-x64")) {
    if (-not $linuxBuildOutputs["linux-x64"].Supervisor) {
        throw "Expected Linux supervisor build output for installer bootstrap artifact."
    }
    [void](Copy-ItemIfChanged -SourcePath $linuxBuildOutputs["linux-x64"].Supervisor -DestinationPath $publishedLinuxAgentBinary)
}
$workerUpdateZips = @{}
$workerUpdateManifests = @{}
$workerUpdateSignatures = @{}
foreach ($workerArch in $workerArchitectures) {
    $workerUpdateZips[$workerArch] = Join-Path $artifactProfileDir "Talos.Worker.$workerArch.Update.zip"
    $workerUpdateManifests[$workerArch] = Join-Path $artifactProfileDir "Talos.Worker.$workerArch.Update.manifest.json"
    $workerUpdateSignatures[$workerArch] = Join-Path $artifactProfileDir "Talos.Worker.$workerArch.Update.manifest.sig"
}
$linuxWorkerUpdateZips = @{}
$linuxWorkerUpdateManifests = @{}
$linuxWorkerUpdateSignatures = @{}
$linuxSupervisorUpdateZips = @{}
$linuxSupervisorUpdateManifests = @{}
$linuxSupervisorUpdateSignatures = @{}
foreach ($linuxArchName in $linuxArchitectures) {
    $linuxWorkerUpdateZips[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Worker.$linuxArchName.Update.zip"
    $linuxWorkerUpdateManifests[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Worker.$linuxArchName.Update.manifest.json"
    $linuxWorkerUpdateSignatures[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Worker.$linuxArchName.Update.manifest.sig"
    $linuxSupervisorUpdateZips[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Supervisor.$linuxArchName.Update.zip"
    $linuxSupervisorUpdateManifests[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Supervisor.$linuxArchName.Update.manifest.json"
    $linuxSupervisorUpdateSignatures[$linuxArchName] = Join-Path $artifactProfileDir "Talos.Supervisor.$linuxArchName.Update.manifest.sig"
}
$agentUpdateZipX64 = $workerUpdateZips["x64-v1"]
$agentUpdateZipX86 = $workerUpdateZips["x86"]
$supervisorUpdateZipX64 = Join-Path $artifactProfileDir "Talos.Supervisor.x64.Update.zip"
$supervisorUpdateZipX86 = Join-Path $artifactProfileDir "Talos.Supervisor.x86.Update.zip"
$viewerUpdateZipX64 = Join-Path $artifactProfileDir "Talos.Viewer.x64.Update.zip"
$agentUpdateManifestX64 = $workerUpdateManifests["x64-v1"]
$agentUpdateManifestX86 = $workerUpdateManifests["x86"]
$supervisorUpdateManifestX64 = Join-Path $artifactProfileDir "Talos.Supervisor.x64.Update.manifest.json"
$supervisorUpdateManifestX86 = Join-Path $artifactProfileDir "Talos.Supervisor.x86.Update.manifest.json"
$viewerUpdateManifestX64 = Join-Path $artifactProfileDir "Talos.Viewer.x64.Update.manifest.json"
$agentUpdateSignatureX64 = $workerUpdateSignatures["x64-v1"]
$agentUpdateSignatureX86 = $workerUpdateSignatures["x86"]
$supervisorUpdateSignatureX64 = Join-Path $artifactProfileDir "Talos.Supervisor.x64.Update.manifest.sig"
$supervisorUpdateSignatureX86 = Join-Path $artifactProfileDir "Talos.Supervisor.x86.Update.manifest.sig"
$viewerUpdateSignatureX64 = Join-Path $artifactProfileDir "Talos.Viewer.x64.Update.manifest.sig"
$agentVersion = Get-CargoPackageVersion -CargoTomlPath (Join-Path $appsRoot "talos_worker\Cargo.toml")
$supervisorVersion = Get-CargoPackageVersion -CargoTomlPath (Join-Path $appsRoot "talos_supervisor\Cargo.toml")
$viewerVersion = Get-CargoPackageVersion -CargoTomlPath (Join-Path $appsRoot "talos_viewer\src-tauri\Cargo.toml")

$sevenZipExe = $pinnedInstallerInputs.SevenZipExecutable
Write-Step "Using 7-Zip CLI: $sevenZipExe"
$sevenZipCompressionLevel = Get-SevenZipCompressionLevel -Profile $BuildProfile
Write-Step "Using 7-Zip compression level: mx=$sevenZipCompressionLevel"

if ($buildSupervisorArtifacts) {
    $sfxStubSourcePath = Get-SfxStubSourcePath -ExplicitPath $SfxStubPath -ArtifactRoot $artifactRoot
    Assert-ExpectedSha256 -Path $sfxStubSourcePath -ExpectedSha256 $pinnedInstallerInputs.SfxSha256 -Label "Pinned 7zSD.sfx"
    Write-Step "Using 7z SFX stub: $sfxStubSourcePath"

    if (Copy-ItemIfChanged -SourcePath $sfxStubSourcePath -DestinationPath $publishedSfxStub) {
        Write-Step "Published 7z SFX stub to artifact profile folder"
    }
    else {
        Write-Step "Skipping 7z SFX stub publish (up to date)"
    }

    Push-Location $artifactProfileDir
    try {
        if (Test-OutputUpToDate -OutputPath $publishedArchive -InputPaths @($publishedBundleExe)) {
            Write-Step "Skipping installer payload archive (up to date)"
        }
        else {
            if (Test-Path -Path $publishedArchive -PathType Leaf) {
                Remove-Item -Force $publishedArchive
            }
            Invoke-Checked "Building installer payload archive (Talos.Agent.Setup.7z)" {
                & $sevenZipExe a -t7z "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y "Talos.Agent.Setup.7z" "Talos.Agent.Setup.exe"
            }
        }
    }
    finally {
        Pop-Location
    }
}

Write-Step "Building updater package archives"
if ($buildWorkerUpdateArtifacts) {
    foreach ($workerArch in $workerArchitectures) {
        $workerPayloadDir = $workerPayloadDirs[$workerArch]
        $workerUpdateZip = $workerUpdateZips[$workerArch]
        Push-Location $workerPayloadDir
        try {
            if (Test-OutputUpToDate -OutputPath $workerUpdateZip -InputPaths @(
                (Join-Path $workerPayloadDir $workerExe),
                (Join-Path $workerPayloadDir $workerHelperExe),
                (Join-Path $workerPayloadDir $workerChatExe)
            )) {
                Write-Step "Skipping worker $workerArch updater package archive (up to date)"
            }
            else {
                if (Test-Path -Path $workerUpdateZip -PathType Leaf) {
                    Remove-Item -Force $workerUpdateZip
                }
                Invoke-Checked "Building worker $workerArch updater package archive" {
                    & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $workerUpdateZip ".\$workerExe" ".\$workerHelperExe" ".\$workerChatExe"
                }
            }
        }
        finally {
            Pop-Location
        }
    }
}

if ($buildLinux) {
    foreach ($linuxArchName in $linuxArchitectures) {
        $linuxPayloadDir = $linuxPayloadDirs[$linuxArchName]
        $linuxWorkerUpdateZip = $linuxWorkerUpdateZips[$linuxArchName]
        Push-Location $linuxPayloadDir
        try {
            if (Test-OutputUpToDate -OutputPath $linuxWorkerUpdateZip -InputPaths @(
                (Join-Path $linuxPayloadDir $linuxWorkerBinary)
            )) {
                Write-Step "Skipping Linux worker $linuxArchName updater package archive (up to date)"
            }
            else {
                if (Test-Path -Path $linuxWorkerUpdateZip -PathType Leaf) {
                    Remove-Item -Force $linuxWorkerUpdateZip
                }
                Invoke-Checked "Building Linux worker $linuxArchName updater package archive" {
                    & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $linuxWorkerUpdateZip ".\$linuxWorkerBinary"
                }
            }
        }
        finally {
            Pop-Location
        }

        $linuxSupervisorUpdateZip = $linuxSupervisorUpdateZips[$linuxArchName]
        Push-Location $linuxPayloadDir
        try {
            if (Test-OutputUpToDate -OutputPath $linuxSupervisorUpdateZip -InputPaths @(
                (Join-Path $linuxPayloadDir $linuxSupervisorBinary)
            )) {
                Write-Step "Skipping Linux supervisor $linuxArchName updater package archive (up to date)"
            }
            else {
                if (Test-Path -Path $linuxSupervisorUpdateZip -PathType Leaf) {
                    Remove-Item -Force $linuxSupervisorUpdateZip
                }
                Invoke-Checked "Building Linux supervisor $linuxArchName updater package archive" {
                    & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $linuxSupervisorUpdateZip ".\$linuxSupervisorBinary"
                }
            }
        }
        finally {
            Pop-Location
        }
    }
}

if ($buildSupervisorArtifacts) {
    Push-Location $payloadX64
    try {
        if (Test-OutputUpToDate -OutputPath $supervisorUpdateZipX64 -InputPaths @(
            (Join-Path $payloadX64 $supervisorExe)
        )) {
            Write-Step "Skipping supervisor x64 updater package archive (up to date)"
        }
        else {
            if (Test-Path -Path $supervisorUpdateZipX64 -PathType Leaf) {
                Remove-Item -Force $supervisorUpdateZipX64
            }
            Invoke-Checked "Building supervisor x64 updater package archive" {
                & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $supervisorUpdateZipX64 ".\$supervisorExe"
            }
        }
    }
    finally {
        Pop-Location
    }

    Push-Location $payloadX86
    try {
        if (Test-OutputUpToDate -OutputPath $supervisorUpdateZipX86 -InputPaths @(
            (Join-Path $payloadX86 $supervisorExe)
        )) {
            Write-Step "Skipping supervisor x86 updater package archive (up to date)"
        }
        else {
            if (Test-Path -Path $supervisorUpdateZipX86 -PathType Leaf) {
                Remove-Item -Force $supervisorUpdateZipX86
            }
            Invoke-Checked "Building supervisor x86 updater package archive" {
                & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $supervisorUpdateZipX86 ".\$supervisorExe"
            }
        }
    }
    finally {
        Pop-Location
    }
}

if ($buildViewerArtifacts) {
    Push-Location $viewerPayloadX64
    try {
        if (Test-OutputUpToDate -OutputPath $viewerUpdateZipX64 -InputPaths @(
            (Join-Path $viewerPayloadX64 "talos_viewer.exe"),
            (Join-Path $viewerPayloadX64 "talos_viewer_updater.exe")
        )) {
            Write-Step "Skipping viewer x64 updater package archive (up to date)"
        }
        else {
            if (Test-Path -Path $viewerUpdateZipX64 -PathType Leaf) {
                Remove-Item -Force $viewerUpdateZipX64
            }
            Invoke-Checked "Building viewer x64 updater package archive" {
                & $sevenZipExe a -tzip "-mx=$sevenZipCompressionLevel" -mmt=on -bd -y $viewerUpdateZipX64 ".\talos_viewer.exe" ".\talos_viewer_updater.exe"
            }
        }
    }
    finally {
        Pop-Location
    }
}

Write-Step "Writing signed updater manifests"
$generatedAtUtc = (Get-Date).ToUniversalTime().ToString("o")

if ($buildWorkerUpdateArtifacts) {
    foreach ($workerArch in $workerArchitectures) {
        $workerUpdatePayload = @{
            product = "worker"
            platform = "windows"
            arch = $workerArch
            channel = "stable"
            version = $agentVersion
            minimumSupportedVersion = $agentVersion
            severity = "normal"
            publishedAtUtc = $generatedAtUtc
            rolloutPercentage = 100
            package = Get-ArtifactMetadata -Path $workerUpdateZips[$workerArch]
            contents = @($workerExe, $workerHelperExe, $workerChatExe)
            requiresRestart = $true
            installMode = "silent"
        }
        $workerUpdateJson = $workerUpdatePayload | ConvertTo-Json -Depth 8
        Write-Utf8NoBomJson -Path $workerUpdateManifests[$workerArch] -Json $workerUpdateJson
        Sign-ManifestFile -ManifestPath $workerUpdateManifests[$workerArch] -Cert $manifestCert -SignaturePath $workerUpdateSignatures[$workerArch]
    }
}
if ($buildLinux) {
    foreach ($linuxArchName in $linuxArchitectures) {
        $linuxWorkerUpdatePayload = @{
            product = "worker"
            platform = "linux"
            arch = $linuxArchName
            channel = "stable"
            version = $agentVersion
            minimumSupportedVersion = $agentVersion
            severity = "normal"
            publishedAtUtc = $generatedAtUtc
            rolloutPercentage = 100
            package = Get-ArtifactMetadata -Path $linuxWorkerUpdateZips[$linuxArchName]
            contents = @($linuxWorkerBinary)
            requiresRestart = $true
            installMode = "silent"
        }
        $linuxWorkerUpdateJson = $linuxWorkerUpdatePayload | ConvertTo-Json -Depth 8
        Write-Utf8NoBomJson -Path $linuxWorkerUpdateManifests[$linuxArchName] -Json $linuxWorkerUpdateJson
        Sign-ManifestFile -ManifestPath $linuxWorkerUpdateManifests[$linuxArchName] -Cert $manifestCert -SignaturePath $linuxWorkerUpdateSignatures[$linuxArchName]

        $linuxSupervisorUpdatePayload = @{
            product = "supervisor"
            platform = "linux"
            arch = $linuxArchName
            channel = "stable"
            version = $supervisorVersion
            minimumSupportedVersion = $supervisorVersion
            severity = "normal"
            publishedAtUtc = $generatedAtUtc
            rolloutPercentage = 100
            package = Get-ArtifactMetadata -Path $linuxSupervisorUpdateZips[$linuxArchName]
            contents = @($linuxSupervisorBinary)
            requiresRestart = $true
            installMode = "silent"
        }
        $linuxSupervisorUpdateJson = $linuxSupervisorUpdatePayload | ConvertTo-Json -Depth 8
        Write-Utf8NoBomJson -Path $linuxSupervisorUpdateManifests[$linuxArchName] -Json $linuxSupervisorUpdateJson
        Sign-ManifestFile -ManifestPath $linuxSupervisorUpdateManifests[$linuxArchName] -Cert $manifestCert -SignaturePath $linuxSupervisorUpdateSignatures[$linuxArchName]
    }
}
if ($buildSupervisorArtifacts) {
    $supervisorUpdatePayloadX64 = @{
        product = "supervisor"
        platform = "windows"
        arch = "x64"
        channel = "stable"
        version = $supervisorVersion
        minimumSupportedVersion = $supervisorVersion
        severity = "normal"
        publishedAtUtc = $generatedAtUtc
        rolloutPercentage = 100
        package = Get-ArtifactMetadata -Path $supervisorUpdateZipX64
        contents = @($supervisorExe)
        requiresRestart = $true
        installMode = "silent"
    }
    $supervisorUpdateJsonX64 = $supervisorUpdatePayloadX64 | ConvertTo-Json -Depth 8
    Write-Utf8NoBomJson -Path $supervisorUpdateManifestX64 -Json $supervisorUpdateJsonX64
    Sign-ManifestFile -ManifestPath $supervisorUpdateManifestX64 -Cert $manifestCert -SignaturePath $supervisorUpdateSignatureX64

    $supervisorUpdatePayloadX86 = @{
        product = "supervisor"
        platform = "windows"
        arch = "x86"
        channel = "stable"
        version = $supervisorVersion
        minimumSupportedVersion = $supervisorVersion
        severity = "normal"
        publishedAtUtc = $generatedAtUtc
        rolloutPercentage = 100
        package = Get-ArtifactMetadata -Path $supervisorUpdateZipX86
        contents = @($supervisorExe)
        requiresRestart = $true
        installMode = "silent"
    }
    $supervisorUpdateJsonX86 = $supervisorUpdatePayloadX86 | ConvertTo-Json -Depth 8
    Write-Utf8NoBomJson -Path $supervisorUpdateManifestX86 -Json $supervisorUpdateJsonX86
    Sign-ManifestFile -ManifestPath $supervisorUpdateManifestX86 -Cert $manifestCert -SignaturePath $supervisorUpdateSignatureX86
}
if ($buildViewerArtifacts) {
    $viewerUpdatePayloadX64 = @{
        product = "viewer"
        platform = "windows"
        arch = "x64"
        channel = "stable"
        version = $viewerVersion
        minimumSupportedVersion = $viewerVersion
        severity = "normal"
        publishedAtUtc = $generatedAtUtc
        rolloutPercentage = 100
        package = Get-ArtifactMetadata -Path $viewerUpdateZipX64
        contents = @("talos_viewer.exe", "talos_viewer_updater.exe")
        requiresRestart = $true
        installMode = "restart"
    }
    $viewerUpdateJsonX64 = $viewerUpdatePayloadX64 | ConvertTo-Json -Depth 8
    Write-Utf8NoBomJson -Path $viewerUpdateManifestX64 -Json $viewerUpdateJsonX64
    Sign-ManifestFile -ManifestPath $viewerUpdateManifestX64 -Cert $manifestCert -SignaturePath $viewerUpdateSignatureX64
}

Write-Step "Writing installer artifact manifest"
$hasWindowsArtifacts = $buildSupervisorArtifacts -or $buildViewerArtifacts
$windowsAuthenticodeStatus = if ($hasWindowsArtifacts -and $authenticodeSigningEnabled) {
    "signed"
}
elseif ($hasWindowsArtifacts) {
    "unsigned"
}
else {
    "not-applicable"
}

if ($windowsAuthenticodeStatus -eq "unsigned") {
    Write-UnsignedArtifactNotice -Path $unsignedNoticePath
}
else {
    Remove-Item -LiteralPath $unsignedNoticePath -Force -ErrorAction SilentlyContinue
}

$buildSourceMetadata = Get-BuildSourceMetadata -RepoRoot $repoRoot
$buildProvenance = [ordered]@{
    schemaVersion = 1
    generatedAtUtc = $generatedAtUtc
    kind = "local-build-metadata-not-cryptographic-attestation"
    source = $buildSourceMetadata
    builder = [ordered]@{
        script = "scripts/build-installers.ps1"
        profile = $BuildProfile
    }
    thirdPartyInputs = [ordered]@{
        policy = Get-ArtifactMetadata -Path $pinnedInstallerInputs.Policy
        acquisitionManifest = Get-ArtifactMetadata -Path $pinnedInstallerInputs.Manifest
        acquisition = Get-Content -LiteralPath $pinnedInstallerInputs.Manifest -Raw | ConvertFrom-Json
    }
    trust = [ordered]@{
        updaterManifestAlgorithm = "RSA-PKCS1-v1_5-SHA256"
        updaterManifestPublicKeySha256 = $manifestPublicKeySha256
        windowsAuthenticodeStatus = $windowsAuthenticodeStatus
        windowsAuthenticodeExpectedThumbprint = if ($authenticodeSigningEnabled) {
            $authenticodeExpectedThumbprint
        }
        else {
            $null
        }
    }
}
$buildProvenanceJson = $buildProvenance | ConvertTo-Json -Depth 8
Write-Utf8NoBomJson -Path $buildProvenancePath -Json $buildProvenanceJson

$manifest = @{
    profile = $BuildProfile
    generatedAtUtc = $generatedAtUtc
    signing = @{
        updaterManifests = @{
            algorithm = "RSA-PKCS1-v1_5-SHA256"
            publicKeySha256 = $manifestPublicKeySha256
        }
        windowsAuthenticode = @{
            status = $windowsAuthenticodeStatus
            expectedCertificateThumbprint = if ($authenticodeSigningEnabled) {
                $authenticodeExpectedThumbprint
            }
            else {
                $null
            }
        }
    }
    integrity = @{
        checksumAlgorithm = "SHA-256"
        checksumFile = "SHA256SUMS"
        provenance = Get-ArtifactMetadata -Path $buildProvenancePath
        unsignedBinaryNotice = if ($windowsAuthenticodeStatus -eq "unsigned") {
            Get-ArtifactMetadata -Path $unsignedNoticePath
        }
        else {
            $null
        }
    }
    updates = @{}
}
if ($buildSupervisorArtifacts) {
    $manifest.payloadExeName = "Talos.Agent.Setup.exe"
    $manifest.wix = @{
        productVersion = $agentInstallerVersion
        bundle = Get-ArtifactMetadata -Path $publishedBundleExe
        msiX64 = Get-ArtifactMetadata -Path $publishedMsiX64
        msiX86 = Get-ArtifactMetadata -Path $publishedMsiX86
    }
    $manifest.sfx = @{
        stub = Get-ArtifactMetadata -Path $publishedSfxStub
        archive = Get-ArtifactMetadata -Path $publishedArchive
    }
    $manifest.updates["supervisorX64"] = @{
        manifest = Get-ArtifactMetadata -Path $supervisorUpdateManifestX64
        signature = Get-ArtifactMetadata -Path $supervisorUpdateSignatureX64
        package = Get-ArtifactMetadata -Path $supervisorUpdateZipX64
    }
    $manifest.updates["supervisorX86"] = @{
        manifest = Get-ArtifactMetadata -Path $supervisorUpdateManifestX86
        signature = Get-ArtifactMetadata -Path $supervisorUpdateSignatureX86
        package = Get-ArtifactMetadata -Path $supervisorUpdateZipX86
    }
}
if ($buildViewerArtifacts) {
    $manifest.viewer = @{
        productVersion = $viewerInstallerVersion
        installer = Get-ArtifactMetadata -Path $publishedViewerMsiX64
        msiX64 = Get-ArtifactMetadata -Path $publishedViewerMsiX64
    }
    $manifest.updates["viewerX64"] = @{
        manifest = Get-ArtifactMetadata -Path $viewerUpdateManifestX64
        signature = Get-ArtifactMetadata -Path $viewerUpdateSignatureX64
        package = Get-ArtifactMetadata -Path $viewerUpdateZipX64
    }
}
if ($buildLinux -and (Test-Path -Path $publishedLinuxAgentBinary -PathType Leaf)) {
    $manifest.linux = @{
        agentBinary = Get-ArtifactMetadata -Path $publishedLinuxAgentBinary
    }
}
if ($buildWorkerUpdateArtifacts) {
    foreach ($workerArch in $workerArchitectures) {
        $manifestSuffix = Convert-ArchToManifestSuffix -Arch $workerArch
        $manifestKey = "worker$manifestSuffix"
        $manifest.updates[$manifestKey] = @{
            manifest = Get-ArtifactMetadata -Path $workerUpdateManifests[$workerArch]
            signature = Get-ArtifactMetadata -Path $workerUpdateSignatures[$workerArch]
            package = Get-ArtifactMetadata -Path $workerUpdateZips[$workerArch]
        }
    }
}
if ($buildLinux) {
    foreach ($linuxArchName in $linuxArchitectures) {
        $manifestSuffix = Convert-ArchToManifestSuffix -Arch $linuxArchName
        $manifest.updates["worker$manifestSuffix"] = @{
            manifest = Get-ArtifactMetadata -Path $linuxWorkerUpdateManifests[$linuxArchName]
            signature = Get-ArtifactMetadata -Path $linuxWorkerUpdateSignatures[$linuxArchName]
            package = Get-ArtifactMetadata -Path $linuxWorkerUpdateZips[$linuxArchName]
        }
        $manifest.updates["supervisor$manifestSuffix"] = @{
            manifest = Get-ArtifactMetadata -Path $linuxSupervisorUpdateManifests[$linuxArchName]
            signature = Get-ArtifactMetadata -Path $linuxSupervisorUpdateSignatures[$linuxArchName]
            package = Get-ArtifactMetadata -Path $linuxSupervisorUpdateZips[$linuxArchName]
        }
    }
}
$manifestJson = $manifest | ConvertTo-Json -Depth 8
Write-Utf8NoBomJson -Path $manifestPath -Json $manifestJson
Write-ArtifactChecksums -ArtifactDirectory $artifactProfileDir -OutputPath $checksumPath

Write-Host ""
Write-Host "Build complete."
if ($buildSupervisorArtifacts) {
    Write-Host "Burn installer path:"
    Write-Host $publishedBundleExe
    Write-Host "Installer payload archive path:"
    Write-Host $publishedArchive
}
if ($buildViewerArtifacts) {
    Write-Host "Viewer installer path:"
    Write-Host $publishedViewerMsiX64
}
if ($buildWorkerUpdateArtifacts) {
    foreach ($workerArch in $workerArchitectures) {
        Write-Host "Worker update package ($workerArch):"
        Write-Host $workerUpdateZips[$workerArch]
        Write-Host "Worker update manifest ($workerArch):"
        Write-Host $workerUpdateManifests[$workerArch]
    }
}
if ($buildLinux) {
    if (Test-Path -Path $publishedLinuxAgentBinary -PathType Leaf) {
        Write-Host "Linux installer agent binary (linux-x64):"
        Write-Host $publishedLinuxAgentBinary
    }
    foreach ($linuxArchName in $linuxArchitectures) {
        Write-Host "Linux worker update package ($linuxArchName):"
        Write-Host $linuxWorkerUpdateZips[$linuxArchName]
        Write-Host "Linux worker update manifest ($linuxArchName):"
        Write-Host $linuxWorkerUpdateManifests[$linuxArchName]
        Write-Host "Linux supervisor update package ($linuxArchName):"
        Write-Host $linuxSupervisorUpdateZips[$linuxArchName]
        Write-Host "Linux supervisor update manifest ($linuxArchName):"
        Write-Host $linuxSupervisorUpdateManifests[$linuxArchName]
    }
}
if ($buildSupervisorArtifacts) {
    Write-Host "Supervisor update package (x64):"
    Write-Host $supervisorUpdateZipX64
    Write-Host "Supervisor update manifest (x64):"
    Write-Host $supervisorUpdateManifestX64
    Write-Host "Supervisor update package (x86):"
    Write-Host $supervisorUpdateZipX86
    Write-Host "Supervisor update manifest (x86):"
    Write-Host $supervisorUpdateManifestX86
    Write-Host "SFX stub path:"
    Write-Host $publishedSfxStub
}
if ($buildViewerArtifacts) {
    Write-Host "Viewer update package (x64):"
    Write-Host $viewerUpdateZipX64
    Write-Host "Viewer update manifest (x64):"
    Write-Host $viewerUpdateManifestX64
}
Write-Host "Manifest path:"
Write-Host $manifestPath
Write-Host "Artifact checksums:"
Write-Host $checksumPath
Write-Host "Build provenance metadata:"
Write-Host $buildProvenancePath
if ($windowsAuthenticodeStatus -eq "unsigned") {
    Write-Warning "Windows release artifacts are intentionally unsigned. Include UNSIGNED-BINARIES.txt with every download and release note."
}
