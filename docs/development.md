# Development setup

Run the commands on this page from the repository root unless a different working directory is shown.

This path runs Talos from source for local development and evaluation. It is not a production
deployment profile; for released images and production topology, see the
[Community deployment guide](community-deployment.md).

### Prerequisites

- **Bun 1.3.14** (pinned by `apps/package.json`)
- **Docker & Docker Compose** (for Postgres, Redpanda, Azurite – see `infra/`)
- **Rust 1.95.0** (pinned by `apps/rust-toolchain.toml`)
- **cargo-audit 0.22.2** (required by the full local quality gate; CI installs the same pin)

### Environment variables
Copy `apps/.env.example` to `apps/.env`; replace or remove every credential placeholder, and replace
the non-credential placeholders used by the services you enable. The shared Compose file explicitly
reads `apps/.env`; a repository-root `.env` is not a
substitute. Public example values and the repository's former fixed development credentials are
rejected before the API, Community stack, or full development stack starts. Generate independent
values with a cryptographically secure password/secret generator; the Windows
`scripts/Setup-DevEnviroment.ps1` path does this automatically for each checkout. Never reuse the
example strings below. For the default Docker Postgres from `infra/`, use:

Example `apps/.env`:

```
# Required
JWT_SECRET=replace_with_a_long_random_string
APP_ENCRYPTION_KEY=replace_with_an_independent_long_random_string
TOKEN_TTL=1h
MACHINE_TOKEN_TTL=30d
# Default dev Postgres (from infra/docker-compose.dev.yml)
DATABASE_URL=postgresql://talos:talos@localhost:3004/talos
RMM_DATABASE_URL=postgresql://talos:talos@localhost:3004/talos

# Local URLs/ports
API_BACKEND_URL=http://localhost:3001
PUBLIC_API_URL=http://localhost:3001
RMM_BIND_ADDR=127.0.0.1:3002
RMM_AGENT_TOKEN=replace_with_shared_agent_token
RMM_SERVER_URL=ws://127.0.0.1:3002/agent/ws
PUBLIC_RMM_API_URL=http://localhost:3002
# Local same-host example; remote agents need a reachable DNS name and matching certificate.
RMM_RELAY_URL=localhost:17443
# Optional overrides
# API_PORT=3001
# FRONTEND_PORT=3000

# Service-to-service auth
SERVICE_KEY=replace_with_shared_service_key
RMM_SERVER_API_KEY=replace_with_shared_rmm_server_key
```

`APP_ENCRYPTION_KEY` is a persistent data-encryption key, not a JWT-signing fallback. Generate it
independently from `JWT_SECRET`, back it up securely, and do not rotate it without migrating data
that was encrypted with the previous key.

Host-native API and Rust listeners, plus every Compose-published port, default to loopback. The
reviewed `*_HOST_BIND` controls in `apps/.env.example` allow intentional per-service exposure;
setting one to `0.0.0.0` is an operator security decision and also requires appropriate TLS,
firewall, origin, and authentication configuration. The API trusts no forwarding proxy unless
`API_TRUSTED_PROXIES` explicitly lists the proxy's address or CIDR. Blanket trust and hop-count
settings are rejected because they are unsafe when the API is also directly reachable.
Request-derived audit and installer metadata goes through that Express trust policy rather than
reading forwarding headers directly. Behind a reverse proxy, configure `PUBLIC_API_URL` (and
`RMM_INSTALLER_PUBLIC_FRONTEND_URL` when the frontend uses a different origin) to the externally
reachable HTTP(S) base URL; the backend cannot infer an external port or path safely.

### Install and database setup (one-time)
Run these from the repository root:

```
# Install every JavaScript workspace from the single committed lockfile and reconstruct the
# reviewed vpx-encode path dependency from its digest-pinned upstream archive plus Talos patch.
bun run --cwd apps setup
```

All JavaScript packages are declared in `apps/package.json`. Do not run installs in
individual packages or commit nested `bun.lock` files. Use `bun --cwd apps/<package> add <dependency>`
when changing one package's dependencies, then verify the root lockfile from `apps/`. The generated
`apps/vpx-encode/` directory is ignored and must match the acquisition policy exactly; the setup
command refuses changed or additional files instead of overwriting them.

The first time you run `bun run --cwd apps dev`, infra starts and migrations are applied automatically. If the database is fresh, migrations run; if already applied, they are skipped (idempotent). To run migrations manually (e.g. after pulling new migrations):

```
cd apps && bunx prisma migrate deploy --schema=api_backend/prisma/schema.prisma
```

Open `http://localhost:3000` after startup. When the database has no users, Talos allows one
first-user registration and then closes public registration. Talos then guides that user through
creating the initial organization, where they become its `SUPER_ADMIN`. Additional accounts must
be provisioned by a `SUPER_ADMIN` from **Organization Config → Users**; Talos ships no default
account or password.

### Run the dev environment
From the repository root:

```
# Start infra (Postgres + Redpanda + Azurite), ensure DB migrations, then start API, frontend,
# and the five Rust RMM services in Docker. Rust server images are built only when their code has changed.
bun run --cwd apps dev

# Or run API and frontend only (from apps/)
bun run --cwd apps api_dev   # API on http://localhost:3001
bun run --cwd apps web_dev   # Frontend on http://localhost:3000

# Run the API/frontend and all native Rust services as local debug processes.
bun run --cwd apps debug
```

`bun run debug` preserves every valid value supplied by the shell or environment files. When a
required local credential is missing—or still contains a published example marker—it generates a
stable replacement in the ignored `apps/.env.debug.local` file before starting infrastructure.
This includes a persistent debug-only `APP_ENCRYPTION_KEY`; values are never printed. Treat that
file as secret material and do not delete or rotate its encryption key while you still need data
encrypted with it.

### RMM relay (TLS stream endpoint)
For desktop streaming and remote view, `RMM_RELAY_URL` must be reachable from the agent and viewer,
and the relay certificate must contain that hostname. Compose publishes the relay's TLS port on
`127.0.0.1:17443` by default. A same-host setup can therefore use `localhost:17443`; remote endpoints
need a reachable DNS name, an appropriate host bind/forward, and a certificate trusted on those
endpoints.

Private keys and local certificates are intentionally not committed. Before `bun run dev` or
`bun run community:up`, provide `apps/certs/local-dev-relay-fullchain.pem` and
`apps/certs/local-dev-relay-key.pem`. Both launchers now fail before starting infrastructure if
those files are absent. [The certificate guide](../apps/certs/README.md) covers a locally trusted
`mkcert` setup, publicly trusted certificates, and exact custom file mounts. Compose exposes only
those two files to the relay, not the containing `apps/certs` directory.

Direct public-UDP discovery is opt-in: set the same operator-controlled `RMM_STUN_SERVER` value on
worker and Viewer hosts to attempt it. When the setting is absent, no public STUN service is
contacted and `auto` transport uses the relay fallback. See the
[Community runtime guide](community-edition.md#direct-udp-discovery-is-opt-in).

### RMM services
The five Rust services (talos_server, talos_relay, talos_telemetry_consumer,
talos_telemetry_producer, and talos_ai_runner) are started in Docker by `bun run dev`. To run a
single server locally (e.g. for debugging) from the `apps` directory:

```
# Rust RMM server (API on http://localhost:3002)
cargo run -p talos_server

# Rust RMM agent (check-in client)
cargo run -p talos_worker
```

### RMM viewer (native)
- Building `apps/talos_viewer/src-tauri` requires libvpx headers and libraries on your system.
- On Windows, run `scripts/Setup-DevEnviroment.ps1`. It checks out the repository-pinned vcpkg
  release, builds the reviewed libvpx overlay, records its provenance, and sets:
  - `VPX_INCLUDE_DIR` to your vcpkg include path (e.g. `C:\vcpkg\installed\x64-windows\include`)
  - `VPX_LIB_DIR` to your vcpkg lib path (e.g. `C:\vcpkg\installed\x64-windows\lib`)
  - `VPX_VERSION` to your installed libvpx version (e.g. `1.13.0`)
- macOS/Linux: install libvpx via your package manager and set `VPX_INCLUDE_DIR`/`VPX_LIB_DIR`/`VPX_VERSION` if pkg-config is not available.

### Virtual display driver (Windows)
For headless or virtual-monitor setups (e.g. RMM, streaming, or screen capture without a physical display), you can install the [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) via **winget**:

```powershell
winget install --id=VirtualDrivers.Virtual-Display-Driver -e
```

- **Project**: [VirtualDrivers/Virtual-Display-Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) on GitHub (Indirect Display Driver for Windows 10/11).
- **Before installing**: Install [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170) if you see `vcruntime140.dll not found`.
- **After install**: Use the Virtual Driver Control app from the [Releases](https://github.com/VirtualDrivers/Virtual-Display-Driver/releases) page to add or manage virtual displays; you can also configure `C:\VirtualDisplayDriver\vdd_settings.xml` for resolutions and options.
- **Driver updates**: Uninstall the virtual display driver before major GPU/chipset driver updates. If you get a black screen, boot into Safe Mode and uninstall it to recover.
- **Agent as a service (headless, no user logged in):** This repository does not currently ship a
  Talos virtual-display driver. Install and validate an operator-selected driver before relying on
  headless capture. As a less secure compatibility option, configure Windows auto-logon once and
  run `talos_worker --configure-headless` with `RMM_AGENT_HEADLESS_USER` /
  `RMM_AGENT_HEADLESS_PASSWORD` set, then reboot.

### Notes
- **Infra**: Postgres, Redpanda, and Azurite run via Docker Compose in `infra/`. Use `bun run --cwd apps infra:up` to start them, or `bun run --cwd apps infra:down` to stop.
- If a JavaScript tool is missing, run `bun --cwd apps install` from the repository root; do not
  install inside an individual workspace.
