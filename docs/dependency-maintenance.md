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
