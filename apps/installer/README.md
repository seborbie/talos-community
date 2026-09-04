# Talos WiX installers

This folder contains a starter WiX layout for:

- MSI payloads for `x86` and `x64` agent installs
- A Burn bundle (`setup.exe`) that chooses which MSI to run based on OS bitness

## Structure

- `msi/Agent.x86.wxs`: x86 MSI authoring (files + service install)
- `msi/Agent.x64.wxs`: x64 MSI authoring (files + service install)
- `msi/Talos.Agent.x86.wixproj`: Builds x86 MSI
- `msi/Talos.Agent.x64.wixproj`: Builds x64 MSI
- `bundle/Bundle.wxs`: Burn bootstrapper chain
- `bundle/Talos.Agent.Bundle.wixproj`: Builds bootstrapper EXE
- `msi/Viewer.x64.wxs`: x64 Viewer MSI authoring
- `msi/Talos.Viewer.x64.wixproj`: Builds the Viewer MSI
- `payload/`: Staged supervisor and Viewer binaries populated by the build script

## Publication license

First-party Talos Community Edition source is licensed `AGPL-3.0-only`. The Burn bundles link to the
canonical GNU AGPL version 3 text; the Viewer MSI displays the matching notice; and Agent/Viewer
MSIs install the root licence and preliminary third-party notice beside the application binaries.
These surfaces grant no additional proprietary agreement.

This does not clear an installer for publication. Every release archive and updater payload must
also contain the licence, complete notices, and a verified corresponding-source location. The
public repository URL, complete generated dependency notices/SBOM, and media/owner rights
decisions remain blockers in
[`docs/open-source-readiness.md`](../../docs/open-source-readiness.md) and
[`docs/licensing-and-provenance.md`](../../docs/licensing-and-provenance.md).

## Prereqs

- .NET SDK available (`dotnet --version`); the build acquires the exact WiX 6.0.0 CLI, SDK, and
  extensions into an ignored local-only NuGet feed before restoring
- The repository's pinned Bun and Rust toolchains
- NASM **3.01** on `PATH`; release builds reject other versions and never install a moving Winget
  package implicitly
- An archive extractor for the acquisition bootstrap. The script prefers `tar`; pass
  `-SevenZipPath` only when a local 7-Zip executable must bootstrap the download. Archive creation
  then uses the acquired, SHA-256-pinned 7-Zip Extra 26.00 `7za.exe`.

The official WiX packages require acceptance of their Open Source Maintenance Fee Agreement,
including maintenance-fee conditions for certain revenue-generating use. Review and satisfy the
retained [`OSMFEULA.txt`](third-party/wix-6.0.0/OSMFEULA.txt) before building. The SFX module is
reconstructed from the exact official LZMA SDK 26.00 archive and its public-domain SDK notice is
retained at [`third-party/7zip-26.00/LZMA-SDK-NOTICE.txt`](third-party/7zip-26.00/LZMA-SDK-NOTICE.txt).
No WiX DLL, extracted UI tree, `7za.exe`, or SFX binary is required in the public checkout.

Run `scripts/Setup-DevEnviroment.ps1` before a Windows release build. Its vcpkg marker binds the
pinned vcpkg commit to the SHA-256 inventory of `scripts/vcpkg-overlays/libvpx`, libvpx version, and
x86/x64 triplets. A release build recomputes that record and refuses stale native libraries after an
overlay change.

## Build order

Installer builds are orchestrated by:

```powershell
# Initial Community policy: unsigned Windows binaries, signed updater manifests.
$manifestPassword = Read-Host "Manifest PFX password" -AsSecureString
.\scripts\build-installers.ps1 `
  -BuildProfile release `
  -SkipAuthenticodeSigning `
  -ManifestCertificatePath "D:\Protected\Talos\community-manifest-signing.pfx" `
  -ManifestCertificatePassword $manifestPassword
```

## Signing boundaries

Installer builds use two independent signatures:

- **Authenticode** identifies Windows executables and installer packages to Windows. It is an
  opt-in fork/maintainer path enabled with `-SignAuthenticodeBinaries`. It uses an explicit
  `-CertificateThumbprint` plus either the Windows certificate store or an
  `-ExternalAuthenticodeSignerPath` adapter.
- **Updater manifest signing** authorizes update metadata and packages to Talos clients. It always
  runs for an installer-artifact build. The build embeds the manifest certificate's RSA public key
  in each updater client, and signs every generated update manifest with the matching private key.

`-BuildProfile release` requires exactly one explicit Authenticode choice:
`-SignAuthenticodeBinaries` for a signed release, or `-SkipAuthenticodeSigning` for an intentionally
unsigned local/Community release. Passing both or neither fails before the build starts. On
non-release builds, `-SkipAuthenticodeSigning` overrides `-SignAuthenticodeBinaries` and does not
load the Authenticode certificate. It does not disable manifest signing or manifest verification.
Binary-only scoped builds do not publish update manifests, so they do not need a manifest key.
Initial official Community Windows artifacts use `-SkipAuthenticodeSigning`. They are not presented
as publisher-signed even though their updater manifests remain cryptographically authorized.

For an Authenticode-enabled installer build, the order is part of the security boundary. Cargo EXEs
are signed before staging. The final Agent x86/x64 and Viewer MSI outputs are then signed and
verified before Burn embeds the Agent MSIs. After Burn constructs the bootstrapper, its
completed Burn bundle is signed and verified before publication, archive creation, or
artifact-manifest hashing. A signed release fails closed if signtool fails, a final
output is missing, or any resulting signature is not valid and produced with the selected
certificate.
Do not sign or otherwise mutate these files after the artifact manifest has recorded their hashes.

Burn requires two Authenticode signatures: one on the engine cached for repair/uninstall and one on
the complete compressed bundle. The script restores WiX CLI 6.0.0 from
`.config/dotnet-tools.json`, detaches and signs the engine, reattaches it, signs the whole bundle,
and then detaches the final engine again to verify the embedded signature. This follows WiX's
official [bundle-signing sequence](https://docs.firegiant.com/wix/tools/signing/#signing-bundles);
signing only the outer EXE is not sufficient.

An HSM or remote signing service can be integrated without giving Talos a private key. Pass a local
PowerShell `.ps1` adapter with `-ExternalAuthenticodeSignerPath`; it receives `FilePath`,
`ExpectedCertificateThumbprint`, and `TimestampServer` parameters. The adapter owns provider
authentication and must throw on failure. Talos then verifies each returned file locally against
the exact expected thumbprint. See the
[release signing guide](../../docs/release-signing.md#hsm-or-external-signing-service-adapter) for
the complete contract and least-privilege requirements.

Production builds retain certificate-store selection through `-ManifestCertificateThumbprint`.
This input is always explicit; an empty manifest thumbprint does not fall back to
`-CertificateThumbprint`. Use a separate, access-controlled manifest key from the Authenticode
identity. If **signtool** fails
with “No certificates were found” even though its certificate appears in PowerShell, fix the
private-key/store access or use `-SkipAuthenticodeSigning` only for an intentionally unsigned local
build.

### Community manifest key bootstrap

A Community maintainer can keep the manifest key in a password-protected PFX outside the repository
instead of importing it into a machine store. On Windows PowerShell, create the key once:

```powershell
$manifestPfx = Join-Path $env:LOCALAPPDATA "Talos\signing\community-manifest-signing.pfx"
$manifestPassword = Read-Host "New manifest PFX password (16+ characters)" -AsSecureString
.\scripts\New-CommunityManifestSigningCertificate.ps1 `
  -OutputPath $manifestPfx `
  -Password $manifestPassword
```

The bootstrap creates a 3072-bit RSA key restricted to digital signatures, requires a password of
at least 16 characters, exports the PFX through a temporary file, refuses to overwrite an existing
key, prints its public-key SHA-256 fingerprint, and removes its temporary certificate-store copy.
Microsoft documents the underlying
[`New-SelfSignedCertificate`](https://learn.microsoft.com/en-us/powershell/module/pki/new-selfsignedcertificate),
[`Export-PfxCertificate`](https://learn.microsoft.com/en-us/powershell/module/pki/export-pfxcertificate),
and certificate-provider
[`Remove-Item -DeleteKey`](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.security/about/about_certificate_provider)
operations. This certificate is only a container for the custom updater RSA key; it is not used as
a publicly trusted Authenticode identity.

Use the same `SecureString` and explicit path for the build:

```powershell
.\scripts\build-installers.ps1 `
  -BuildProfile dev `
  -SkipAuthenticodeSigning `
  -ManifestCertificatePath $manifestPfx `
  -ManifestCertificatePassword $manifestPassword
```

The PFX is loaded ephemerally, and only its public key is copied into the ignored
`apps/installer/tmp/` build directory. The build prints the public key's SHA-256 fingerprint for
release review. PFX files are ignored by Git and Docker, but maintainers should still store the key
outside the checkout, restrict access, and keep encrypted backups of the PFX and its password in
separate protected locations.

The embedded key is a pinned public key, not a CA trust chain. Every client built with it rejects a
manifest signed by any other key. Losing the PFX prevents future updates to those clients; replacing
it without a staged key-rotation release also strands them. Reuse the same protected PFX for the
lifetime of a Community release line and treat rotation as a client migration, not a build retry.

The script optionally signs staged executables and final WiX outputs in the order above, creates
`Talos.Agent.Setup.7z`, copies the verified acquired `7zSD.sfx`, and publishes all artifacts to
`apps/installer/artifacts/<profile>/`.
Every artifact build also writes `SHA256SUMS`, `build-provenance.json`, and integrity/signing fields
in `manifest.json`. Build provenance binds the third-party acquisition policy and generated
acquisition manifest, including exact archive/package/member hashes. An unsigned Windows build
additionally writes `UNSIGNED-BINARIES.txt`; release
notes and download surfaces must retain that notice. The provenance file is transparent local build
metadata, not a cryptographic attestation or SLSA claim, so the release platform must attach its own
provenance evidence as well.
It also downloads Microsoft VC++ Redistributables (`vc_redist.x86.exe` and `vc_redist.x64.exe`) and the Microsoft Edge WebView2 Runtime bootstrapper (`MicrosoftEdgeWebView2RuntimeInstallerX64.exe`) into `apps/installer/prereqs/` if missing. The VC++ inputs use immutable Microsoft Download URLs and pinned SHA-256 values. Every newly downloaded or cached prerequisite must also have a currently valid Authenticode signature whose signer organization is Microsoft Corporation.

WebView2's evergreen bootstrapper URL intentionally rotates and has no repository-pinned digest.
Its narrow, time-bounded exception is `DR-010` in the dependency risk register; the mandatory
Microsoft Authenticode check runs on every build, including when the bootstrapper is cached.

Linux Docker builders obtain the x86-64 `rustup-init` 1.28.2 executable from Rust's versioned
archive and verify its pinned SHA-256 before execution. They never pipe a downloaded script into a
shell. Cross-builds enumerate tracked and non-ignored working-tree files into a disposable source
context, mount that source read-only, and mount a separate output directory. Ignored `.env`, relay
certificate, and signing-key files are not visible to Cargo or dependency build scripts; only the
manifest public-key DER is copied in explicitly. The macOS Viewer build similarly uses the exact
`@tauri-apps/cli` 2.10.1 dependency from the frozen Bun workspace and passes `--locked` to Cargo.

The macOS Agent and Viewer packagers do not currently perform Apple notarization or ticket
stapling. Code signing alone is not a publishable macOS release gate. An authorized maintainer must
notarize, staple, and Gatekeeper-verify each final package using owner-controlled Apple credentials
before publication.

## Product-version contract

Cargo package versions are the only release-version source passed to WiX:

- Agent x86/x64 MSIs and the Agent Burn bundle use `talos_worker/Cargo.toml`. The supervisor is a
  bootstrap/update implementation component; its independently versioned crate does not control
  the overall Agent installer's upgrade ordering.
- The Viewer MSI uses `talos_viewer/src-tauri/Cargo.toml`. The canonical Viewer Burn project uses
  the same value if it is built manually, although the release script currently publishes the MSI.

The WiX authoring contains `Version="$(var.ProductVersion)"`; it does not contain a copied version.
`build-installers.ps1` validates the Cargo value as a three-component Windows Installer version,
passes it as an MSBuild property, and includes the Cargo manifest in the stale-output inputs. A
direct WiX build must pass `-p:ProductVersion=<major.minor.build>` and fails if the property is
missing. `bun --cwd apps run installer:versions:check` mechanically verifies the mappings, active
`.wixproj` compile inputs, WiX variable use, and release-script wiring.

The validation follows Microsoft's
[ProductVersion limits](https://learn.microsoft.com/en-us/windows/win32/msi/productversion): major
and minor are at most 255 and build is at most 65,535. A fourth field is forbidden because Windows
Installer ignores it, which would make distinct Cargo releases compare as the same MSI version.

Before publishing, increment the applicable Cargo package version above the highest version already
released under that product's stable `UpgradeCode`. `<MajorUpgrade>` rejects a lower version as a
downgrade; rollback therefore means publishing a new, higher package version containing reverted
code, not rebuilding an older version. The first release using this contract moves the checked-in
Agent and Viewer versions forward from their previously hard-coded values, so existing packages
remain upgradeable.

Viewer downloads can be served by the API from either discovered artifacts under `apps/installer/artifacts/<profile>/` or explicit env vars:

- `RMM_VIEWER_INSTALLER_PATH`
- `RMM_VIEWER_INSTALLER_FILENAME`
- `RMM_VIEWER_INSTALLER_MANIFEST_PATH`

The viewer is distributed as `Talos.Viewer.x64.msi`. Its MSI uses `WixUI_Advanced`, defaults to a current-user install, and offers an all-users option in advanced setup.

## Runtime properties

The bundle passes these MSI properties:

- `ENROLLMENT_TOKEN`
- `RMM_SERVER_URL`

Example install:

```powershell
.\apps\installer\bundle\bin\Release\Talos.Agent.Setup.exe EnrollmentToken="token_here" RmmServerUrl="https://rmm.example.com"
```

## Scoped installer assembly

The legacy API can build scoped downloads at request time by concatenating:

1. `7zSD.sfx` (stub)
2. UTF-8 SFX config with scoped `EnrollmentToken` + `RmmServerUrl` command args
3. `Talos.Agent.Setup.7z` (prebuilt payload archive containing Burn EXE)

Changing an executable after Authenticode signing invalidates that signature. The API must never
hold an online code-signing key, so this legacy path is disabled unless a private-development
operator explicitly sets `RMM_ENABLE_UNSIGNED_SCOPED_INSTALLERS=true`. It is not a publication
path and the Community UI does not advertise it.

The public-release target is one immutable Burn bootstrapper produced by the reviewed release
pipeline. It is intentionally Authenticode-unsigned for the initial Community release and may be
signed by a fork or future owner-controlled signing service without changing the enrollment design.
Enrollment material must be supplied separately and should ultimately be redeemed through a
short-lived, single-use bootstrap code. This preserves the exact reviewed binary and keeps signing
authority out of the API tier. ADR-0010 records the boundary and migration.

During install, Burn checks for WebView2 and VC++ 2015-2022 runtime and runs the matching bootstrapper/redistributable silently when required.

## Notes

- This is intentionally a starter scaffold; CPU feature-based selection is not wired yet.
- Complete short-lived bootstrap-code redemption before treating the Windows flow as a public
  self-service installer.
- Initial Community releases use `-BuildProfile release -SkipAuthenticodeSigning`, retain
  `UNSIGNED-BINARIES.txt`, checksums, provenance, and manifest-verification evidence, and state the
  unsigned status prominently. Fork maintainers choosing `-SignAuthenticodeBinaries` must retain
  successful signature-verification output with their release evidence.
