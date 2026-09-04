# Talos Community prerelease readiness report

- Status: **source-alpha publication authorized on 2026-09-04; packaged release gates remain open**
- Source licence: `AGPL-3.0-only`
- Intended history: one reviewed Community snapshot; private development history remains private
- Original report date: 2026-08-28; publication decision updated 2026-09-04

## What is ready for review

- An allowlist-based, fail-closed source exporter produces a disposable directory without copying
  private Git history, local state, generated output, secrets/certificates, or unreviewed binaries.
- The export manifest binds every included file to the source `HEAD`, Git tree/status state, policy
  digest, file digest/mode, and deterministic content-tree digest.
- Production Compose supports bundled PostgreSQL by default and an external-PostgreSQL mode.
- Traefik file-provider overlays cover public ACME, operator-supplied certificates, and local TLS
  without mounting the Docker socket or enabling its dashboard.
- `talos-server` is implemented as a no-Bun Linux x64/Windows x64 launcher with embedded Compose
  assets. Clean-host installation still requires Docker Engine, Compose v2, real digest-qualified
  Talos images, and a protected install JSON. External PostgreSQL additionally requires a protected
  backup destination before the first migration.
- The release pipeline assembles unsigned Windows artifacts, launcher binaries, immutable image
  references, checksums, notices, SBOMs, and provenance without exposing signing material to pull
  requests or general build jobs.

## Explicit limitations for the first candidate

- Initial Windows binaries are intentionally Authenticode-unsigned and may show `Unknown publisher`
  or reputation warnings. Updater manifests remain separately signed and packages remain
  digest-verified.
- macOS binaries are not publishable until owner-controlled Developer ID signing, notarization,
  stapling, and Gatekeeper checks exist.
- The supported deployment target is a single-host Community appliance. Multi-host orchestration,
  managed high availability, and an operational support SLA are not promised.
- The immutable, short-lived, single-use Windows enrollment bootstrapper remains incomplete.
- Real-domain ACME issuance/renewal, external PostgreSQL, clean Windows installer lifecycle, clean
  Linux install/update/rollback, and full backup/restore still need platform evidence.

## Source provenance status

### Closed engineering gaps

- `PX-001` source-export remediation: exact `vpx-encode` baseline provenance was recovered as
  upstream tag
  `vpx-encode/v0.6.2`, commit `5519bec430184208ee33221ddc727ccb9429b88e`, crates.io archive
  SHA-256 `cd1f41af42de7667cbdba44e8c38c36e9ede970f1291c32ea57c0ded8eb6f4b6`.
  The exporter withholds the reconstructed tree and publishes a digest-pinned fetch/patch recipe
  whose output matches the reviewed private copy exactly. This removes the anonymous-build gap
  without fabricating a missing notice. Qualified notice review remains a legal release gate for
  redistributing reconstructed source or linked binaries.
- `PX-002`: historical tracked WiX 6.0.2 cache/extract output and the SFX stub are excluded. Builds
  acquire exact official 7-Zip/LZMA SDK 26.00 and WiX 6.0.0 inputs, verify archives/packages,
  selected members, outputs, and retained notices by SHA-256, then use a local-only WiX feed. The
  official WiX packages' maintenance-fee agreement is disclosed and remains binding on builders.

### Owner/rights decisions that automation cannot infer

- The provenance-unverified shared icon and cursor are replaced. The deterministic AGPL icon source
  and its Tauri-generated ICNS/ICO/PNG outputs are exact-hash-bound; live frames use the pinned
  MIT-licensed Lucide cursor component. A guard retains the former cursor path.
- Eight first-party Community screenshots supplied by the project owner were visually reviewed for
  synthetic content, privacy, metadata, and marks, then bound to exact export-policy hashes. The
  superseded landing screenshot was not included because its pre-change trial wording is stale.
- Qualified licence review must decide whether the upstream `vpx-encode` MIT declaration and
  documented authorship are sufficient for the published patch and release binaries, or obtain an
  authoritative notice/replacement. Automation cannot invent a copyright notice.
The missing `vpx-encode` directory is no longer an anonymous-build failure: `bun run setup`
reconstructs it from the exact upstream archive plus reviewed patch. This is a controlled local
build input, not permission to redistribute it without the legal review above.

## Owner and external gates

The owner approved the source-only alpha with the outstanding reviews tracked in
[PUB-001](https://github.com/seborbie/talos-community/issues/1). Current decisions and follow-ups:

1. final README review completed and source publication authorized on 2026-09-04;
2. source and release notice verification against the confirmed identity: Sebastian Orbe,
   https://github.com/seborbie/talos-community;
3. private security/conduct reporting enabled; non-maintainer test remains outstanding;
4. logo/trademark and other media rights;
5. qualified licence/source review; and
6. repository creation/public visibility authorized; artifact/release approval remains separate.

## Verification evidence and gaps

- Focused exporter unit/integration tests cover deterministic output, dirty-source/blocker
  reporting, allowlist classification, binary digest enforcement, unreviewed binary rejection,
  symlink rejection, destination isolation, and fail-closed incomplete exports.
- Two independently generated snapshots from the 2026-08-28 integration checkpoint produced
  byte-identical manifests. Exact checkpoint hashes are retained in private review evidence rather
  than copied into this exported file: embedding the export's content-tree hash inside the content
  tree would make that value stale by construction. This is an explicitly incomplete checkpoint,
  not the future release identity; regenerate and retain its manifest from the clean integrated
  publication commit.
- Gitleaks is pinned by digest in release automation. Exact synthetic/public-fixture exceptions use
  path-and-value matching; the final integrated private history, final source snapshot, initialized
  public Git object database, release archives, SBOMs, images, and logs all require fresh scans.
- A follow-up digest-pinned scan of all 167 private commits retained seven redacted findings in old
  protocol-fixture and certificate/telemetry example paths. They match the deterministic/local test
  material from the initial triage; no unexpected live credential was identified. The disposable
  source directory, its fresh one-commit Git history, and a tar archive each returned zero findings
  under the digest-pinned Gitleaks image. No credential-rotation action was identified. Any future
  unexpected finding blocks publication and requires rotation before disclosure.
- Inside an anonymous disposable snapshot, `bun ci`, the digest-pinned `vpx-encode`
  reconstruction, workspace integrity, frozen-install dry-run, the 53 focused publication/export,
  acquisition, licence, installer, and release-pipeline tests, script type-check, licence gate, and
  publication-readiness gate passed. A separate clean reconstruction reproduced the reviewed
  `vpx-encode` file hashes exactly, and pinned installer-input acquisition verified the official
  archives, members, packages, and retained notices. Windows/.NET installer compilation, Compose
  smoke, and the remaining platform release gates are not claimed from this macOS checkpoint.

See [the export procedure](public-source-export.md),
[the full readiness checklist](open-source-readiness.md), and
[release signing](release-signing.md) before treating any candidate as publishable.
