# Talos Community release notes

> **Unsigned binaries:** Official Talos Community binaries in this release are intentionally not
> Authenticode-signed or Apple-notarized unless a platform entry below explicitly says otherwise.
> Windows may report `Unknown publisher` or show SmartScreen/reputation warnings. Verify downloads
> with the attached `SHA256SUMS`, confirm the release source, and follow your organisation's
> software-approval policy. Do not disable operating-system security controls globally.

## Artifacts and trust

- Source revision: `REPLACE_WITH_FULL_COMMIT_SHA`
- Source snapshot policy manifest: included as `.talos-export-manifest.json` in the source archive
- Linux x86-64: attached `*-linux-x86_64.tar.gz`; run `sudo ./start-talos.sh` after extraction
- Windows x64: attached `*-windows-x86_64-UNSIGNED.zip`; extract and double-click `Start-Talos.cmd`
- Docker with Compose v2 or newer is required; select Linux containers on Windows
- Local browser: `https://talos.localhost:8443`; follow the bundled `GETTING_STARTED.md` certificate guidance
- Verify both the attached outer and extracted inner `SHA256SUMS` before execution
- Talos application images: four immutable digest references in `image-references.json`
- Windows Authenticode: **unsigned**
- macOS signing/notarization: **not published**
- Updater manifest public-key SHA-256: `REPLACE_WITH_REVIEWED_FINGERPRINT`
- Checksums: attached `SHA256SUMS`
- Build provenance/attestations: `REPLACE_WITH_LINKS_OR_EXPLICIT_UNAVAILABLE_STATUS`

## Changes

- Replace with user-visible changes.

## Known limitations

- Local evaluation binds only IPv4 loopback 8080/8443 and uses a generated self-signed certificate;
  approve its trust for both frontend and API. Remote devices require the public deployment guide.
- No default account or password; registration closes after the first account.
- Four public Linux images support amd64/arm64; the downloadable launchers support x86-64 only.
- Linux download baseline: Ubuntu 24.04 x86-64; older glibc and musl/Alpine hosts are unverified.
- The bundle includes PostgreSQL; no separately installed database or Redis is required.
- OS service registration and automatic Docker installation are not provided.

- Only install and explicit update resolve the owner-selected official `traefik:latest`; normal
  starts reuse the resolved digest recorded by `talos-server`.
- The immutable single-use-code Windows bootstrapper flow is not complete unless release evidence
  explicitly demonstrates the ADR-0010 gates.
- Replace with release-specific limitations.

## Verification completed

- [ ] Clean Windows launcher build and unsigned notice; if clients are included, explicit `-SkipAuthenticodeSigning`
- [ ] No-Bun bundle install and healthy status on clean Linux and Windows hosts
- [ ] Upgrade plus deliberately failed-update rollback/recovery evidence attached
- [ ] Bundled PostgreSQL backup and restore into disposable infrastructure
- [ ] If native clients are included: matching manifest accepted; tampered/wrong-key manifests rejected
- [ ] If native clients are included: candidate provenance and artifact manifest match the protected expected release-line key fingerprint
- [ ] If native clients are included: wrong package digest rejected
- [ ] Clean install, upgrade, repair, rollback, and uninstall evidence attached
- [ ] `SHA256SUMS` verified after upload
- [ ] Build provenance/attestation verified
- [ ] Four published image digests match the reviewed multi-architecture OCI archives
- [ ] All four GHCR images are repository-linked and anonymously accessible at the reviewed digest
- [ ] Linux released-bundle smoke gate passed (first account, restart, retained data/secrets, loopback ports)
- [ ] Windows double-click startup, repeated launch and failure console tested on a clean supported host
- [ ] SPDX 2.3 source, launcher and image SBOMs reviewed; native-client SBOM also reviewed for full scope
- [ ] Public ACME issuance/renewal and real WebSocket/relay traffic verified where applicable
- [ ] Secret scan confirms no PFX/private key in source, artifacts, logs, or images
- [ ] Qualified human review of cryptography, updater, installer, and release changes
