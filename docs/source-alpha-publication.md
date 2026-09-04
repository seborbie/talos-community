# Community source-alpha publication — 4 September 2026

Sebastian Orbe approved the final README and publication to
https://github.com/seborbie/talos-community on 2026-09-04. The scope is an AGPL-3.0-only source
alpha. Supported binaries, container releases, signing credentials, and private development
history are outside this publication.

## Verification and publication controls

- The reviewed source passed the full `bun run quality` gate in a fresh policy-exported copy:
  399 JavaScript tests and 404 Rust tests passed. Both disposable PostgreSQL integrations ran.
  The macOS run reused the compiled Cargo cache; it is not an independent reproducible-build claim.
- The four reproduced registration, CI acquisition, promotion validation, and JavaScript audit
  defects were fixed with regression coverage. Only README/policy/documentation metadata changed
  after that full run; focused publication/export checks are repeated for the final snapshot.
- The source exporter excludes private history, local state, credentials, reconstructed third-party
  build inputs, and unreviewed binaries. The public repository starts with one snapshot commit.
- Private vulnerability reporting was enabled and verified by GitHub's API before source upload.
  Anonymous access to the reporting URL redirects to GitHub login with the correct return URL.
  This does not prove that a signed-in non-maintainer successfully submitted a report.
- GitHub secret scanning and push protection are enabled. Default workflow-token permissions are
  read-only. No signing secrets were provisioned and no binary-release workflow was dispatched.
- Existing dependency exceptions have public issues linked in the dependency risk register.
  The Rust audit retains 25 allowed warnings and reports zero vulnerabilities. The yanked
  `chacha20` warning is now tracked explicitly as DR-013; no vulnerability exception was added.

## Scoped follow-up: PUB-001

Tracking: [PUB-001](https://github.com/seborbie/talos-community/issues/1).
Owner: Sebastian Orbe. Expiry/review deadline: **2026-09-11**.

The exact unmet requirements are the non-maintainer reporting test and the outstanding qualified
human/security, licence/notice, and name/logo reviews recorded in the readiness checklist. The
owner authorized this source publication after receiving the outstanding-checks report; this
records that decision without claiming those reviews passed.

Pre-public-visibility reporting validation is impractical because GitHub offers this feature only
for public repositories. The publication session also has no signed-in non-maintainer account or
independent qualified reviewer. An empty public repository was therefore created first, then
reporting enabled before source upload. See
[GitHub's reporting documentation](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately).

The exception is limited to the initial source alpha. Its risks include an unverified reporter
journey and review findings that may still require source or notice corrections. Compensating
controls are enabled private intake, explicit prohibitions on public sensitive reports, the
allowlisted and scanned source snapshot, passing automated licence checks, prominent alpha
limitations, and exclusion of reconstructed vendor source and binary payloads. Complete or
explicitly reassess the tracked checks by the deadline; subsequent releases must not rely on an
expired exception. Do not convert this decision into legal, security, or production certification.

Platform installer lifecycle, complete binary notices/SBOMs, production Compose/ACME and
backup/restore evidence remain gates for an official packaged release.
