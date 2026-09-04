<#
.SYNOPSIS
  Produces and validates the content-addressed provenance record for Talos vcpkg inputs.

.DESCRIPTION
  The vcpkg commit alone is not enough to identify the native libraries used by a release:
  Talos also supplies an in-repository libvpx overlay. These helpers hash every overlay file by
  normalized relative path and content, then bind that digest to the vcpkg commit, libvpx
  version, and target triplets. Both setup and release builds use the same implementation so a
  changed overlay cannot silently reuse stale C:\vcpkg output.
#>

function Get-TalosDirectorySha256 {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)][string]$DirectoryPath
  )

  if (-not (Test-Path -LiteralPath $DirectoryPath -PathType Container)) {
    throw "Directory to fingerprint does not exist: $DirectoryPath"
  }

  $resolvedRoot = (Resolve-Path -LiteralPath $DirectoryPath).Path
  $entries = @(
    Get-ChildItem -LiteralPath $resolvedRoot -File -Recurse | ForEach-Object {
      $relativePath = $_.FullName.Substring($resolvedRoot.Length)
      $relativePath = ($relativePath -replace '^[\\/]+', '') -replace '\\', '/'
      [pscustomobject]@{
        RelativePath = $relativePath
        FullName = $_.FullName
      }
    } | Sort-Object -Property RelativePath
  )

  if ($entries.Count -eq 0) {
    throw "Directory to fingerprint is empty: $resolvedRoot"
  }

  $lines = @(
    foreach ($entry in $entries) {
      $fileHash = (Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
      "$($entry.RelativePath)`t$fileHash"
    }
  )
  $canonicalInventory = [string]::Join("`n", [string[]]$lines) + "`n"
  $bytes = [System.Text.Encoding]::UTF8.GetBytes($canonicalInventory)
  $sha256 = [System.Security.Cryptography.SHA256]::Create()
  try {
    $digest = $sha256.ComputeHash($bytes)
  }
  finally {
    $sha256.Dispose()
  }

  return ([System.BitConverter]::ToString($digest)).Replace("-", "").ToLowerInvariant()
}

function Get-TalosVcpkgProvenanceRecord {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)][string]$VcpkgCommit,
    [Parameter(Mandatory = $true)][string]$OverlayPath,
    [Parameter(Mandatory = $true)][string]$LibvpxVersion,
    [Parameter(Mandatory = $true)][string[]]$Triplets
  )

  if ($VcpkgCommit -notmatch '^[0-9a-fA-F]{40}$') {
    throw "vcpkg commit must be an exact 40-character Git object ID."
  }
  if ([string]::IsNullOrWhiteSpace($LibvpxVersion)) {
    throw "libvpx version must not be empty."
  }
  $normalizedTriplets = @($Triplets | ForEach-Object { $_.Trim() } | Where-Object { $_ } | Sort-Object -Unique)
  if ($normalizedTriplets.Count -eq 0) {
    throw "At least one vcpkg target triplet is required."
  }

  $overlaySha256 = Get-TalosDirectorySha256 -DirectoryPath $OverlayPath
  return @(
    "format=talos-vcpkg-provenance-v1"
    "vcpkg_commit=$($VcpkgCommit.ToLowerInvariant())"
    "overlay_sha256=$overlaySha256"
    "libvpx_version=$LibvpxVersion"
    "triplets=$($normalizedTriplets -join ',')"
  ) -join "`n"
}

function Test-TalosVcpkgProvenanceRecord {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)][string]$MarkerPath,
    [Parameter(Mandatory = $true)][string]$ExpectedRecord
  )

  if (-not (Test-Path -LiteralPath $MarkerPath -PathType Leaf)) {
    return $false
  }
  $actual = ((Get-Content -LiteralPath $MarkerPath -Raw) -replace "`r`n", "`n").Trim()
  $expected = ($ExpectedRecord -replace "`r`n", "`n").Trim()
  return ($actual -ceq $expected)
}
