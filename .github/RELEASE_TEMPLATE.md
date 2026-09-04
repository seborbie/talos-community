# Talos Community release notes

> **Unsigned binaries:** Official Talos Community binaries in this release are intentionally not
> Authenticode-signed or Apple-notarized unless a platform entry below explicitly says otherwise.
> Windows may report `Unknown publisher` or show SmartScreen/reputation warnings. Verify downloads
> with the attached `SHA256SUMS`, confirm the release source, and follow your organisation's
> software-approval policy. Do not disable operating-system security controls globally.

## Artifacts and trust

- Source revision: `REPLACE_WITH_FULL_COMMIT_SHA`
- Source snapshot policy manifest: included as `.talos-export-manifest.json` in the source archive
- No-Bun appliance bundle: attached `.tar.gz` and `.zip`; verify the outer and inner `SHA256SUMS`
- Talos application images: four immutable digest references in `image-references.json`
- Windows Authenticode: **unsigned**
- macOS signing/notarization: **not published**
- Updater manifest public-key SHA-256: `REPLACE_WITH_REVIEWED_FINGERPRINT`
- Checksums: attached `SHA256SUMS`
- Build provenance/attestations: `REPLACE_WITH_LINKS_OR_EXPLICIT_UNAVAILABLE_STATUS`

## Changes

- Replace with user-visible changes.

## Known limitations

- Only install and explicit update resolve the owner-selected official `traefik:latest`; normal
  starts reuse the resolved digest recorded by `talos-server`.
- The immutable single-use-code Windows bootstrapper flow is not complete unless release evidence
  explicitly demonstrates the ADR-0010 gates.
- Replace with release-specific limitations.

## Verification completed

- [ ] Clean Windows build with explicit `-SkipAuthenticodeSigning`
- [ ] No-Bun bundle install and healthy status on clean Linux and Windows hosts
- [ ] Upgrade plus deliberately failed-update rollback/recovery evidence attached
- [ ] Bundled PostgreSQL backup and restore into disposable infrastructure
- [ ] Matching manifest accepted; tampered/wrong-key manifests rejected
- [ ] Candidate provenance and artifact manifest match the protected expected release-line key fingerprint
- [ ] Wrong package digest rejected
- [ ] Clean install, upgrade, repair, rollback, and uninstall evidence attached
- [ ] `SHA256SUMS` verified after upload
- [ ] Build provenance/attestation verified
- [ ] Four published image digests match the reviewed multi-architecture OCI archives
- [ ] SPDX 2.3 source, launcher, native-client, and image SBOM inventory reviewed
- [ ] Public ACME issuance/renewal and real WebSocket/relay traffic verified where applicable
- [ ] Secret scan confirms no PFX/private key in source, artifacts, logs, or images
- [ ] Qualified human review of cryptography, updater, installer, and release changes
