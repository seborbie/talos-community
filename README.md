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

**Download availability:** the first controller bundles and public container images are being
prepared. No approved download is published yet. When a prerelease is approved, its versioned
archives, `SHA256SUMS`, source and notices will appear on
[GitHub Releases](https://github.com/seborbie/talos-community/releases).

### Windows x64 — download and double-click

1. Install/start your approved **Docker Desktop** with **Linux containers** selected.
2. From the same approved release, download `talos-community-<version>-windows-x86_64-UNSIGNED.zip`
   and `SHA256SUMS`. Verify the archive using the [download guide](docs/getting-started-downloads.md).
3. Extract the entire ZIP and double-click **Start-Talos.cmd**. The launcher starts the server
   stack, retains its database, and opens **https://talos.localhost:8443** when healthy.

### Linux x86-64 — run public containers

The download baseline is **Ubuntu 24.04 x86-64** with **Docker Engine and Compose v2 or newer**.
Download the approved release's `talos-community-<version>-linux-x86_64.tar.gz` and `SHA256SUMS`,
verify the archive, then extract it and enter its directory. Set `VERSION` to that release's version
without the `community-v` tag prefix:

```sh
VERSION='replace-with-release-version'
tar -xzf "talos-community-${VERSION}-linux-x86_64.tar.gz"
cd "talos-community-${VERSION}"
sudo ./start-talos.sh
```

The launcher pulls the release's public GHCR images (`talos-api-backend`, `talos-frontend`,
`talos-server`, and `talos-relay` under `ghcr.io/seborbie`) at their recorded immutable digests and
runs them with PostgreSQL and Traefik. No image build, registry login, separately installed
database, or Redis is required. Native clients are not included in the controller-only download.

For either platform, explicitly approve the generated **local certificate** using the
[download guide](docs/getting-started-downloads.md), then create your first account at
**https://talos.localhost:8443**. There is no default password; registration closes after the first
account. Evaluation listens only on this machine's loopback address. Follow the
[deployment guide](docs/community-deployment.md) when connecting remote devices.

To stop/start again, use `Start-Talos.cmd stop` / `Start-Talos.cmd` on Windows, or
`sudo ./start-talos.sh stop` / `sudo ./start-talos.sh` on Linux. Stopping preserves the database and
generated credentials. Neither platform requires Bun, Rust, Git, or a compiler to run the bundle.

### Build from source

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
