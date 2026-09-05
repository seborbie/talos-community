# Open-source publication readiness

- Source status: **owner-approved Community source alpha (2026-09-04)**
- Packaged release status: **licensed, but not yet cleared for publication**
- Owner: Talos maintainers
- Last reviewed: 2026-08-28

This checklist separates repository engineering work from decisions and platform verification that
cannot be inferred or performed safely by an automated contributor. A checked engineering gate is
evidence only for that gate; it is not a general security, legal, or compliance claim.

## Implemented repository controls

- First-party source licence selected as `AGPL-3.0-only`, with the canonical root `LICENSE` text and
  matching first-party Bun/Cargo package metadata.
- Installer licence links/notices agree with that SPDX selection, and current WiX MSI definitions
  install the root licence and preliminary third-party notice.
- Deterministic `bun run license:check` discovery for the installed frozen Bun graph and locked
  Cargo metadata, with exact reviewed-expression drift control and vendored licence-file hashes.
- Preliminary first-party/generated/vendor/binary/media inventory in
  [`licensing-and-provenance.md`](licensing-and-provenance.md) and
  [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).
- Bun-native isolated workspace with one first-party lockfile, pinned Bun/Rust toolchains, quality
  policy, advisory checks, generated cross-language contracts, and architecture/state records.
- Explicit Community updater-manifest PFX bootstrap and fingerprint review, fail-closed manifest
  verification, wrong-key/tamper/digest regressions, and a tracked-file signing-secret policy.
- Initial unsigned-Windows policy wired into the default build command, repository/download/release
  notices, `UNSIGNED-BINARIES.txt`, `SHA256SUMS`, and transparent build metadata. These repository
  controls do not replace the unchecked clean-Windows or release-attestation gates below.
- A development Community Compose selector with migration bootstrap, relay-certificate preflight,
  per-service environment allowlists, and opt-in update endpoints. This is not yet evidence that
  the planned appliance-like production deployment or clean-volume install is complete.
- Contributor, conduct, support, security, and automated-agent policies, with repository-relative
  private security/conduct intake that does not depend on a guessed GitHub owner.

## Resolved owner decisions

- [x] Product name: Talos Community Edition.
- [x] First-party source licence: GNU Affero General Public License version 3 only
      (`AGPL-3.0-only`).
- [x] External contribution terms: inbound-equals-outbound `AGPL-3.0-only`, without a separate
      contributor licence agreement for ordinary contributions.
- [x] Publication history: create a new public Community repository from a reviewed, squashed
      snapshot; retain the existing development history privately.
- [x] Initial official binaries may be unsigned when clearly disclosed; updater-manifest signing
      remains a separate security requirement.

## Publication blockers requiring owner or legal input

- [x] The owner supplied the exact legal copyright-holder name: Sebastian Orbe (2026-09-04).
- [x] Select the public repository and corresponding-source URL:
      https://github.com/seborbie/talos-community. Binary/release surfaces still require validation
      against their actual release payloads.
- [x] The owner completed the final README review and authorized source publication (2026-09-04).
- [ ] Confirm trademark ownership and the permitted Talos name/logo policy for forks.
- [x] Enable GitHub private vulnerability reporting before uploading source; GitHub requires a public repository.
- [ ] Test security and conduct intake from a signed-in non-maintainer account; tracked under
      [PUB-001](https://github.com/seborbie/talos-community/issues/1), due 2026-09-11.
- [ ] Obtain qualified review of the dependency licence-expression allowlist and contribution/
      copyright notices. The automated policy is drift control, not legal advice.

## Source, provenance, and repository-export gates

- [x] Recover the exact upstream baseline for the patched `apps/vpx-encode/` copy: upstream tag
      `vpx-encode/v0.6.2`, revision `5519bec430184208ee33221ddc727ccb9429b88e`, published crate
      SHA-256 `cd1f41af42de7667cbdba44e8c38c36e9ede970f1291c32ea57c0ded8eb6f4b6`.
- [x] Exclude reconstructed `vpx-encode` implementation source from the public snapshot (retain only its generated Cargo manifest for Dependabot) and implement an exact
      crates.io-archive plus reviewed-patch acquisition path with input, patch, and output hashes.
- [ ] Obtain qualified approval of the `vpx-encode` notice position before redistributing
      reconstructed source or linked binaries. Its Git tree and published crate declare MIT but
      omit a standalone copyright/licence file; do not invent one.
- [x] Exclude historical WiX caches/extracts and the tracked 7-Zip SFX stub from the public snapshot;
      acquire exact WiX 6.0.0 and 7-Zip/LZMA SDK 26.00 inputs reproducibly with archive/package/member
      hashes and retained notices. Disclose that official WiX packages carry OSMFEULA maintenance-fee
      terms that each builder must review and satisfy.
- [x] Replace the provenance-unverified shared icon and cursor with a deterministic AGPL icon
      source plus pinned Tauri outputs and the pinned MIT-licensed Lucide cursor component; retain
      a blocked-path rule against reintroducing the old cursor file.
- [x] Review the Community screenshots supplied by the project owner for personal/customer data,
      credentials, hostnames, metadata, and third-party marks; bind every included JPEG to its
      reviewed SHA-256 in the export policy.
- [x] Recover exact Next.js provenance and retain the MIT notice for the five byte-identical
      Create Next App SVGs under `apps/frontend/public/`.
- [ ] Review and document every local patch to vendored source. Preserve all upstream licence files
      and notices; never relicense vendor trees through the root AGPL file.
- [ ] Generate complete dependency notices and an SPDX or CycloneDX SBOM from each actual source,
      binary, and container release payload. Verify every archive contains `LICENSE`, required
      notices, and its corresponding-source location.
- [x] Implement a disposable, allowlist-based source exporter that excludes private history/local
      state/generated output, refuses symlinks/submodules and unreviewed binary content, and records
      a machine-readable source/policy/content manifest.
- [ ] Produce the final clean integrated snapshot and run secret, personal-data, large-object,
      ignored-file, binary-provenance, and Git-submodule/LFS checks against that exact export before
      repository creation. Current incomplete exports truthfully retain owner/provenance blockers.

## Platform release gates

- [ ] Build and upgrade-test MSI/Burn artifacts on clean supported Windows hosts; verify versioning,
      prerequisite signatures, explicit unsigned-build warnings, updater manifests, install,
      rollback, and uninstall. Do not represent unsigned binaries as publisher-verified.
- [ ] Replace the default-disabled runtime SFX compatibility route with an immutable Windows
      bootstrapper that redeems a short-lived, single-use enrollment code. Never place an
      Authenticode private key in the API tier.
- [ ] Build macOS artifacts with an owner-controlled Developer ID, secure timestamp, notarization,
      stapling, and Gatekeeper verification before describing them as publishable.
- [ ] Build and install Linux artifacts in clean supported distributions and verify update/rollback.
- [ ] Build all release containers from a clean checkout and run a clean-volume Community smoke
      test covering migrations, authentication, agent registration, restart, backup, and restore.
- [ ] Complete deployment-specific threat modelling, observability, public exposure, external
      PostgreSQL, Traefik/ACME, and disaster-recovery verification for the supported topology.

## Release sign-off

Before tagging a public release, attach exact evidence for every applicable item above, run the
canonical gates in [ENGINEERING_QUALITY.md](../ENGINEERING_QUALITY.md), resolve or renew every
dependency-risk entry, and obtain qualified human review for authentication, remote execution,
cryptography, installers, updates, and legal/licence content.

## Initial source-alpha publication decision

The owner approved publication after reviewing the README and the remaining-checks report on
2026-09-04. [PUB-001](https://github.com/seborbie/talos-community/issues/1) records a scoped,
time-bounded follow-up for the unchecked human reviews and non-maintainer intake test.
This source publication does not certify those checks as passed or authorize a supported binary
release. See [publication evidence](source-alpha-publication.md).
