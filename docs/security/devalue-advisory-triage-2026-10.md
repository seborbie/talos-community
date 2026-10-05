# October 2026 devalue advisory triage

Talos `main` locked `devalue` 5.9.0 through Svelte and SvelteKit. The September 21 and 28
scheduled dependency-security jobs failed on [GHSA-9rgm-9g3h-6x36](https://github.com/advisories/GHSA-9rgm-9g3h-6x36).
Six additional reviewed advisories published to GitHub on October 1 affect versions through
5.9.2: [GHSA-j22f-vq7h-c4qm](https://github.com/advisories/GHSA-j22f-vq7h-c4qm),
[GHSA-hx4r-w6wj-j8fg](https://github.com/advisories/GHSA-hx4r-w6wj-j8fg),
[GHSA-mcm9-63f2-9j32](https://github.com/advisories/GHSA-mcm9-63f2-9j32),
[GHSA-wf3x-273g-mvxv](https://github.com/advisories/GHSA-wf3x-273g-mvxv),
[GHSA-x5rw-q4pp-hg5g](https://github.com/advisories/GHSA-x5rw-q4pp-hg5g), and
[GHSA-4q55-j62x-fr9h](https://github.com/advisories/GHSA-4q55-j62x-fr9h). They include
possible serialization of unrelated Node Buffer memory and resource exhaustion. The
upstream fixes require at least 5.9.3.

The frontend does not import `devalue` directly. SvelteKit can serialize server-side data
into HTML, making that boundary relevant even without a first-party call to `devalue.parse`.
Server-side route authorization and tenant scoping remain the owners of identity and access
decisions; this dependency update does not change them. No Talos exploit or affected `load()`
payload was demonstrated. Treat potentially serialized user, request, and tenant data as
sensitive. The reviewed mitigation pins every Bun workspace copy to 5.9.4, without adding
an advisory exception. A regression test rejects any resolved copy below 5.9.3 or an
unreviewed prerelease; the live advisory audit remains authoritative for new findings.

Failure modes include disclosure of unrelated process data in a server response and excessive
CPU or memory work. The safe response to a future advisory is to update the lockfile and
verify all resolved copies, not suppress the audit. Rollback to the prior lockfile restores
the affected release and needs explicit security review. This patch changes only dependency
resolution and does not add privileges, network access, logging, or credentials. Qualified
human review and cross-platform CI are required before integration.
