# Talos Community Edition

Open-source, self-hosted remote monitoring, device management, and remote desktop software.

## Why I made Talos

I started Talos as a personal project with a simple question: could I build a better RMM than the
tools available today?

I believe software is deflationary and should become more capable and more accessible over time. Managing your own
devices should be no exception. I'm sharing Talos so people can use it, understand how it works,
and help make it better.

— Sebastian Orbe

> **Alpha:** expect bugs, incomplete features, and breaking changes. Use a test environment;
> Talos is not ready for unattended production use.

## Quick start

For local evaluation, install **Bun 1.3.14**, **Rust 1.95.0**, and **Docker with Compose v2**.
From the repository root:

```sh
bun run --cwd apps setup
cp apps/.env.example apps/.env
```

Configure your own credentials in `apps/.env` using the [setup guide](docs/development.md), and
create the [local relay certificates](apps/certs/README.md). Then start the Community stack:

```sh
bun run --cwd apps community:up
```

Open [localhost:3000](http://localhost:3000), create the first account, and follow the organization
setup. Public registration closes after the first account; Talos ships no default password.

## More information

- [Documentation](docs/README.md): setup, configuration, deployment, architecture, and limitations.
- [Screenshots](docs/screenshots/community-edition/README.md).
- [Contributing](CONTRIBUTING.md), [support](SUPPORT.md), and [reporting vulnerabilities](SECURITY.md).

Initial official Windows binaries are intentionally **unsigned** and may trigger SmartScreen.
Verify `SHA256SUMS`; do not disable security controls. Read the [binary trust guide](docs/release-signing.md).

Copyright © 2026 Sebastian Orbe. Licensed under [AGPL-3.0-only](LICENSE).
[Third-party notices](THIRD_PARTY_NOTICES.md) apply to bundled and vendored components.
