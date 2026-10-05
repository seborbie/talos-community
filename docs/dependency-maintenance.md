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

The owner currently wants only `main` on public GitHub. Maintainers prepare fixes locally until the
owner authorizes the normal reviewed change flow. Dependabot itself creates temporary PR branches
when updates are available; it cannot apply updates while permanently keeping exactly one branch.
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
Run the release-input regression tests and repository quality gates, and obtain qualified
release review before integration. Roll back the manifest/lock and script/contract pins together.

The upstream [2.11.5 CLI release](https://github.com/tauri-apps/tauri/releases/tag/tauri-cli-v2.11.5)
includes an updater signature/version binding fix. A passing frontend build or
CLI version check does not verify packaged application signing or installer execution.

### Replacement groups and native package compatibility

Dependabot replaced #47 with #51 on October 5 and deleted #47's branch, which also closed the
stacked Vite proposal #43 unmerged. Preserve the reviewed commits in maintainer-owned recovery
branches; do not mistake either closure for integration. The replacement group incorporates the
shared #48/#49 repair ancestry while leaving those PRs' scope unchanged. Its CLI 2.12.1 pin moves
with the script and contract. Native Tauri API and Rust crate major/minor versions, including
installed plugins, must also match; frontend-only builds do not verify that boundary.

ADR-0018 records the Tauri 2.12.1 / deep-link 2.6.1 alignment and its regression. The regression
reads the workspace lockfiles/manifests directly because Tauri's npm-based discovery does not
report the installed packages in this isolated Bun workspace. Do not bypass its native version
check or infer compatibility from a CLI version output. Roll back JavaScript/Rust pins and both
locks together. The recovered Vite migration remains a separate draft, preserving ADR-0017 and
its preprocessing/CSS regressions. Native installer/signing execution and human review remain
separate gates; the Linux GLib finding is unchanged.
