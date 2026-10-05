# Independent review policy

Status: repository policy, authorized by the owner on 2026-10-05
Applies to: required source, security, protocol, dependency, provenance and release reviews

## Eligible reviewers and independence

Required review may be performed by a qualified AI reviewer or a qualified human reviewer. The
same substantive standard applies to both. Qualification means the reviewer can assess the
change's relevant design, trust boundaries, failure modes, compatibility, provenance and evidence;
an AI label, account role or passing CI does not establish qualification by itself.

The author or authoring AI agent must not satisfy independent review by approving its own work.
For automated changes, use a separate AI review agent/session or a qualified human reviewer.
The reviewer must inspect the complete relevant source diff, repository policy, tests and actual
verification evidence. An author's summary can orient the review but cannot substitute for those
materials. Author self-checks remain required and are recorded separately.

If the reviewer cannot evaluate a required area or verify a factual claim, record the gap and
obtain additional qualified review or evidence. Do not invent identities, qualifications, legal
ownership, licence permission, successful executions or approvals.

## Required review record

Record the following in the PR review/discussion or a linked review artifact:

- The full reviewed commit SHA, base revision and scope. For a release, also identify the exact
  candidate/artifact digests, platform and evidence package.
- Reviewer identity: a named human or, for AI, the separate agent/session identifier and model
  when available. Identify the author/authoring session and state how the reviewer is independent.
- Source, contracts, threat analysis, licence/provenance records and verification evidence actually
  inspected. Report each applicable check as passed, failed or not run, with commands, logs or CI
  links. Do not turn a document checklist or tool invocation into proof of an unexecuted check.
- Findings, severity, required corrections and their disposition. An outcome must distinguish
  accepted findings, non-blocking risks, unresolved blockers and unavailable evidence.
- A clear outcome for that revision: approve, request changes, or incomplete. Resolve blocking
  findings and obtain re-review before treating the required review as satisfied.

Material source, dependency, configuration or artifact changes require review of the changed
revision and affected evidence. Documentation-only revisions still need their actual diff
assessed; an earlier review must not silently be relabelled as approval of a new commit. The
record may reference earlier evidence whose inputs are demonstrably unchanged.

## Controls preserved by reviewer eligibility

AI review does not replace or waive:

- Focused negative/regression tests, full applicable quality gates, locked dependency resolution,
  advisory policy or Linux/macOS/Windows validation for affected native changes.
- Written threat analysis, identity/authorization controls, unsafe-code review, protocol fixtures,
  migration/rollback planning, or the existing warning/exception process.
- Authoritative licence and ownership evidence, vendor notices, complete release SBOMs, source
  provenance, artifact digests and attestations. A reviewer can assess supplied evidence but cannot
  create missing rights or certify legal/security/compliance facts from an assertion.
- Clean-host signing, installer/update/rollback, backup/restore, exposure and release acceptance
  checks where applicable. Missing platform access is a disclosed blocker, not a successful check.

Historical unchecked reviews and evidence remain unchecked until actually completed. The owner's
reviewer-eligibility instruction does not dismiss an existing changes-requested review. Address
its substantive findings and obtain re-review; do not convert account metadata or the eligibility
change into approval.

## Review versus authorization and GitHub enforcement

Review assesses a concrete change; it does not authorize new external actions. Preserve the
owner's requested scope and explicit authorization for merging, settings changes, deployment and
publication. This policy grants no permission to change branch protection, rulesets, required
reviewer environments, Actions allowlists, credentials or signing identities, or to publish a
release. Proposed security-setting changes must be returned as exact, reviewable changes for
separate action-time approval.

Keep protected release environments and separate publication authorization. When GitHub requires
an account-based approval, an AI review record does not impersonate an approver or automatically
satisfy that platform gate. An authorized eligible account must perform the configured approval,
or the owner must separately approve a concrete settings change. Do not bypass a configured gate.

A source policy and a review record are not proof that GitHub enforces them. Repository-settings
snapshots report actual state separately. No setting or release workflow is altered by adopting
this policy. See [the engineering contract](../ENGINEERING_QUALITY.md),
[settings checklist](../.github/REPOSITORY_SETTINGS.md) and
[Community release process](community-release-process.md).
