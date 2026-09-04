# Talos Community release process

This process creates a reviewed candidate, promotes exact container bytes and assembles a no-Bun
bundle, then creates a GitHub **prerelease** only after a separate protected approval. None of the
workflows publishes from a pull request, branch push, schedule, or successful build automatically.

The workflows are implemented but have not been run to publish Talos. The source-candidate phase
intentionally fails while `.config/public-export-policy.json` contains unresolved owner or
provenance gates. Resolve those gates in a reviewed integration commit; do not use
`--allow-incomplete` for a release.

## One-time repository configuration

Create these GitHub environments with required reviewers, deployment-branch/tag restrictions, and
an approval timeout:

- `community-manifest-signing` protects the hardened Windows release runner and the two updater
  manifest signing secrets `TALOS_MANIFEST_SIGNING_PFX_BASE64` and
  `TALOS_MANIFEST_SIGNING_PFX_PASSWORD`. Configure its non-secret environment variable
  `TALOS_EXPECTED_MANIFEST_KEY_SHA256` to the exact lowercase 64-character SHA-256 fingerprint of
  the approved release-line public key.
- `community-release-publish` protects the only job with `packages: write` and the bundle assembly
  job.
- `community-release-promotion` protects the only job with `contents: write`.

The manifest-signing runner must have the labels `self-hosted`, `windows`, `x64`, and
`talos-release`. Treat it as an ephemeral release appliance: apply current OS patches, restrict
interactive and network access, pin the documented Bun/Rust/WiX/vcpkg/NASM inputs, clear the
workspace and temporary files after every run, and retain its audit log. A general-purpose or
pull-request runner is not suitable.

Store the PFX and password in separate owner-controlled systems before placing their CI copies in
the protected environment. Record the expected public-key SHA-256 outside GitHub, configure the
matching `TALOS_EXPECTED_MANIFEST_KEY_SHA256` protected-environment variable, and require a second
maintainer to review both. The candidate validates the variable as lowercase SHA-256 and refuses
the artifact handoff unless both `build-provenance.json` and `manifest.json` match it exactly. The
signing inputs and expected-fingerprint expression exist only in the named signer step; checkout,
dependency installation, and lifecycle scripts cannot see them. The step removes the environment
values before the build command and destroys the temporary PFX in `finally`.

Configure GHCR so only the protected publication environment can write the four Community image
packages. The bundle consumes digest references, never mutable tags. Restrict package deletion and
tag rewriting through the repository's owner policy even though an OCI digest remains immutable.

## Version ownership

Create one reviewed annotated tag named `community-v<SemVer>`, without SemVer build metadata. The
candidate workflow derives the release version and full source commit from that tag and rejects
branch or lightweight-tag dispatches. Component and installer versions remain derived from their
reviewed Cargo manifests by `build-installers.ps1`; they are recorded in the native artifact
manifest. The Community bundle version, image tag, image records, and launcher filenames derive
from the annotated Community tag. Do not type a second release version into a workflow.

## Phase 1: candidate (no publication permission)

Manually run `Community release candidate` from the annotated tag. It:

1. installs locked dependencies and runs the full repository quality contract;
2. invokes `public-source-export.ts` without a bypass flag, initializes a disposable one-commit
   history from that exact export, and scans both its history and archive;
3. builds Linux x86-64 and explicitly unsigned Windows x64 `talos-server` launchers;
4. builds native clients on the protected Windows runner, signs every updater manifest, records
   the public-key fingerprint, and explicitly marks Windows Authenticode as unsigned;
5. builds four linux/amd64+linux/arm64 OCI archives without pushing them, generates SPDX SBOMs,
   scans both platforms for high/critical vulnerabilities, and scans image layers for secrets; and
6. checksums and attests every handoff artifact.

The Gitleaks image is v8.30.1 at the reviewed digest in `.gitleaks.toml` and the workflows. Fixture
exceptions require a rule, exact path, and exact value. Do not add commit-wide or path-only
allowlists to make old private history pass; the public repository must start from the reviewed
exported snapshot/history.

If CI signing is unavailable, use the offline invocation in `docs/release-signing.md` on an
isolated Windows host. Transfer only the resulting release artifact directory through an
authenticated, access-logged channel and verify its `SHA256SUMS`, `build-provenance.json`,
`manifest.json`, signature inventory, source commit, clean-tree flag, release profile, and public
key fingerprint before it enters a protected release runner. Never transfer the PFX or password
with the artifacts. The standard GitHub candidate remains fail-closed until that verified handoff
is supplied through the protected runner; there is no unauthenticated upload shortcut.

## Phase 2: exact image and bundle publication

Manually run `Community release image and bundle publication` and provide the successful candidate
run ID, the same release tag, and the exact tag again as the registry-write confirmation. It
verifies the candidate run name, result, and source commit, scans its logs, and checks every
downloaded handoff checksum.

Only the protected image job can write packages. It copies the exact reviewed OCI archive with
digest preservation, reads both manifests back as raw bytes, and refuses a digest mismatch. The
bundle job then creates:

```text
talos-community-<version>/
  bin/linux-x86_64/talos-server
  bin/windows-x86_64/talos-server-UNSIGNED.exe
  clients/UNSIGNED-WINDOWS/
  compose/
  database/schema.prisma
  database/migrations/
  sbom/
  notices-and-guides/
  LICENSE
  THIRD_PARTY_NOTICES.md
  community-install.example.json
  image-references.json
  release-manifest.json
  SHA256SUMS
```

The launcher embeds the Compose assets, so the copied Compose/Traefik files are auditable recovery
material rather than a Bun/source-checkout runtime dependency. The generated install request uses
the four immutable published Talos image digests. Bundled PostgreSQL remains at its reviewed digest
in the embedded Compose/runtime assets; the launcher also uses that same reviewed third-party
digest as its isolated ACME-volume backup/restore helper. Neither role is mislabelled as a
Talos-built image. By owner decision, only install and explicit update resolve the official
`traefik:latest`; the launcher records and reuses the resulting digest on normal starts. A release
therefore does not claim a permanently pinned Traefik version.

The assembler can also be exercised locally after obtaining all protected inputs:

```sh
cd apps
bun ./scripts/community-release-bundle.ts \
  --repo-root .. \
  --output /absolute/output/talos-community-<version> \
  --release-tag community-v<version> \
  --release-version <version> \
  --source-sha <40-character-commit> \
  --linux-launcher /absolute/input/talos-server \
  --windows-launcher /absolute/input/talos-server.exe \
  --native-artifacts /absolute/input/native-release \
  --image-records /absolute/input/published-images.json \
  --sbom-directory /absolute/input/sbom
```

The command rejects an existing output directory, symlinks, checksum gaps, source/provenance
mismatches, missing signed-manifest pairs, contradictory unsigned status, incomplete image records,
mutable image references, and missing or malformed SPDX 2.3 SBOMs.

## Phase 3: clean-host evidence and prerelease

Before promotion, use the assembled archives and published digests—not a workspace build—on clean
Linux and Windows hosts. Retain an access-controlled evidence package covering:

- install and healthy status with bundled PostgreSQL;
- a same-line upgrade and deliberately failed update with the documented rollback/recovery result;
- verified backup and restore into disposable infrastructure;
- stop/start, diagnostics redaction, uninstall with data preserved, and confirmed destructive
  uninstall on disposable data;
- Windows DACL inspection and unsigned/SmartScreen behavior without disabling security controls;
- updater matching-signature success plus tampered manifest, wrong key, and wrong package digest
  failures;
- real WebSocket/relay traffic; and, for public mode, DNS/NAT/IPv6, Let's Encrypt staging then
  production issuance, renewal, and retained ACME state; and
- vulnerability, licence/notices, checksum, attestation, and known-limitations review by qualified
  humans.

Hash the final evidence package with SHA-256 and publish it at an authenticated HTTPS location.
Then manually run `Community release prerelease promotion` with the publication run ID, tag,
evidence URL/digest, and the exact `PRERELEASE community-v<version>` confirmation. Its protected job
verifies the publication handoff checksums and GitHub attestations, scans publication logs and final
archives, refuses to overwrite an existing release, and creates a prerelease from
`.github/RELEASE_TEMPLATE.md`.

Do not promote the prerelease to stable until every release-note checkbox has evidence. The
workflows attach provenance and metadata, but Talos does not claim reproducible builds until two
independent builders produce bit-for-bit identical outputs.
