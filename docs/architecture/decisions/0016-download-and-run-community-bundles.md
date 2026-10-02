# ADR-0016: Download-and-run Community evaluation bundles

- Status: proposed; qualified release/security review required before publication
- Date: 2026-10-02
- Owners: Talos maintainers

## Context

IT teams need to download a version and test Talos without compiling it. Main already supplies a
native Docker-backed launcher and protected candidate, GHCR publication, and prerelease promotion
phases (ADR-0004/0012). The bundle previously required four public DNS names and hand-edited ACME
configuration even for local evaluation, and users could not distinguish Linux/Windows downloads.
The owner has confirmed that Docker is an acceptable prerequisite for Windows.

## Options considered

- Package Bun/API, frontend, two native servers and PostgreSQL directly on Windows. This adds
  database distribution/licensing, process supervision, DLL/runtime, installer, port and Windows
  service/update boundaries. It requires a separate supported topology and extensive Windows
  integration evidence; it is unnecessary given the accepted Docker prerequisite.
- Publish a monolithic container with every service and the database. This couples durable-state
  ownership and failure/restart behavior and introduces another supervisor. A Docker socket inside
  that container would also expose host administration. Rejected.
- Improve the existing native launcher and release bundle. Selected: retain the tested process,
  migration and backup boundaries and reduce the operator's required configuration.

## Decision

Generate a ready-to-use `community-install.local.json` from the exact published image records, plus
`Start-Talos.cmd`, `start-talos.sh`, and `GETTING_STARTED.md`. Local names end in `.localhost`;
Traefik publishes only `127.0.0.1:8080/8443`. PostgreSQL stays on its internal network and named
volume. No shared password, default account, telemetry/AI service, or Redis is added.

The `quickstart` command checks Docker/Compose and Linux-container mode before creating state. It
selects the existing journaled install or start operation in a separate module. It requires local
routes plus bundled PostgreSQL and compares the entire requested configuration with the persisted
configuration. Repeated launches reuse secrets and image digests. Changed versions/settings need
an explicit update. Matching interrupted installs retry only before migration may have started;
other incomplete journals require explicit recovery.

Windows uses the current user's `%LOCALAPPDATA%\Talos\Server`; this avoids an unnecessary UAC step
with Docker Desktop. It retains the existing restrictive DACL. Linux uses `/var/lib/talos-server`
with existing privileged Docker access. One Compose project per daemon remains the limit.
Wrappers perform no runtime installation, privilege escalation, certificate-store changes or
Docker permission changes. Windows opens the browser only after successful quickstart and keeps
the console visible after success/failure. Native no-argument invocation still prints help.

Candidate OCI images carry source/revision/version/licence labels so GHCR links them to the
repository. Publication reads the exact digest back **without** an authfile/credential mount and
requires a match before handing records to the bundle. Owner configuration of public package
visibility remains explicit; the workflow never changes repository/package access settings.
Linux runtime smoke consumes the assembled archive and public image digests on a disposable hosted
runner. Its failure blocks the publication run and therefore prerelease promotion.

All existing source-export, vulnerability, licence, secret-scan, client-manifest-signing,
checksum/attestation and protected approval gates remain. Platform-named archives contain the same
complete audited inventory for the selected scope. Controller-only is the default and does not
include native clients. Their updater-manifest key is recorded as null/not applicable because the
server launcher neither embeds a client key nor updates itself. A checksummed, attested candidate
identity records the distribution; publication verifies its tag/version/source and carries it into
assembly. Full scope must be selected explicitly and still requires the protected signer and every
client-manifest/key-continuity check. This removes unrelated client signing prerequisites from
controller trial downloads without shipping unsigned updater-capable clients.

Runtime acceptance found that Docker allocated `172.31.240.2` to the relay before Traefik started,
causing `Address already in use` after migrations. All edge overlays now provide an automatic IPAM
pool that excludes the static trusted proxy address. The launcher derives the opposite half of a
validated subnet, reserves the first usable address for Docker's gateway, and retains the exact
API proxy allowlist. Existing networks need deliberate recreation with data retained; the normal
journal/recovery boundary remains. Tests reject removal of the allocation pool and verify default,
custom and smallest supported subnets.

## Trust boundary and failure analysis

The launcher controls Docker, which confers host administration. Its inputs are validated JSON,
immutable digest records, and protected existing journals; subprocess arguments remain separate
from shell code. Wrapper paths are quoted; no credentials are embedded. The downloaded local JSON
is non-secret but is protected before reading. Secret generation/storage remain owned by the
existing installer. No publication occurs on a branch or PR push.

Local browsers need explicit trust of the generated self-signed certificate for frontend **and**
API names. Automatically importing a certificate would modify an operator trust boundary, so it
is documented rather than silently performed. Local certificate renewal can require renewed trust.
Unsigned Windows execution remains subject to SmartScreen and organisation approval.

Wrong container OS, unavailable Docker, bad JSON, malformed/symlinked state, changed release and
incomplete migrations fail closed. A failed pre-migration pull can retry the same journal; data is
never reset to repair startup. A failed anonymous registry check can leave the already-approved
image private; owner visibility configuration followed by an idempotent exact-digest retry repairs
it without retagging. Smoke runs contain only synthetic users and no production infrastructure;
raw auth responses, generated credentials and certificate keys are excluded from evidence uploads.

## Consequences and verification

One archive, one launcher action, and browser setup replace source compilation and public DNS setup
for evaluation. Docker and certificate trust remain explicit prerequisites. Images cover amd64 and
arm64, while downloadable launchers cover x86-64 only. Windows/Linux binaries remain unsigned as
specified by the existing release policy. No offline installer or Windows service is claimed.

Focused tests cover dispatch, prerequisites, first/repeated startup selection, configuration drift,
and unsafe recovery. Bundle tests verify exact local image references, no credentials, checksums,
notices, and executable Linux modes. Linux release smoke covers anonymous released-image install,
first-account creation/closure, stop/restart and preserve-data uninstall with login persistence,
secret persistence, and loopback ports. Actual Windows double-click/DACL/SmartScreen, browser trust,
native installer/updater, ARM64 runtime, public ACME and real relay traffic still require the
existing clean-host evidence and qualified human review.

## Rollout and rollback

Integrate after independent review and required checks, without conflicting with pending #41/#40
or the separately owned dependency lockfile repairs. This work is stacked on the exact PR48
security repair, without duplicating its lockfile changes. Complete protected environment review,
owner package visibility, licence/source/security reviews, and platform evidence
before running publication. No new persistent credential is introduced; Actions uses its scoped
ephemeral token. No release is cleared by this ADR alone.

Only full scope additionally needs the updater-manifest PFX/password, approved public-key
fingerprint and isolated `talos-release` Windows signer. Controller scope uses hosted Windows
builds under the existing intentionally unsigned Authenticode policy. Qualified release/security
review remains required for both scopes.

Retain old bundles and verified off-host backups. Removing the wrappers or this release does not
remove state/volumes. The existing journal determines whether a configuration rollback is safe;
after migration may have started, restore the verified backup. Certificate trust removal belongs
to the operator's trust process.
