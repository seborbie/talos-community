# Contributing to Talos

Thank you for improving Talos. Contributions are expected to leave the repository safer, clearer,
and easier to maintain.

## Contribution licensing

Talos uses inbound-equals-outbound contribution terms. First-party Talos Community Edition source
is distributed under `AGPL-3.0-only`; by submitting a contribution, you agree to license that
contribution under `AGPL-3.0-only` as part of Talos Community Edition and confirm that you have the
right to do so.

Contributors retain copyright in their work unless they enter a separate written assignment. A
pull request does not transfer copyright or grant proprietary/commercial relicensing rights beyond
the documented `AGPL-3.0-only` contribution terms. Talos does not require a contributor licence
agreement for ordinary inbound-equals-outbound contributions.

## Before you change code

1. Read [AGENTS.md](AGENTS.md) and the
   [engineering quality contract](ENGINEERING_QUALITY.md).
2. Search existing issues and architecture decisions for related work.
3. For a substantial behavior, protocol, storage, security, or deployment change, open an issue or
   design discussion before investing in an implementation.
4. Never include customer data, production credentials, private certificates, signing keys, or
   proprietary third-party artifacts in an issue or contribution.

## Local workflow

JavaScript tooling and the Cargo workspace live in `apps/`:

```sh
cd apps
bun install
bun run workspace:check
bun run contracts:check
bun run check
bun run test
bun run build
bun run check:rust
bun run test:rust
bun run audit:js
bun run audit:rust
bun run license:check
```

Use the pinned Bun and Rust versions. Do not create package-local Bun lockfiles. Add tests for
behavior changes and a regression test for every bug fix. Update documentation, generated
contracts, migrations, and architecture decisions in the same contribution when applicable.

## Pull requests

- Keep one coherent purpose per pull request and explain the user/operational impact.
- Describe trust boundaries, migration, compatibility, rollout, and rollback for sensitive work.
- State every command actually run and every check that could not be run.
- Preserve unrelated changes; do not reformat or regenerate broad areas without a reason.
- Maintainers may require qualified review for authentication, authorization, cryptography,
  updates/installers, remote command execution, unsafe Rust, or schema migrations.

Security vulnerabilities must follow [SECURITY.md](SECURITY.md), not a public issue. Support and
usage questions follow [SUPPORT.md](SUPPORT.md). Participation is governed by
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
