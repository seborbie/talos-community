# First controller prerelease: exact owner actions

Status on 2026-10-02: no GitHub release or approved download exists. Main is
`e76907551f335af84c439dadb431cb9f2fad0c7e`. The controller change is
[PR #50](https://github.com/seborbie/talos-community/pull/50), stacked on the security repair
[PR #48](https://github.com/seborbie/talos-community/pull/48). Both currently lack human reviews.
The implemented build/test source at `93687057762d4d7ad4f78442cb49b02f50721b57` passed
[all seven hosted quality jobs](https://github.com/seborbie/talos-community/actions/runs/36986467520).
Later README-only changes do not constitute release approval.

## 1. Unblock the reviewed source commit — needed next

- Name a qualified reviewer and obtain review of #48 and #50. The requirement is explicit in
  [AGENTS.md](../AGENTS.md#safety-sensitive-areas) and
  [ENGINEERING_QUALITY.md](../ENGINEERING_QUALITY.md#review-and-automated-agent-behavior):
  “release pipelines require qualified human review”. Resolve review comments and integrate
  through the repository's normal review process; rerun checks on the final integration commit.
  Preserve the separately owned #41/#40 publication work and #49 dependency repair.
- Include the authorised **controller-only alpha** distribution in that qualified review: Windows x64
  launcher, Ubuntu 24.04 x86-64
  launcher, four public amd64/arm64 application images, bundled PostgreSQL/Traefik, corresponding
  source and complete notices. Review [AGPL-3.0-only](../LICENSE),
  [third-party notices](../THIRD_PARTY_NOTICES.md), and the
  [licensing/provenance guide](licensing-and-provenance.md). The recorded
  [publication decision](../.config/public-export-policy.json) covers a **source-only alpha**;
  its existing scope is not a binary/container compatibility approval.
- Include the documented [unsigned Windows policy](release-signing.md), approved Docker Desktop
  licensing for target Windows organisations, local certificate trust, and the alpha limitations
  in [ADR-0016](architecture/decisions/0016-download-and-run-community-bundles.md).

This is the first required gate. The release tooling requires a **reviewed annotated**
`community-v<SemVer>` tag. A branch build or a draft GitHub release cannot replace that gate.
Local review builds are not final release assets, and are never described as approved downloads.

## 2. Choose a version and build the candidate

Choose the first alpha version and the exact reviewed integration commit. Create the annotated
`community-v<version>` tag on that commit; do not tag the unreviewed PR head as an approved release.
Run [Community release candidate](https://github.com/seborbie/talos-community/actions/workflows/community-release-candidate.yml)
from that tag with **include_native_clients = false**.

The current release workflow action pins are already permitted by the repository allowlist.
Recheck if integrating action-update PRs; do not change allowed-actions settings as a shortcut.

Wait for every gate: frozen dependencies, full quality and audits, fail-closed source export,
Linux/Windows release builds, both OCI architectures, high/critical vulnerability checks,
secret scans, SPDX SBOMs, checksums and attestations. Keep the successful candidate run ID.
Controller-only mode requires **no PFX, manifest-key fingerprint, persistent PAT, or self-hosted
signer**. Those are additional requirements only if native clients are explicitly included.

## 3. Approve the exact image publication

Run [Community release image and bundle publication](https://github.com/seborbie/talos-community/actions/workflows/community-release-publish.yml)
with the candidate run ID, the same tag, and that exact tag as the registry-write confirmation.
The existing **community-release-publish** environment requires approval from **@seborbie**.
Approve that run in GitHub's deployment review UI; no environment/security settings need changing.

For each resulting package (`talos-api-backend`, `talos-frontend`, `talos-server`, `talos-relay`),
the owner must confirm repository linkage and **Public** visibility in GitHub Packages. A first
copy can create a private package. If the workflow's anonymous exact-digest check fails, set that
package's intended visibility through the owner's approved process, then rerun the same candidate
and tag. Do not generate a PAT or overwrite an existing version tag to work around the failure.
The current CLI account cannot list package metadata without extra `read:packages` scope, so this
checklist does not assert their present visibility. The anonymous gate supplies the release proof.

Keep the successful publication run ID and Linux released-archive smoke evidence. That smoke
must verify anonymous pulls, installation, first-account closure, account/secret/data persistence,
loopback networking and confirmed disposable cleanup.

## 4. Complete clean-host evidence and approve GitHub Releases

Use the exact assembled archives on disposable supported hosts. Record Windows Docker Desktop
startup by double-click, repeat launch and failure console, DACLs, certificate/browser trust and
organisation SmartScreen approval. Complete Linux/Windows upgrade and deliberately failed-update
recovery, backup/restore and retained-data uninstall evidence. Mark every applicable checkbox in
[RELEASE_TEMPLATE.md](../.github/RELEASE_TEMPLATE.md) with supporting evidence; do not claim tests
that were not run.

Publish the non-secret evidence package at an authenticated HTTPS location and record its SHA-256.
Run [Community release prerelease promotion](https://github.com/seborbie/talos-community/actions/workflows/community-release-promote.yml)
with the successful publication run ID, same tag, evidence URL/hash, and exact
`PRERELEASE community-v<version>` confirmation. The existing **community-release-promotion**
environment requires approval from **@seborbie**. Its job verifies attestations/checksums/logs and
creates the GitHub prerelease with versioned archives and `SHA256SUMS`.

Then verify downloads and public image access again. Only after that succeeds should README's
availability notice be replaced with the exact approved release link. Keep the alpha and
unsigned-binary notices. No stable/production release is approved by this checklist.
