#Requires -Version 5.1
#Requires -Modules PKI

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $true)]
    [System.Security.SecureString]$Password,

    [string]$Subject = "CN=Talos Community Manifest Signing",

    [ValidateRange(1, 20)]
    [int]$ValidYears = 10
)

$ErrorActionPreference = "Stop"

if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)) {
    throw "Community manifest signing certificate bootstrap is supported only on Windows."
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    throw "-OutputPath must name a password-protected .pfx file."
}
if ($null -eq $Password -or $Password.Length -lt 16) {
    throw "-Password must contain at least 16 characters. Use a password manager to generate and store it."
}

$fullOutputPath = [System.IO.Path]::GetFullPath($OutputPath)
if ([System.IO.Path]::GetExtension($fullOutputPath) -ine ".pfx") {
    throw "-OutputPath must end in .pfx."
}
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$repositoryPrefix = $repositoryRoot.TrimEnd([char[]]@(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
)) + [System.IO.Path]::DirectorySeparatorChar
if ($fullOutputPath.StartsWith($repositoryPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to create a manifest signing private key inside the Talos repository. Choose a protected path outside '$repositoryRoot'."
}
if (Test-Path -LiteralPath $fullOutputPath) {
    throw "Refusing to overwrite existing manifest signing key '$fullOutputPath'."
}

$outputDirectory = Split-Path -Parent $fullOutputPath
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
}
$temporaryPfxPath = Join-Path $outputDirectory ".talos-manifest-$([guid]::NewGuid().ToString('N')).tmp.pfx"

$certificate = $null
$certificateStorePath = $null
$publicKeyFingerprint = $null
try {
    try {
        $certificate = New-SelfSignedCertificate `
            -Type Custom `
            -Subject $Subject `
            -FriendlyName "Talos Community manifest signing" `
            -CertStoreLocation "Cert:\CurrentUser\My" `
            -KeyAlgorithm RSA `
            -KeyLength 3072 `
            -HashAlgorithm SHA256 `
            -KeyUsage DigitalSignature `
            -KeyUsageProperty Sign `
            -KeyExportPolicy ExportableEncrypted `
            -NotBefore (Get-Date).AddMinutes(-5) `
            -NotAfter (Get-Date).AddYears($ValidYears)

        $certificateStorePath = "Cert:\CurrentUser\My\$($certificate.Thumbprint)"
        # X509Certificate2.GetPublicKey() returns the RSA public-key value encoded inside the
        # certificate. This is the same PKCS#1 DER blob embedded by build-installers.ps1, so the
        # fingerprint printed here can be compared directly with the release-build output.
        $publicKeyBytes = $certificate.GetPublicKey()
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            $publicKeyFingerprint = ([System.BitConverter]::ToString(
                $sha256.ComputeHash($publicKeyBytes)
            ) -replace "-", "").ToLowerInvariant()
        }
        finally {
            $sha256.Dispose()
        }

        Export-PfxCertificate `
            -Cert $certificate `
            -FilePath $temporaryPfxPath `
            -Password $Password `
            -ChainOption EndEntityCertOnly `
            -NoProperties `
            -NoClobber | Out-Null
    }
    finally {
        if ($certificate) {
            $certificate.Dispose()
        }
        if ($certificateStorePath -and (Test-Path -Path $certificateStorePath)) {
            # The PFX is the explicit source of truth. Remove the temporary store copy and its key
            # so the bootstrap does not silently leave a second private-key location behind.
            Remove-Item -Path $certificateStorePath -DeleteKey -Confirm:$false
        }
    }

    Move-Item -LiteralPath $temporaryPfxPath -Destination $fullOutputPath
}
catch {
    Remove-Item -LiteralPath $temporaryPfxPath -Force -ErrorAction SilentlyContinue
    throw
}

Write-Host "Created password-protected manifest signing PFX:"
Write-Host $fullOutputPath
Write-Host "Manifest public key SHA-256: $publicKeyFingerprint"
Write-Host "Back up this PFX and its password in separate protected locations. Do not commit either one."
