# ADR-0019: Allow independent AI or human review

- Status: proposed; owner-authorized policy change awaiting integration
- Date: 2026-10-05
- Owner: Talos maintainers

## Context

The owner explicitly removed the repository's human-only review requirement and accepted AI
review. Existing instructions, protocol guidance, security notes and release checklists name
human reviewers, while the substantive quality contract requires evidence, focused security
assessment, provenance/licence checks and supported-platform validation.

Review eligibility and permission to modify GitHub security controls or publish a release are
separate decisions. Existing changes-requested reviews and historical evidence gaps also remain.

## Options considered

1. Keep human-only wording. This contradicts the owner's current reviewer-eligibility decision.
2. Allow the authoring agent to approve itself or count passing CI as review. This removes
   independence and does not assess design, factual evidence or unresolved findings.
3. Permit independent qualified AI or human reviewers under one evidence standard while retaining
   validation, authorization and configured platform gates.

## Decision

Adopt option 3 through `docs/review-policy.md`. Update active instructions, contribution/protocol
rules, release/readiness checklists and dependency-review references consistently. Automated
changes obtain review from a separate review agent/session or qualified human. The record binds
reviewer identity, independence, scope, inspected evidence, findings and outcome to a full commit.

Retain exact lock/pin, threat-analysis, licence/source, tests, three-platform native validation,
release acceptance and exception requirements. The change does not supply missing legal facts,
retroactively complete unchecked evidence or dismiss prior owner reviews.

## Consequences

Reviewer type no longer blocks an otherwise qualified independent review. Required reviewers
must still demonstrate relevant coverage and record uncertainty; the author cannot self-certify.
This documentary policy is not an automated identity or truth verifier and does not establish
GitHub enforcement. Account-based protected environment approvals remain actual platform gates.

## Rollout

Publish a focused draft after #53 so that its newer native/dependency review documents are updated
without mixing dependency implementation into the policy diff. Independently review this policy
change, run the applicable existing policy/quality checks, and report actual GitHub settings
read-only. The owner's explicit instruction applies to reviewer eligibility now; branch integration
still follows the authorized reviewed PR flow. Reassess each outstanding PR's actual diff and
findings under this policy rather than treating the owner instruction as technical approval.

No branch, ruleset, environment, Actions allowlist, credential, workflow permission or publication
setting is changed. Return any later proposed setting change for action-time approval. No release
publication is authorized.

## Rollback

Restore the prior reviewer-eligibility text only through a new explicit owner policy decision.
Retain substantive evidence and validation requirements during any rollback. Do not rewrite past
review records or mark unresolved findings/evidence complete when policy changes.
