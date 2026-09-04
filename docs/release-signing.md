# Release certificates, signatures, and unsigned binaries

Talos has four independent trust systems. A certificate or key for one does not satisfy another:

| Trust system               | Purpose                                                           | Community release policy                                                        |
| -------------------------- | ----------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| Deployment TLS             | Protects public HTTP and relay connections                        | Operator-owned Traefik/ACME certificates; never used to sign software           |
| Updater manifests          | Authorizes update metadata to clients with a pinned public key    | Required for every update-capable build                                         |
| Windows Authenticode       | Gives Windows a publisher identity for EXE/MSI files              | Initial official Community artifacts are intentionally unsigned                 |
| Apple signing/notarization | Gives macOS an Apple publisher identity and notarization evidence | Separate owner-controlled credentials; no official macOS release is claimed yet |

An updater-manifest signature does **not** remove an operating-system `Unknown publisher` warning.
Likewise, HTTPS authenticates the deployment endpoint, not a downloaded executable.

## Initial Community release: unsigned Windows binaries

The initial Community release path must select unsigned Windows output explicitly:

```powershell
$manifestPfx = "D:\Protected\Talos\community-manifest-signing.pfx"
$manifestPassword = Read-Host "Manifest PFX password" -AsSecureString

.\scripts\build-installers.ps1 `
  -BuildProfile release `
  -SkipAuthenticodeSigning `
  -ManifestCertificatePath $manifestPfx `
  -ManifestCertificatePassword $manifestPassword
```

A release build fails if neither or both of `-SkipAuthenticodeSigning` and
`-SignAuthenticodeBinaries` are selected. Skipping Authenticode does not disable manifest signing.
The build emits:

- `UNSIGNED-BINARIES.txt`, which must accompany every unsigned Windows download;
- `SHA256SUMS`, covering every file in the artifact directory except the checksum file itself;
- `build-provenance.json`, recording the source revision, tracked-tree state, builder entry point,
  profile, manifest-key fingerprint, and Authenticode state; and
- `manifest.json`, with per-artifact hashes and explicit signing/integrity metadata.

`build-provenance.json` is transparent local build metadata, not a cryptographic attestation and
not a SLSA claim. The release workflow must additionally attach platform-generated build
provenance/attestations and publish `SHA256SUMS` beside the artifacts. Verify a download in
PowerShell with:

```powershell
(Get-FileHash -Algorithm SHA256 -LiteralPath .\Talos.Viewer.x64.msi).Hash.ToLowerInvariant()
```

Compare the entire 64-character value with the matching `SHA256SUMS` line obtained from the same
release page. A checksum detects a mismatch; it does not establish who produced the release.

Windows may show SmartScreen, reputation, antivirus, application-control, or `Unknown publisher`
warnings for unsigned files. Do not tell users to disable these controls globally. They should
verify the checksum and source, follow their organisation's approval policy, build from reviewed
source, or use binaries signed by a publisher their organisation trusts.

## Updater-manifest release-line key

Possession of the manifest private key authorizes updates for every deployed client that embeds its
public half. Treat the release-line PFX and password as high-impact release secrets even though the
X.509 certificate is self-signed.

Create one key per release line on a clean supported Windows host:

```powershell
$manifestPfx = "D:\Protected\Talos\community-manifest-signing.pfx"
$manifestPassword = Read-Host "New 16+ character manifest PFX password" -AsSecureString

.\scripts\New-CommunityManifestSigningCertificate.ps1 `
  -OutputPath $manifestPfx `
  -Password $manifestPassword
```

The bootstrap refuses to overwrite a file, creates a 3072-bit RSA digital-signature key, exports a
password-protected PFX, removes its temporary `CurrentUser\My` certificate and private key, and
prints the embedded public key's SHA-256 fingerprint. The release build loads that PFX with
`EphemeralKeySet`, derives the public key again, and prints the same fingerprint.

For each release line:

1. Record the expected public-key fingerprint in protected release records. Configure the exact
   lowercase value as `TALOS_EXPECTED_MANIFEST_KEY_SHA256` in the protected
   `community-manifest-signing` GitHub environment and have a second maintainer compare it with the
   bootstrap output. The candidate workflow fails before artifact handoff when the variable is
   missing, malformed, or differs from `build-provenance.json` or `manifest.json`.
2. Store the encrypted PFX and its password in separate protected systems. Keep at least one tested
   offline recovery copy of each, with access logging and a documented custodian.
3. Grant access only to maintainers authorized to publish endpoint updates. Do not place the PFX,
   password, or an exported PEM key in Git, a container context/image, API storage, logs, or a
   general-purpose CI secret shared with pull-request jobs.
4. Test recovery by loading a backup on an isolated Windows host and comparing the derived public
   fingerprint. Do not test by rotating production clients to a disposable key.
5. Retain signed manifests, packages, checksums, provenance, fingerprint review, and release
   approvals together as release evidence.

If the PFX or password is lost, already-deployed clients cannot accept a replacement key. Restore
the protected backup; do not add an unsigned-verification bypass. If the key is suspected stolen,
stop publishing, remove update endpoints if necessary, investigate, and design a staged client
migration. Talos does not yet implement dual-key manifest verification or an in-place rotation
protocol. A safe future rotation requires a release signed by the old key that distributes code
trusting the new key before the old signer is retired.

## Optional Authenticode for fork maintainers

Fork maintainers may sign with their own publisher identity. This is opt-in and does not change the
official Community unsigned policy.

### Windows certificate-store path

Use a code-signing certificate issued by a publisher CA or an internal PKI trusted under the target
organisation's policy. A self-signed Authenticode certificate does not give public Windows clients
a trusted publisher identity. Install the certificate and its accessible private key into
`LocalMachine\My` or `CurrentUser\My`, then pass its explicit 40-character certificate thumbprint.
For example, a fork maintainer can import a protected PFX from outside the checkout into the current
user store without making the imported key exportable:

```powershell
$authenticodePassword = Read-Host "Authenticode PFX password" -AsSecureString
$authenticodeCertificate = Import-PfxCertificate `
  -FilePath "D:\Protected\Fork\publisher-code-signing.pfx" `
  -CertStoreLocation Cert:\CurrentUser\My `
  -Password $authenticodePassword
$authenticodeThumbprint = $authenticodeCertificate.Thumbprint
```

Then run the signed build:

```powershell
.\scripts\build-installers.ps1 `
  -BuildProfile release `
  -SignAuthenticodeBinaries `
  -CertificateThumbprint $authenticodeThumbprint `
  -ManifestCertificatePath $manifestPfx `
  -ManifestCertificatePassword $manifestPassword
```

The script uses SHA-256 file digests and RFC 3161 timestamping. It signs cargo-built executables,
then Agent x86/x64 and Viewer MSI outputs before Burn embeds the Agent MSIs. Burn requires two
signatures: the detached engine used for repair/uninstall and the final compressed bundle. Every
output is re-read with `Get-AuthenticodeSignature` and must have a valid signature from the exact
selected thumbprint before publication or hashing.

### HSM or external signing-service adapter

Use `-ExternalAuthenticodeSignerPath` for an HSM or remote signing service. It must name a local
PowerShell `.ps1` adapter with this exact parameter contract:

```powershell
param(
  [Parameter(Mandatory = $true)] [string[]]$FilePath,
  [Parameter(Mandatory = $true)] [string]$ExpectedCertificateThumbprint,
  [Parameter(Mandatory = $true)] [string]$TimestampServer
)
```

The adapter authenticates to its provider, signs every `FilePath`, and throws on any failure. Talos
passes no PFX, password, token, or private key to the adapter. After it returns, the build verifies
each file locally against `ExpectedCertificateThumbprint`; a provider success response alone is not
accepted. The same adapter is used for the detached Burn engine and outer bundle.

Select it with the public certificate thumbprint whose signature the service will return:

```powershell
.\scripts\build-installers.ps1 `
  -BuildProfile release `
  -SignAuthenticodeBinaries `
  -CertificateThumbprint "0123456789ABCDEF0123456789ABCDEF01234567" `
  -ExternalAuthenticodeSignerPath "D:\Protected\Fork\Invoke-RemoteSigning.ps1" `
  -ManifestCertificatePath $manifestPfx `
  -ManifestCertificatePassword $manifestPassword
```

Provider authentication should use the narrowest available workload identity or hardware-backed
policy, require release-environment approval, and exclude untrusted pull requests. Never lend or share another organisation's signing identity, and never put an Authenticode private key in the
Talos API tier. If sponsorship becomes available, funding owner-controlled certificates, HSMs,
signing-service fees, and qualified release review is welcome; contact the destination published by
the project maintainers.

## macOS is separate

Windows Authenticode inputs cannot sign or notarize macOS software. The current macOS scripts can
use a keychain identity and sign updater manifests, but code signing alone is not a publishable
Apple release gate. An authorized owner must use an owner-controlled Developer ID, secure
timestamp, notarize the final package, staple the ticket, and verify it with Gatekeeper. Until that
platform gate is completed, release notes and download pages must not imply Apple notarization or
publish an unreviewed macOS artifact as an official Talos build.

## Immutable bootstrapper boundary

The API's legacy request-time SFX route changes executable bytes after build and therefore cannot
retain an offline publisher signature. It remains disabled by default and is not a Community
publication path. The intended replacement is one immutable release artifact—unsigned initially or
optionally Authenticode-signed by a fork—that redeems a high-entropy, short-lived, single-use code
for scoped enrollment material. That redemption protocol and its clean-Windows replay, expiry,
revocation, upgrade, repair, rollback, and uninstall gates are not implemented yet; do not describe
the Windows self-service installer as complete until they are.

See [ADR-0008](architecture/decisions/0008-community-manifest-signing-keys.md) and
[ADR-0010](architecture/decisions/0010-disable-runtime-sfx-assembly.md) for the governing trust
decisions.
