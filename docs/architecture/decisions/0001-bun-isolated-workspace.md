# ADR-0001: Bun-native isolated workspace

- Status: accepted
- Date: 2026-08-17
- Owners: Talos maintainers

## Context

Talos had six independent JavaScript installs and lockfiles under `apps/`. Shared tool versions could
drift, package-local installs hid undeclared dependencies, the root setup command omitted one UI
package, and there was no repository-wide JavaScript check, test, or build command.

The project wants pnpm-style strict workspace isolation while retaining Bun's installer, runtime,
test runner, and performance.

## Options considered

### pnpm workspace with Bun used only as a runtime

This provides mature workspace isolation but requires two JavaScript package-manager toolchains and
two sets of operational conventions.

### Bun workspace with the default legacy/hoisted linker

This provides one install and lockfile, but an existing repository can retain hoisting and phantom
dependencies.

### Bun workspace with the isolated linker and strict resolution

Bun documents `linker = "isolated"` as its pnpm-like mode. It uses a central package store and
workspace-local symlinks. Bun also documents that its default fallback store hoist can still let a
store package resolve undeclared dependencies; `hoist = false` removes that fallback so undeclared
imports fail, matching pnpm's strict-resolution setting.

## Decision

Talos uses a Bun-native workspace rooted at `apps/` with:

- one committed `apps/bun.lock`;
- six explicitly declared first-party workspaces;
- `apps/bunfig.toml` setting the isolated linker and disabling the fallback hoist;
- an empty `hoistPattern` as the Bun 1.3.14-compatible encoding of that strict policy, retained
  until the pinned release parses `hoist = false` directly;
- Bun catalogs for dependencies intentionally standardized across packages;
- package-specific versions where native compatibility differs;
- filtered, frozen workspace installs in Docker and release scripts;
- repository-wide `check`, `test`, `build`, and `quality` scripts;
- Bun 1.3.14 pinned in `packageManager`, containers, setup tooling, and a runtime check.

Vendored upstream source is not enrolled as a first-party workspace. Any upstream lockfile retained
inside immutable vendor source is metadata for that source, not a Talos install boundary.

## Consequences

Positive:

- one deterministic dependency graph and reviewable lockfile;
- strict pnpm-like protection from phantom dependencies;
- shared commands cover every maintained JavaScript package;
- dependency changes invalidate container/native frontend caches correctly;
- Bun remains the only first-party JavaScript package manager and runtime.

Costs and risks:

- Docker stages must preserve both workspace-local symlinks and the root `.bun` package store;
- sparse/filtered container installs must copy every workspace manifest before installation;
- native and release scripts must install from `apps/`, not a leaf directory;
- consolidating historical locks can change transitive resolution, so all packages require build and
  test verification during migration.

## Rollout

1. Declare workspaces, catalogs, strict isolated linker, and pinned Bun version.
2. Generate one root lock while preserving previously locked direct dependency versions.
3. Remove leaf Bun lockfiles and update install/cache assumptions.
4. Run frozen-lock validation, all workspace checks/tests/builds, native builds, and container builds.
5. Add the same gates to CI.

## Rollback

The change can be reverted by restoring the package-local lockfiles and Docker/release install paths
from version control. No application data migration is involved.

## References

- [Bun workspaces](https://bun.com/docs/pm/workspaces)
- [Bun isolated installs](https://bun.com/docs/pm/isolated-installs)
- [Bun filtered commands](https://bun.com/docs/pm/filter)
- [Bun frozen installs and CI](https://bun.com/docs/pm/cli/install)
