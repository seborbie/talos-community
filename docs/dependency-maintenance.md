# Dependency maintenance

Dependabot checks the Bun workspace and single lockfile in `/apps`, the Cargo workspace and single
lockfile in `/apps`, and GitHub Actions in `/`. Weekly version updates group minor/patch releases;
major updates remain separate. Cargo also includes transitive dependencies in routine updates.
Security updates are event-driven and do not wait for the weekly schedule. Alerts, update proposals,
and successful builds are separate signals: a passing RustSec audit does not dismiss GitHub alerts.

## Cargo manifests must be present in Git

Dependabot fetches manifests before invoking Cargo; it does not run `bun run setup` to create missing
path dependencies. `bun run workspace:check` now runs `dependabot:check`, which traverses local Cargo
members and path dependencies using only Git-visible files. Tests also verify those manifests survive
public export. Run it before committing a new local crate or changing export/ignore rules.

`apps/vpx-encode/Cargo.toml` is the exact generated metadata from the pinned upstream archive plus
Talos patch. Only this manifest is tracked. Setup verifies its SHA-256 before reconstructing the
excluded implementation and README. Do not edit the generated manifest independently: update the
reviewed patch and acquisition policy, reconstruct into a new directory with
`bun scripts/third-party-acquisition.ts vpx --repo-root .. --output <new-directory>` from `apps/`,
and copy its verified manifest. Review licence/provenance effects before changing that dependency.
Dependabot proposals that modify this manifest require the same coordinated regeneration; the
acquisition gate intentionally rejects metadata that no longer matches the reviewed patch.

## Limits and operation

Dependabot is not the updater for every pinned input. Rust/Bun toolchains, arbitrary script URLs,
the vpx patch, WiX/7-Zip acquisition policy, and container pins in Dockerfiles/Compose remain manual
maintenance items; the daily repository review must include them. Existing CI, licence, lockfile,
and advisory gates continue to apply to dependency changes. No advisory is ignored by this fix.

The owner has authorized autonomous maintenance branches, PR creation, review, and normal merges
once required checks and reviews are satisfied. Use temporary `codex/` branches, verify main after
integration, and delete merged maintenance branches after safely retargeting dependent PRs.
Author self-review does not substitute for independent qualified AI or human review under
[the review policy](review-policy.md). Repository security-setting changes and release actions
require separate authorization. Dependabot creates temporary PR branches when updates are
available; it cannot apply updates while permanently keeping exactly one branch.
Do not automatically delete those new proposals. Closed older proposals may require a Dependabot
recheck/recreation after the repair is merged.

After landing this change, run a Cargo Dependabot update from GitHub's dependency graph update page
and verify that it progresses beyond manifest fetching and produces either an update or a no-update
result. A local manifest check is not proof that GitHub's hosted updater has succeeded. Continue to
track the existing alert triage in [issue #30](https://github.com/seborbie/talos-community/issues/30).

### Tauri CLI release pin coordination

A viewer CLI dependency update must also update `TAURI_CLI_VERSION` in
`scripts/build-macos-viewer.sh` and the exact-version expectation in
`apps/scripts/release-input-contract.ts`. The 2.11.5 group update in PR #47 exposed this
coupling: the frozen install selected 2.11.5 while the release script still required 2.10.1.
Keep the runtime version comparison, frozen installation and Cargo `--locked` checks intact.
Run the release-input regression tests and repository quality gates, and obtain
independent qualified AI or human release review under [the review policy](review-policy.md)
before integration. Roll back the manifest/lock and script/contract pins together.

The upstream [2.11.5 CLI release](https://github.com/tauri-apps/tauri/releases/tag/tauri-cli-v2.11.5)
includes an updater signature/version binding fix. A passing frontend build or
CLI version check does not verify packaged application signing or installer execution.

### Vite and Svelte plugin major upgrades

Upgrade the four Vite consumers and the shared Svelte plugin catalog together. Vite 8.3.0 needs
the supported plugin 7 line; plugin 6.2.4 declares only Vite 6/7 peers and can strip imports used
only in Svelte template markup. Run the four preprocessing regressions and web CSS resolution
regression, zero-warning Svelte checks, production builds with ordinary warning reporting, and
browser verification. [ADR-0017](architecture/decisions/0017-vite8-svelte-plugin-migration.md)
records the coordinated graph, narrow Tailwind SSR configuration and rollback.

### Replacement groups and native package compatibility

Dependabot replaced #47 with #51 on October 5 and deleted #47's branch, which also closed the
stacked Vite proposal #43 unmerged. Preserve the reviewed commits in maintainer-owned recovery
branches; do not mistake either closure for integration. The replacement group incorporates the
shared #48/#49 repair ancestry while leaving those PRs' scope unchanged. Its CLI 2.12.1 pin moves
with the script and contract. Native Tauri API and Rust crate major/minor versions, including
installed plugins, must also match. Direct viewer WebView2/Windows core types must share Wry's
COM crate identities; frontend-only builds do not verify either boundary.

ADR-0018 records the Tauri 2.12.1 / deep-link 2.6.1 alignment and its regression. The regression
reads the workspace lockfiles/manifests directly because Tauri's npm-based discovery does not
report the installed packages in this isolated Bun workspace. Do not bypass its native version
check or infer compatibility from a CLI version output. Roll back JavaScript/Rust pins and both
locks together. The recovered native coordination and Vite migration were merged through
PRs #52 and #53 on October 5, preserving ADR-0017/0018 and their regressions. Native
installer/signing execution and independent qualified AI or human review remain separate gates;
the Linux GLib finding is unchanged.

### October 6 advisory refresh

The live Bun audit identified two additional findings after the previous main checks passed:
[proxy-addr IP spoofing](https://github.com/advisories/GHSA-jqcg-44mw-7w3h) and
[source-map-js source-map amplification](https://github.com/advisories/GHSA-68fv-2mgg-jv7q).
Use the compatible patched versions 2.0.8 and 1.2.2 respectively. Exact root overrides enforce
every transitive copy within all current parent ranges; no advisory ignore is added.

With pinned Bun 1.3.14, naming these undeclared children in `bun update` adds root runtime
dependencies and can leave vulnerable nested copies. Regenerate the lock with
`bun install --lockfile-only` after changing the overrides, inspect every resolved copy, and use
`bun ci --force` or a clean install to refresh existing isolated links. Ordinary install reuse
can retain an older transitive link even after the lock has changed. The generator also prunes
26 unreferenced esbuild platform records and their unused parent record left after the
Vite/Rolldown migration.

The proxy defect matters when an operator configures a short IPv4-mapped IPv6 subnet or an
all-zero IPv6 prefix. Talos accepts these CIDR forms, so patched Express must reject unrelated
IPv4 peers rather than believe their forwarding headers. Regressions exercise client/audit IP
and request origin through the real Express policy. Ordinary `trust proxy = false` remains
unaffected; no deployed configuration or credential bypass is inferred.

Source-map regressions resolve the implementation through both actual PostCSS versions and
Tailwind node, reject invalid or excessive indexed-map offsets without expanding their mappings,
and preserve valid lookups. Build inputs remain a trust boundary even though no first-party API
route was found accepting uploaded source maps. Run full checks, tests, frontend builds, live
advisory policy and independent review before merging. The existing Prisma/SvelteKit exceptions
and GLib migration remain separate unresolved findings.

## Hosted verification on 2026-09-06

The repair reached `main` in `10e1698ae892c73453252305abbec40c3f105231`.
[Main quality](https://github.com/seborbie/talos-community/actions/runs/33962065445) and
[dependency security](https://github.com/seborbie/talos-community/actions/runs/33962065398) passed.
The [hosted Cargo security update](https://github.com/seborbie/talos-community/actions/runs/33962071554)
progressed beyond manifest fetching and reported `security_update_not_possible`: GLib 0.18.5 is
the latest resolvable version, while the first fixed version is 0.20.0. This verifies the manifest
repair, but does not resolve the GLib finding or establish a successful routine Cargo update;
the concurrent routine Cargo run was cancelled. Bun and GitHub Actions updates produced new PRs.
