# PUB-001 source-alpha review packet

Prepared 2026-09-10; reassessed 2026-09-11 for [issue #1](https://github.com/seborbie/talos-community/issues/1).
Owner: Sebastian Orbe. The existing **2026-09-11** deadline is unchanged.
Outstanding acceptance items remain pending. Current source review may be completed by an
independent qualified AI or human reviewer under [the review policy](../review-policy.md).
This packet supplies review inputs; it does not
approve publication, extend the exception, or authorize a supported release.

The September and October 2 sections preserve historical observations and the policy in force
on those dates. Use the October 5 reassessment below for current integration status and remaining
gates. Green historical checks do not clear later advisories or complete outstanding acceptance.

## Revision and automated evidence

The initial production-source review target was `21cd13f0f04dbb0193f0c472cb5818ef81e3cf5b`
(main after PR #38).
[Quality run 34341760078](https://github.com/seborbie/talos-community/actions/runs/34341760078)
and [security run 34341760389](https://github.com/seborbie/talos-community/actions/runs/34341760389)
passed on September 9; their status was rechecked September 10. Quality includes Linux, macOS,
Windows and the disposable PostgreSQL integration job. These are prior hosted results, not newly
executed platform tests. Any material source change requires reviewing its diff and refreshing
applicable evidence.

The September 10 API check confirmed private vulnerability reporting was enabled; this evidence
is superseded by the September 11 status below. Only the GLib alert
remains open in GitHub. The [last hosted GLib updater](https://github.com/seborbie/talos-community/actions/runs/34341771460)
failed to find a compatible fixed resolution. [Issue #30](https://github.com/seborbie/talos-community/issues/30)
is still overdue (September 7). Green RustSec checks do not clear that finding: RustSec classifies
this advisory as informational unsoundness. No supported release should infer clearance from CI.

Local checks executed September 10: 31 publication/release contract, promotion and bundle tests
passed (97 assertions); the Cargo security regression passed (3 tests, 17 assertions).
`bun run publication:check`, `bun install --frozen-lockfile`, `bun run license:check` (318 Bun,
748 Cargo packages), and `bun run workspace:check` passed. Relative source links and
`git diff --check` passed. Full local quality and local PostgreSQL/platform execution were not
repeated for this documentation-only change; fresh hosted results belong on PR #40.

## Historical September 11 visibility and intake reassessment

The repository API now reports `private: true` and `visibility: private`. The private vulnerability
reporting endpoint returns HTTP 404. GitHub documents private vulnerability reporting for
[public repositories](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository).
The previously enabled intake cannot be relied on as a current compensating control. No alternate
private destination or non-maintainer submission has been verified. The security and conduct
policies now state this limitation and retain the prohibition on publishing sensitive details.

Branch-protection and ruleset inspection both return HTTP 403 with a plan/visibility restriction.
A blank GitHub review decision is not evidence that the required independent review occurred.
Continue to require the repository's review policy before any merge. This maintenance run did not
change visibility, protections, reporting settings, or the Actions allowlist.

PUB-001 remains open and due **today, September 11**. Sebastian must establish a verified private
intake route and record the outstanding qualified reviews or explicitly reassess the risks under
the existing exception process. The fallback channel is not yet concrete enough for acceptance.
Private visibility does not retroactively complete the source-publication follow-up or extend its
deadline. No supported release is cleared by this reassessment.

PR #40 at `bd961d426cf6a9d005f60839d552dfd9df715c96` passed all seven
[Quality jobs](https://github.com/seborbie/talos-community/actions/runs/34453229795) and its
[manual security audit](https://github.com/seborbie/talos-community/actions/runs/34453259926)
on September 10. These results were inspected September 11; they predate this documentation
correction. Independent approval remains absent.

## Private security and conduct intake

Read [SECURITY.md](../../SECURITY.md) and [CODE_OF_CONDUCT.md](../../CODE_OF_CONDUCT.md).
Both use the repository's private reporting form; conduct reports use the `[Code of Conduct]`
title prefix. A signed-in non-maintainer must verify both policy links reach the intended private
form and, in coordination with the owner, submit clearly marked benign intake tests. Use no real
vulnerability, personal information, credentials, or exploit content. The receiving maintainer
must confirm receipt and that the test content is not public. Record tester, date, policy revision,
and pass/fail in a private record; put only a non-sensitive result/reference on issue #1.

Acceptance requires successful non-maintainer submission and maintainer receipt for both routes.
An enabled API flag or an anonymous redirect to login does not meet that requirement. The current
session has not performed these submissions.

## First-user bootstrap: qualified security review

Read [ADR-0013](../architecture/decisions/0013-community-first-user-registration.md),
[registration logic](../../apps/api_backend/lib/communityRegistration.ts),
[HTTP auth routes](../../apps/api_backend/routes/auth.routes.ts), and
[PostgreSQL regression](../../apps/api_backend/tests/communityRegistrationPostgres.integration.test.ts).
Review the server-side validation, credential/token handling, transaction lock, authoritative
zero-user check, and failure responses together. Confirm the supported database isolation matches
the ADR. The lock serializes registration but cannot prove the first caller owns the installation;
the operator must bootstrap on a trusted local or restricted network. Deleting every user reopens
registration and must not be treated as routine account recovery.

The hosted PostgreSQL test covers first registration and login, subsequent HTTP 403, and a race
with one 201 and one 403. It is not evidence of independent-process restart, every rollback path,
or hostile-network deployment testing. Use a migrated, empty disposable `*_test` database for any
rerun; follow the `PostgreSQL integration` job in
[quality.yml](../../.github/workflows/quality.yml). Never point this fixture at an existing deployment.
Record reviewer, reviewed commit, findings and disposition on issue #1. The complete bootstrap
review by an independent qualified AI or human reviewer remains pending even if automated checks pass.

## Release pipeline: qualified review and platform evidence

Review the complete [candidate](../../.github/workflows/community-release-candidate.yml),
[promotion](../../.github/workflows/community-release-promote.yml), and
[publish](../../.github/workflows/community-release-publish.yml) workflows with
[bundle validation](../../apps/scripts/community-release-bundle.ts) and their tests.
Assess exact artifact/run/commit binding, checksum and signature failures, path/symlink rejection,
registry digest preservation, token permissions, environment approvals, and rollback. Contract
tests exercise selected invariants; they do not execute a signed installer or prove hosted
environment protection is configured correctly.

PRs #33–#37 propose workflow action updates. Independent review of the original exact pins
and upstream source does not approve a future main-based integration head or complete hosted
execution. Their five new pins remain outside the repository allowlist; the separately requested
setting change has not been approved as of the October 5 reassessment. Do not dispatch release workflows as a
substitute for review. Before a packaged release, attach supported-platform install/update/rollback,
Compose/ACME and backup/restore evidence plus complete binary notices/SBOMs as required by
[source publication scope](../source-alpha-publication.md).

## Licence, notices and name/logo: qualified owner review

Review [LICENSE](../../LICENSE), [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md),
[licensing/provenance inventory](../licensing-and-provenance.md),
[readiness checklist](../open-source-readiness.md), and
[licence policy](../../apps/scripts/license-policy.ts). Confirm first-party ownership, inbound
contribution terms, source licence compatibility, retained notices, and permission for each shipped
asset. Record an explicit permitted Talos name/logo policy for forks. The automated gate checks
metadata and reviewed expressions; it is not a legal compatibility opinion or trademark clearance.
Reconstructed vpx source and unreviewed third-party binaries remain outside this source publication.

## Completion record

For each section, the responsible reviewer records the exact revision, date, evidence reference,
findings, and accepted/blocked outcome on issue #1 without private report contents. Sebastian must
complete or explicitly reassess unresolved publication checks by September 11 under the existing
exception process. A reassessment must state the outstanding risk and disposition; this document
grants no extension. Keep issue #1 open until its acceptance requirements are actually met.

## Historical status reassessed September 14, 2026

GitHub now reports public visibility and private vulnerability reporting enabled. This supersedes
the September 11 availability evidence above; a non-maintainer submission and maintainer receipt
are still unverified. PUB-001 remains overdue from September 11, and its qualified reviews remain
outstanding. No exception deadline has changed.

The main branch protection endpoint now returns HTTP 404 (`Branch not protected`), and the
repository ruleset list is empty. These results differ from the earlier visibility/plan error.
Sebastian must review the missing protection configuration; maintenance has not changed any
security settings and continues to require independent review before integration.

PR #40 at `da80edb77a3f88b7274c0683ec21b5b1efa92907` passed all seven hosted Quality jobs
(run 34748147939) and dependency security (34748166328), executed September 13 and inspected
September 14. These results predate this status correction. PR #41 is still its prerequisite
and requires independent review. No integration or supported release is claimed.

## Historical status reassessed October 2, 2026

The current main revision is `e76907551f335af84c439dadb431cb9f2fad0c7e`. GitHub still reports
public visibility, private vulnerability reporting enabled, an unprotected main branch and no
rulesets. The non-maintainer intake and qualified human acceptance items remain unverified.
Issues #1 and #30 remain open with their original overdue dates. No security setting, deadline or
exception was changed by this reassessment.

Main's September CI predates the October dependency findings. The proposed
[security repair #48](https://github.com/seborbie/talos-community/pull/48), at
`6e1c9c22bc400c403c0079a20b337649d67861c3`, pins devalue 5.9.4 and rustls 0.23.45.
All seven [Quality jobs](https://github.com/seborbie/talos-community/actions/runs/36978858958)
and the [dependency audit](https://github.com/seborbie/talos-community/actions/runs/36978858927)
passed on that exact proposed head. The written
[devalue threat analysis](devalue-advisory-triage-2026-10.md) and
[rustls threat analysis](rustls-advisory-triage-2026-10.md) describe the affected boundaries and
verification limits. Those fixes are not on main and still require qualified human review.

GLib 0.18.5 remains unresolved in the Linux Tauri dependency graph. The owner chose to await a
supported upstream Tauri migration; no private backport or incompatible GLib override is proposed.
Passing RustSec policy with registered informational warnings does not establish advisory
clearance or supported-release readiness. PRs #33–#37 remain blocked by their proposed exact
action pins being absent from the repository allowlist. Release authorization and an Actions
security-settings change are separate outstanding decisions.

This documentation branch incorporates the refreshed release fixture prerequisite #41 and the
shared #48 security commits. Its residual change against #41 is documentation only. Retarget #41
after #48 merges, then retarget this PR after #41 merges, before deleting either base branch.
Fresh CI belongs to the updated PR heads; earlier September results remain historical evidence.
No release publication, protection bypass, or acceptance of the overdue review items is claimed.

## Status reassessed October 5, 2026

The actual main base for this refreshed packet is
`7f3d24e24fc06e576b1185de3474ab9980d36715`. PRs #48, #49, #52, #53 and #54
are merged: devalue 5.9.4/rustls 0.23.45, coordinated Tauri CLI/native package pins, the
Vite 8.3.0/Svelte plugin 7.3.1 migration and the independent AI-or-human review policy are
integrated. Actual main `63d97b557fe77948e4d12a0177af9dddb647a417` passed all seven
[Quality jobs](https://github.com/seborbie/talos-community/actions/runs/37353838518) and
[dependency security](https://github.com/seborbie/talos-community/actions/runs/37353935206)
on October 5. Those exact-main results verify that earlier integrated tree, not an uncreated
future documentation head.

[PR #55](https://github.com/seborbie/talos-community/pull/55) merged the corrected DR-008/DR-009
scope and incomplete-expiry record as `9065f727357b8ba1321c03e026a1750c34ef7403` after separate
independent source approval and all seven
[Quality jobs](https://github.com/seborbie/talos-community/actions/runs/37356893177) passed on
`f2151753cf51cfa32fd4fb3f98aa3d595bf48486`. Its documentation-only change did not alter the
previously audited dependency inputs. Issues #9 and #10 were synchronized without closing them,
changing the review date or granting risk acceptance.

The fixture prerequisite [PR #41](https://github.com/seborbie/talos-community/pull/41) is merged
as the current base above. Its final head `886b094c2efa7a818e941eb6518be9948c60798b`, based on
`9065f727357b8ba1321c03e026a1750c34ef7403`, received separate independent source approval. All seven
[Quality jobs](https://github.com/seborbie/talos-community/actions/runs/37359125075) and an explicit
[security-only audit](https://github.com/seborbie/talos-community/actions/runs/37359601167) passed
on that exact head. The actual merge tree was verified identical to the reviewed head. The
refreshed #40 head still needs its own independent review and fresh applicable checks; this
packet does not pre-approve itself or claim that main's post-merge workflow has finished.

At 18:39 UTC, read-only GitHub checks confirmed public visibility, private reporting enabled
and issues #1/#30 open. The 18:13 UTC protection snapshot returned `Branch not protected` and
an empty ruleset list. Protected release environments named the owner's account as reviewer,
allowed administrator bypass and did not prevent self-review. Documentary independence does
not prove stronger platform enforcement. No setting or release approval was changed. The
non-maintainer submission and private maintainer receipt remain unverified for both policy routes.
No alternate private destination was verified.

Audit success is policy-qualified. The raw Bun report retains high-severity
`GHSA-ggr8-5vv4-36mx` (`deepmerge-ts` 7.1.5 via Prisma CLI/config 6.19.3) and low-severity
`GHSA-pxg6-pf52-xh8x` (`cookie` 0.6.0 via SvelteKit 2.70.3). Prisma CLI/config remains in the
API image and is used for production migrations; absence of first-party request imports does
not remove that deployment exposure. Framework cookie serialization also remains present.
[DR-008/DR-009](../architecture/dependency-risk-register.md) explicitly record that expiry is
**not approved or established** and exception/release acceptance is incomplete. The November 17
review date is unchanged and is not risk acceptance. No compatible supported fixed resolution
was identified in the reviewed Prisma 6/SvelteKit 2 graph; no override or invented expiry was added.

GLib 0.18.5 remains unresolved in Linux Tauri. The owner-directed wait for a supported upstream
migration remains in force. Passing registered RustSec warnings does not clear it. Actions
#33–#37 await a separate exact-pin allowlist decision; no settings bypass is authorized. Release
source #50 still requires its refreshed-head review and checks. Full bootstrap and pipeline
review, source/licence/name-logo acceptance, non-maintainer intake, installer lifecycle, complete
binary/container notices/SBOMs and deployment/restore evidence remain pending. PUB-001 remains
overdue from September 11 and issue #30 from September 7. No exception, deadline, release
authorization or production configuration was changed.
