# ADR-0020: Integrate the approved Actions runtimes

- Status: accepted for source integration; release acceptance remains separate
- Date: 2026-10-06
- Scope: setup-bun 2.2.0, setup-buildx 4.3.0, setup-qemu 4.3.0, upload-artifact 7.0.1 and cache 6.1.0

## Context

Dependabot PRs #33–#37 predate the verified Buildx acquisition sequence and some artifact handoffs.
Merging their patches without refreshing current callers could lose release protections or leave
newer uploads on an older runtime. The owner separately approved exactly these five action SHAs
for the selected-actions allowlist on 2026-10-06. That approval retains existing entries and keeps
both GitHub-owned and verified-publisher broad allowances disabled.

The new Docker, artifact and cache actions use Node 24. Hosted runners supply its runtime; any
self-hosted runner must support Node 24 and satisfy GitHub's current runner update/security policy.
Node 24 support alone does not establish protected Windows signer readiness.

## Decision

Integrate all five original PR heads in one main-based branch, preserving their commits and
reviewing the complete resulting diff. Resolve stale workflow conflicts by retaining current
release inputs and changing only the approved action references. Replace every current caller,
including uploads introduced after the Dependabot branches were created.

Retain the checksum-verified Buildx v0.37.2 helper, isolated Docker configuration, omitted setup
action version input, disabled binary cache, immutable BuildKit image and immutable ARM64-only
binfmt input. The new toolkit still permits an unverified download fallback; it does not enable
its optional Sigstore verification in this caller. The unconditional guard establishes the
verified existing-plugin path, rather than eliminating that upstream fallback.

Preserve Bun 1.3.14, frozen dependency installation, existing cache paths/keys and cross-OS defaults.
Keep upload's archived, hidden-files-excluded defaults. A separate hosted compatibility workflow
uses the same contracted container setup inputs, executes pinned Bun on amd64 and emulated arm64,
and verifies upload7/download4 byte checksums, nested ZIP contents and hidden-file exclusion.
It has read-only repository permissions, synthetic artifacts and no publication or signing steps.

## Alternatives

Keeping older actions would defer the runtime updates. Independently merging five stale branches
would require repeated conflict reconciliation; the combined branch preserves their history and
provides one final source revision for independent review and applicable CI. Broadening allowed
publishers or changing protected release settings is outside the approved scope.

## Consequences and remaining trust boundaries

The hosted check is compatibility evidence, not release qualification. Bun acquisition still
relies on upstream release transport without a separately verified binary checksum/signature.
The updated setup action can also execute and reuse an existing Bun installation when its reported
version matches the requested pin. This is an additional trust boundary on long-lived runners;
clean hosted checks do not attest that pre-existing executable. Cache contents remain untrusted
build inputs. Existing Buildx download fallback risk remains
documented. Independent source/provenance review, full quality/security CI, protected platform
validation, rights/notices/SBOM evidence and separate publication authorization still apply.

## Rollout and rollback

Merge only after exact-head independent review and applicable CI succeed. Check actual main's
quality, security and compatibility runs afterward and confirm the original PR inventory. A
source rollback restores the previous action references; those original pins remain permitted.
Do not dispatch release workflows or alter the allowlist again as part of rollback without the
owner's separate authorization.
