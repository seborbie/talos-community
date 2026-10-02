# September 2026 dependency alert triage

Tracking: [issue #30](https://github.com/seborbie/talos-community/issues/30). Owner: Sebastian Orbe.
Review due: **2026-09-07**. Prepared 2026-09-05; this document does not close the issue or authorize a supported binary release.

## Priority and scope

The initial GitHub baseline had 11 alerts: five high, four moderate and two low. At preparation this was the highest actionable maintenance item: severity 4 (high-impact security), urgency 4 (due within 48 hours), score 16. No demonstrated Talos exploit or active exploitation was established by this review.

The local remediation updates three existing dependencies within their owning manifest constraints:

| Package | Before | After | GitHub alerts addressed by version |
| --- | --- | --- | --- |
| openssl | 0.10.75 | 0.10.80 | Eight alerts, including all five high alerts |
| serde_with | 3.16.1 | 3.21.0 | One moderate alert |
| rand | 0.8.5 | 0.8.6 | One low alert |

Cargo also updates openssl-sys to 0.9.117, serde_with_macros to 3.21.0, darling/core/macro to 0.23.0 and adds bs58 0.5.1 as required by the selected dependencies. The resolver reuses the existing windows-core 0.57.0 for iana-time-zone instead of 0.61.2. No package manifests or dependency constraints were overridden. Reviewed metadata requires at most Rust 1.88 for these updates, below the pinned Rust 1.95.0. The new bs58 dependency is dual MIT/Apache-2.0; the licence/source gate passes without new exceptions.

## Exposure and primary evidence

- OpenSSL is selected through reqwest -> native-tls/hyper-tls, with native OpenSSL implementation on Linux and other non-Windows/non-Apple targets. The all-target dependency graph reaches server, AI runner, telemetry consumer, supervisor, updater, worker and viewer packages. No direct use of the named OpenSSL APIs was found in first-party source; this is not proof that every transitive path is unreachable. Updating the wrapper avoids relying on that inference. The final required patch fixes AES key-wrap-with-padding buffer sizing: [maintainer advisory GHSA-phqj-4mhp-q6mq](https://github.com/rust-openssl/rust-openssl/security/advisories/GHSA-phqj-4mhp-q6mq). The other seven OpenSSL advisory IDs and thresholds are preserved in issue #30.
- serde_with is selected by tauri-utils in build-time and runtime dependency graphs. Inspected tauri-utils 2.9.2 source uses skip_serializing_none; no KeyValueMap use was found in that source or first-party code. The patched version removes the empty-entry serialization panic described in [maintainer advisory GHSA-7gcf-g7xr-8hxj](https://github.com/jonasbb/serde_with/security/advisories/GHSA-7gcf-g7xr-8hxj).
- rand 0.8 is used directly by the appliance and server and transitively by WebSockets and telemetry/SASL. [RUSTSEC-2026-0097](https://rustsec.org/advisories/RUSTSEC-2026-0097.html) describes reentrant custom-logger conditions. Updating to 0.8.6 removes the affected release rather than depending on logger reachability. The other locked rand lines, 0.9.5 and 0.10.2, already exceed their respective patched thresholds (0.9.3 and 0.10.1).

## Why the previous RustSec audit passed

The current downloaded RustSec database is commit 5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5, last updated 2026-09-02. It records rand RUSTSEC-2026-0097 and GLib RUSTSEC-2024-0429 as informational `unsound` findings, which are emitted as warnings rather than counted as vulnerabilities by the current cargo-audit command. Searches of that database found no corresponding new OpenSSL advisory aliases and no serde_with advisory directory. Thus a zero-vulnerability RustSec result does not establish that GitHub's findings are false. Both sources remain part of maintenance; no alert is dismissed or exception added.

The focused Cargo security regression supplements current audits by rejecting the reviewed affected OpenSSL, serde_with and rand version ranges in the workspace lockfile. It covers all three affected rand release lines and rejects unreviewed prereleases. It is not a complete advisory database. The old lockfile fails the regression and the patched lockfile passes.

## Remaining GLib finding and release boundary

GLib 0.18.5 remains selected by the Linux Tauri/GTK stack. [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html) affects VariantStrIter and is fixed in GLib 0.20.0. A forced transitive bump across incompatible GTK/GLib versions is not a supported fix. [DR-001 / issue #2](https://github.com/seborbie/talos-community/issues/2) continues to track the platform migration. The one remaining moderate GitHub alert must remain open until a compatible solution is integrated and verified.

This patch changes dependency resolution, not trust configuration, credentials, authorization, TLS policy or update-signing policy. Required human review and native CI remain necessary before integration. After merge, verify main CI, rescan the GitHub alerts, and confirm that the ten addressed alerts close while the GLib finding remains tracked. Do not claim resolution from this local lockfile alone. Rollback restores the previous lockfile but also restores the affected versions, so any rollback requires an explicit risk assessment.

## Validation

`bun run quality` passed locally on macOS with 408 JavaScript tests and 411 Rust tests, including the focused security regression. `bun run license:check` passed for 299 Bun and 748 Cargo packages. The security regression was also executed against the pre-update lockfile and failed as intended. Two opt-in PostgreSQL integrations were skipped locally because their database URLs were not configured. At preparation, Windows/Linux CI and the GitHub dependency rescan were pending. The integration verification below supersedes that status.

## Integration verified on 2026-09-06

PR #32 was merged into the repair branch, then [PR #31](https://github.com/seborbie/talos-community/pull/31)
landed both fixes on main as `10e1698ae892c73453252305abbec40c3f105231` on September 5.
[Main quality](https://github.com/seborbie/talos-community/actions/runs/33962065445) passed all seven
platform/integration jobs, and [dependency security](https://github.com/seborbie/talos-community/actions/runs/33962065398)
passed. These hosted checks were executed September 5 and inspected September 6; the local
validation above is historical evidence, not a rerun today.

The GitHub alert API now reports alerts 2–11 fixed, with fix timestamps on September 5: all eight
OpenSSL alerts, serde_with, and rand. Alert 1 (GLib, moderate) remains open. The current #30
priority is severity 3 × urgency 4 (September 7 deadline), score 12. No active exploit was
established. The [hosted GLib update](https://github.com/seborbie/talos-community/actions/runs/33962071554)
confirms that 0.18.5 is the latest resolvable version and 0.20.0 the first fixed version. Keep
#30 and DR-001 open while evaluating a supported upstream migration; no deadline or exception
has been extended. PUB-001 human reviews remain due September 11 (severity 3 × urgency 3 = 9).

On September 6, `bun test scripts/cargo-security-regression.test.ts` passed again against the
integrated lockfile (3 tests, 17 assertions). Full local quality was not rerun for this documentation-only update.
