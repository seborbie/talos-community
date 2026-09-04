# ADR-0008: Explicit Community updater-manifest signing keys

- Status: accepted
- Date: 2026-08-28
- Owners: Talos maintainers

## Context

Talos updater clients verify RSA PKCS#1 v1.5 SHA-256 signatures over update manifests using a
public key embedded at client build time. The release script previously sourced both Windows
Authenticode and manifest-signing certificates only by hard-coded certificate-store thumbprints.
It also resolved the Authenticode certificate for every artifact build before considering
`-SkipAuthenticodeSigning`. A Community contributor could therefore neither make an intentionally
unsigned local installer nor produce verifiable manifests without access to private production
certificates.

Authenticode and updater authorization are separate trust decisions. Skipping Authenticode for a
local build must not make unsigned update manifests acceptable, and a manifest private key must
never be copied into a build context or committed to the repository.

The Community release owner has selected intentionally unsigned official binaries for the initial
release because an owner-controlled commercial publisher certificate and signing service are not
currently funded. This changes disclosure and release evidence, not updater authorization.

## Options considered

### Disable manifest verification for Community builds

This makes first-time setup easy, but creates a separate insecure updater and allows anyone who can
serve or modify update metadata to execute arbitrary replacement binaries. It is rejected.

### Generate a new manifest key automatically on every build

This avoids key setup, but clients from one build cannot verify manifests from the next build. A
lost or silently rotated key strands deployed clients and encourages bypasses. It is rejected.

### Require every contributor to import a certificate into the Windows store

This preserves the existing implementation, but mutates machine state, complicates cleanup and
automation, and provides no clear Community bootstrap or backup contract.

### Accept an explicit password-protected PFX for manifest signing

The build can load the key ephemerally, derive and embed its public half, and continue signing every
manifest. Production can retain its existing store-backed certificate workflow.

## Decision

`scripts/build-installers.ps1` resolves the Authenticode certificate only when
`-SignAuthenticodeBinaries` is active and `-SkipAuthenticodeSigning` is absent. Skipping
Authenticode has no effect on updater-manifest signing.

Full installer/update builds require one manifest-signing identity. Production may continue to use
`-ManifestCertificateThumbprint`. Community and other isolated builds may instead provide
`-ManifestCertificatePath` plus a `SecureString` `-ManifestCertificatePassword`; an explicit PFX
takes precedence over the store thumbprint. An empty manifest thumbprint does not implicitly reuse
the Authenticode thumbprint. The PFX is resolved literally, loaded with .NET's
`EphemeralKeySet`, required to contain a digital-signature-capable 2048-to-8192-bit RSA private key,
and never copied into the workspace. The build exports only a PKCS#1 public-key DER file under the
ignored installer temporary directory, embeds it in the updater clients, and signs all generated
manifests with the matching private key.

`scripts/New-CommunityManifestSigningCertificate.ps1` is the supported Windows bootstrap. It
creates a 3072-bit RSA digital-signature key, exports it to a password-protected PFX at an explicit
non-existing path, refuses overwrite, and removes the temporary Current User certificate and key.
It prints the same PKCS#1 public-key SHA-256 fingerprint printed by the build. The build never
invents or rotates a key implicitly.

The initial official Community release selects `-SkipAuthenticodeSigning` explicitly and labels its
Windows artifacts as unsigned in repository documentation, release notes, the installer download
surface, artifact metadata, and the generated `UNSIGNED-BINARIES.txt`. It publishes `SHA256SUMS` and
local `build-provenance.json` metadata alongside every artifact set; release automation must attach
platform build attestations separately and must not call the local metadata a SLSA attestation.

Fork maintainers may opt into `-SignAuthenticodeBinaries` with an explicit expected certificate
thumbprint. They may use the existing local Windows certificate-store path or provide a PowerShell
adapter through `-ExternalAuthenticodeSignerPath` for an HSM/remote signing service. The adapter
receives artifact paths, the expected public thumbprint, and timestamp URL only. Talos passes no
private key, PFX, password, or provider token, and locally verifies every resulting signature
against the expected thumbprint before publishing or hashing the file.

Linux cross-build containers do not receive the checkout as a writable bind mount. The shell and
PowerShell release paths enumerate tracked plus non-ignored working-tree files into a disposable
source context, explicitly omit `.env`, certificate, and signing-key forms, add only the public-key
DER when required, and mount that source read-only. Cargo output is copied through a separate
writable temporary mount. The development relay similarly mounts its exact TLS certificate and key
rather than the containing `apps/certs` directory. These controls keep an updater private key or PFX
outside network-enabled dependency/build processes and outside a compromised relay container.

A platform-neutral static contract and regression tests guard certificate-resolution control flow,
ephemeral PFX loading, RSA bounds, bootstrap non-overwrite behavior, and signing documentation.
The Windows release gate remains responsible for executing the actual PowerShell, WiX, signing,
installation, and update flow.

## Security and trust-boundary analysis

- **Identity and authorization:** possession of the manifest private key authorizes an update for
  clients containing its public key. The self-signed X.509 wrapper is a key container; no public CA
  or Windows certificate-chain trust is used for manifest authorization.
- **Sensitive data:** the PFX, its password, and any unlocked private-key handle are secrets. The
  password is accepted as a `SecureString`, not logged or written by the build. The PFX stays outside
  the checkout and is loaded ephemerally. Git and Docker context rules exclude PFX files as a
  defense in depth measure. Linux cross-build containers receive a sanitized read-only source tree,
  never the ignored checkout contents; the relay receives two exact TLS files, never the directory.
- **Abuse cases:** theft of the PFX and password allows forged updates. A substituted public key at
  build time produces clients controlled by the substituting signer. Release review must therefore
  verify the intended key fingerprint and protect the build host and key inputs.
- **Failure modes:** a wrong password, missing private key, non-RSA key, disallowed key usage, or
  unsupported key size fails before client compilation. A lost key prevents new updates for already
  deployed clients. There is no unsafe fallback to unsigned manifests.
- **Least privilege:** Community bootstrap uses `CurrentUser\My` only as a temporary provider
  location and deletes both certificate and private key after export. Authenticode store access is
  not requested for unsigned Community builds. External Authenticode adapters own provider
  authentication outside Talos and receive no private signing material from the build.

## Consequences

Positive:

- Community contributors can build a cryptographically coherent updater without production keys;
- `-SkipAuthenticodeSigning` no longer creates an unrelated Authenticode certificate dependency;
- production store-backed behavior and manifest verification remain intact;
- public and private key inputs cannot silently diverge because the public key is derived from the
  same certificate used to sign manifests;
- network-enabled Linux build dependencies and the relay cannot enumerate ignored signing material.

Costs and limitations:

- maintainers must protect and back up a PFX and its password separately;
- a real bootstrap/build/update verification still requires a supported Windows host;
- there is no in-place manifest key rotation protocol or dual-key verification yet;
- unsigned public Windows artifacts receive weaker publisher/reputation signals and may trigger
  `Unknown publisher` or SmartScreen warnings; Authenticode remains the preferred future path once
  owner-controlled signing infrastructure is funded, but it is not misrepresented as an initial
  Community release prerequisite.

## Rollout

1. Add guarded Authenticode resolution and explicit ephemeral PFX loading.
2. Add the Community key bootstrap, documentation, and cross-platform contract tests.
3. Isolate Linux Docker builds behind sanitized read-only source and separate output mounts, and
   restrict the relay to exact TLS file mounts.
4. On Windows, bootstrap a disposable test key and build with `-SkipAuthenticodeSigning`; retain
   the unsigned notice, checksums, provenance, and explicit release-note disclosure.
5. Verify a produced manifest succeeds with the embedded key and fails after manifest or signature
   tampering.
6. For a Community release line, create and escrow the long-lived release key under maintainer
   policy before distributing clients.

## Rollback

Before distributing clients, revert to the certificate-store manifest input if required. After
clients are distributed, do not replace the manifest key by rebuilding with another PFX: those
clients will reject its manifests. Restore the escrowed key or ship a staged, signed client release
that adds the next public key before rotating the signer.
