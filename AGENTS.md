# Talos repository instructions

These instructions apply to every human and automated contributor working in this repository.

## Required reading

Before changing code, read [ENGINEERING_QUALITY.md](ENGINEERING_QUALITY.md). It is the repository's
quality contract. More specific `AGENTS.md` files may add requirements for a subtree but may not
weaken the root contract.

## Required workflow

1. Inspect `git status` and preserve unrelated worktree changes.
2. Read the code, tests, contracts, and relevant architecture decision records before editing.
3. Keep mechanical refactors, dependency changes, generated output, and behavior changes separate
   whenever practical.
4. Add or update tests for changed behavior. Add a regression test for every bug fix.
5. Run the narrowest relevant checks first, then every applicable repository gate.
6. Review the complete diff. Report commands run, failures, skipped checks, and remaining risk.

Never claim a check passed unless it was executed. Never bypass or weaken a gate merely to make a
change green. A necessary exception must follow the documented exception process.

## Repository commands

JavaScript tooling and the Cargo workspace both live under `apps/`.

```sh
cd apps
bun install                 # local install; one workspace and one bun.lock
bun run workspace:check     # workspace membership, linker, and lockfile invariants
bun install --frozen-lockfile --dry-run
bun run contracts:check     # generated Rust -> TypeScript protocol contract drift
bun run license:check       # first-party SPDX and dependency licence/source policy
bun run check               # TypeScript and Svelte checks for all workspaces
bun run test                # tests exposed by JavaScript workspaces
bun run build               # production web/native frontend builds
bun run check:rust          # rustfmt and Clippy
bun run test:rust           # Rust test suite
bun run audit:js            # Bun advisory policy and registered exceptions
bun run audit:rust          # RustSec vulnerabilities; requires pinned cargo-audit
bun run quality             # full local quality contract
```

CI and release jobs must use `bun ci` (or `bun install --frozen-lockfile`) and Cargo `--locked`.
Do not create package-local Bun lockfiles or install dependencies independently in leaf packages.

## Architectural ratchets

- Do not add new responsibilities to known oversized entry points when a coherent module can be
  extracted. Except for emergency fixes, touching a god file should make it smaller or establish a
  tested seam toward decomposition.
- New functionality defaults to an in-process module. A new service requires an explicit,
  documented isolation, scaling, security, ownership, or deployment need.
- Durable or cross-replica state must not exist solely in a process-local collection.
- Do not independently hand-maintain equivalent HTTP paths and payload types in multiple languages.
  Change the machine-readable contract and generated/verified consumers instead.

## Safety-sensitive areas

Authentication, authorization, cryptography, remote commands, installers and updates, unsafe Rust,
database migrations, protocol compatibility, and release pipelines require focused tests and
qualified human review.
