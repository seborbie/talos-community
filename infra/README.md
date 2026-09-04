# Talos development infrastructure

Docker Compose stack for local development: **PostgreSQL**, Redpanda (Kafka API), Redpanda Console, and Azurite (Blob storage). Used by the API, RMM server, RMM telemetry pipeline, and general dev workflow.

## Prerequisites

- Docker
- Docker Compose

## Quick start

From the **repository root**:

```bash
docker compose -f infra/docker-compose.dev.yml up -d postgres redpanda-0 console azurite
docker compose -f infra/docker-compose.dev.yml run --rm topic-init
```

Or use the monorepo scripts (from repo root):

```bash
bun --cwd apps run infra:up
```

For the five-service source-development topology, see the
[Community development guide](../docs/community-edition.md). For the released-image topology with
bundled or external PostgreSQL, see the
[production Community deployment guide](../docs/community-deployment.md). Public ingress,
Let's Encrypt, custom certificates, and local self-signed mode are documented in the
[Community edge guide](../docs/community-edge.md).

## Services

All published host sockets below bind to `127.0.0.1` by default. This includes unauthenticated
development infrastructure; it is intentionally not reachable from another machine.

| Service | Host endpoint | Notes |
| --- | --- | --- |
| frontend | `http://localhost:3000` | SvelteKit app |
| api_backend | `http://localhost:3001` | Express API |
| talos_server | `http://localhost:3002` | Rust Axum API |
| talos_telemetry_producer | `http://localhost:3003` | Telemetry ingest producer |
| PostgreSQL | `localhost:3004` | DB for API/RMM (PostgreSQL protocol) |
| Redpanda (Kafka external) | `localhost:3005` | External Kafka listener |
| Schema Registry | `http://localhost:3006` | Redpanda schema registry external |
| Redpanda Console | `http://localhost:3007` | Web UI |
| Azurite Blob | `http://localhost:3008` | Blob endpoint |
| Redpanda Admin API | `http://localhost:3009` | Redpanda admin API |
| talos_relay | `https://localhost:17443` | Local TLS relay published directly by Compose |
| talos_telemetry_consumer | _not published_ | Internal-only container |
| topic-init | _not published_ | One-shot init job |

Default Postgres credentials (override with env vars):

- **User:** `talos` (or `POSTGRES_USER`)
- **Password:** `talos` (or `POSTGRES_PASSWORD`)
- **Database:** `talos` (or `POSTGRES_DB`)

**DATABASE_URL** (for API and RMM):

```
postgresql://talos:talos@localhost:3004/talos
```

If a service must be reachable from another host, set only its explicit bind override (for example,
`TALOS_API_HOST_BIND=0.0.0.0` or `TALOS_POSTGRES_HOST_BIND=0.0.0.0`) and apply deployment-specific
firewall, TLS, authentication, and CORS policy. `apps/.env.example` lists all Community and
full-stack overrides. For an externally reachable Redpanda listener, also set
`TALOS_REDPANDA_ADVERTISED_HOST` to the name clients can resolve. Do not edit away the loopback
defaults globally.

The API container listens on its private Compose interface but its host socket remains loopback
only. A host-native API listens on `127.0.0.1` by default. The API trusts no forwarding proxy unless
`API_TRUSTED_PROXIES` explicitly lists proxy IP addresses/CIDRs or reviewed Express named ranges;
blanket trust and hop-count configuration are rejected.

## Stop

From repository root:

```bash
docker compose -f infra/docker-compose.dev.yml down
```

Or:

```bash
bun --cwd apps run infra:down
```

To remove persisted data (Postgres + Redpanda + Azurite volumes):

```bash
docker compose -f infra/docker-compose.dev.yml down -v
```

## Rust servers (full-stack build and run)

The five Rust services (`talos_server`, `talos_relay`, `talos_telemetry_consumer`,
`talos_telemetry_producer`, and `talos_ai_runner`) are defined in `docker-compose.dev.yml` and built
from one `infra/Dockerfile.rust-servers` for fast full-stack builds.

**When you run `bun run dev` from `apps/`:**

1. Infra (Postgres, Redpanda, Azurite) and DB migrations run as before.
2. **Rust services:** A hash of server source code is computed; if it changed since the last build,
   the five images are built (Docker layer cache keeps rebuilds fast). Then the services are started
   with `docker compose up` so they run as containers.

So you only pay for a full Rust build when you change code in the server crates; otherwise the script skips the build and starts the existing images.

**Manual build** (from repository root):

```bash
docker build -f infra/Dockerfile.rust-servers --target talos_server -t talos/talos-server:dev .
docker build -f infra/Dockerfile.rust-servers --target talos_relay -t talos/talos-relay:dev .
docker build -f infra/Dockerfile.rust-servers --target talos_telemetry_consumer -t talos/talos-telemetry-consumer:dev .
docker build -f infra/Dockerfile.rust-servers --target talos_telemetry_producer -t talos/talos-telemetry-producer:dev .
docker build -f infra/Dockerfile.rust-servers --target talos_ai_runner -t talos/talos-ai-runner:dev .
```

Or from `apps/`: `bun run docker:rust-servers` (builds all five; the first build compiles the shared
workspace and the remaining targets reuse its cache).

**Build profile:** Set `PROFILE=release` to build optimized binaries (e.g. `PROFILE=release docker compose -f infra/docker-compose.dev.yml build ...`). Default is `debug`.

**Relay TLS in Docker:** Compose mounts only the exact certificate and private-key files and defaults
to `apps/certs/local-dev-relay-fullchain.pem` plus `apps/certs/local-dev-relay-key.pem`. It never
mounts the containing directory because that directory may also hold updater-signing material.
`bun run dev` and `bun run community:up` validate both regular files before Compose starts. The
certificate's SAN must match `RMM_RELAY_URL`, and every connecting endpoint must trust its issuer.

For different host files, export `RMM_RELAY_TLS_CERT_HOST_PATH` and
`RMM_RELAY_TLS_KEY_HOST_PATH` before invoking the Bun launcher. Relative values are resolved from
`infra/`, like Compose volume sources. `RMM_RELAY_TLS_CERT_PATH` and `RMM_RELAY_TLS_KEY_PATH` remain
container paths below `/.certs/`; do not put host filesystem paths in those two settings. See
[`apps/certs/README.md`](../apps/certs/README.md) for local and public-certificate examples.

### Compose environment boundaries

The five Rust containers use explicit environment allowlists in Compose; they do not receive the
complete `apps/.env`. The checked inventory in
`apps/scripts/compose-environment-isolation.test.ts` must be updated whenever a service adds or
removes a setting:

| Service | Credentials deliberately retained | Other configuration retained |
| --- | --- | --- |
| `talos_server` | `RMM_SERVER_API_KEY` | API/relay/producer routing, bind/CORS, execution limits, logging |
| `talos_relay` | TLS private key through its read-only certificate mount only | bind, TLS paths/mode, pending-session expiry, logging |
| `talos_telemetry_consumer` | telemetry service key, RMM server key, local Azurite development key | broker/topics, API projection routes, retry/fetch limits, blob emulator, logging |
| `talos_telemetry_producer` | RMM server key | bind, broker/topics, logging |
| `talos_ai_runner` | AI-runner service key and RMM server key | API/server routing, job/command bounds, optional relay CA, logging |

Database, JWT, encryption, OpenAI, and unrelated endpoint credentials are not present in those
containers. The AI runner receives only one optional CA file, not the relay certificate directory.
When a private CA is needed, set both `TALOS_AI_RUNNER_RELAY_CA_PATH` (container path) and
`TALOS_AI_RUNNER_RELAY_CA_HOST_PATH` (host source file) as described in the certificate guide.

`api_backend` remains the sole `env_file` owner in this development model. It owns the repository's
broad application configuration surface (authentication/encryption, service integration, AI
provider, storage, and dynamically named update-artifact settings), and its container does not
mount the relay certificate directory. A production deployment should replace the development env
file with a deployment-specific secret/configuration provider and an independently reviewed API
allowlist.

To build only one server, use the per-crate Dockerfiles under `apps/talos_server/`, `apps/talos_relay/`, etc.

## Legacy installer SFX artifacts in Docker (API)

The request-time scoped EXE route (`POST /rmm/installers/profiles/:id/download-exe`) is an unsigned
compatibility path and is disabled by default. It is not suitable for Community or public release:
enabling it produces a different executable for each enrollment token, so it is not the immutable
file reviewed, checksummed, and attested by the release pipeline. The target replacement is one
immutable, explicitly unsigned-initially (or optionally Authenticode-signed by a fork),
single-use-code bootstrapper described in
[ADR-0010](../docs/architecture/decisions/0010-disable-runtime-sfx-assembly.md).

Only a developer who deliberately sets `RMM_ENABLE_UNSIGNED_SCOPED_INSTALLERS=true` can make the API
assemble the legacy 7z SFX executable from host-mounted artifacts:

- Host path: `apps/installer/artifacts`
- Container mount: `/installer-artifacts` (read-only)
- Default stub path inside container: `/installer-artifacts/dev/7zSD.sfx`
- Default payload archive path inside container: `/installer-artifacts/dev/Talos.Agent.Setup.7z`

You can override these with `RMM_INSTALLER_SFX_STUB_PATH`, `RMM_INSTALLER_PAYLOAD_7Z_PATH`, and
`RMM_INSTALLER_PAYLOAD_EXE_NAME` in Compose environment when needed for that explicit local-only
compatibility mode. Never place an Authenticode private key in the API tier.

## Feature upgrade ISO media

Feature Upgrade Center uses a DB-backed catalog and blob-backed ISO binaries:

- Default container: `talos-feature-upgrade-isos`
- API env: `FEATURE_UPGRADE_ISO_CONTAINER`, `FEATURE_UPGRADE_ISO_SAS_TTL_SECONDS`, `AZURE_STORAGE_CONNECTION_STRING`
- Local Compose points `AZURE_STORAGE_CONNECTION_STRING` at the Azurite service and starts Azurite with `--location /data --skipApiVersionCheck` so the current Azure Blob SDK works against the local emulator and writes to the mounted data path.
- By default, Compose writes Azurite's backing store to `apps/.azurite-data`, which is mountable on macOS and Windows. Set `AZURITE_DATA_HOST_PATH` to another host directory when you want the data elsewhere.

To seed local media, create the container in Azurite, upload the Talos-provided Windows ISO blob, then insert a matching row into `public.feature_upgrade_iso_media` with the same `container_name` and `blob_name`. The UI reads catalog rows through `GET /rmm/feature-upgrades/iso-media`; worker download URLs are only issued to the RMM server through the server-key protected download-link route.
