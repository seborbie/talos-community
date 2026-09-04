# Talos Community Edition development topology

This page describes the source-development and evaluation launcher. It selects five core services
from `infra/docker-compose.dev.yml` and therefore requires Bun and local image builds. It is not the
production deployment definition.

For released images, bundled or external PostgreSQL, migration-first startup, backup/restore, and
the stable Traefik interface, use the [production Community deployment guide](community-deployment.md).

## Start the core stack

Prerequisites:

- Bun 1.3.14;
- Docker with Docker Compose v2 and `compose up --wait` support;
- a configured `apps/.env` as described in the [development setup](development.md);
- relay certificates under `apps/certs/`, or the corresponding relay certificate path overrides.

The default filenames are `local-dev-relay-fullchain.pem` and `local-dev-relay-key.pem`. They are
not committed; see the [relay certificate guide](../apps/certs/README.md). `community:up` validates
the resolved host files before starting PostgreSQL, so a missing certificate cannot leave a partial
stack running.

From `apps/`:

```sh
bun run community:config
bun run community:up
```

Startup, migration, and config validation receive `apps/.env` through an explicit `--env-file`, so
URL, port, database, and certificate-mount interpolation use the same documented configuration. An
exported shell value still takes precedence when a one-off override is needed. Stop/down deliberately
do not require the file, so cleanup remains available if configuration is damaged or removed.
Before certificate validation or the first PostgreSQL phase, `community:up` rejects missing core
credentials and every credential value known to have been published as a repository placeholder or
former fixed bootstrap default. `community:config` applies the same policy. Errors name variables,
never their values. Generate unique values per installation; on Windows the repository setup script
creates independent CSPRNG-backed JWT-signing, application-encryption, RMM-server, service,
telemetry, AI-runner, and agent secrets. `APP_ENCRYPTION_KEY` is persistent: back it up and do not
rotate it with `JWT_SECRET` unless encrypted application data is migrated deliberately.

The launcher builds and starts only:

| Core service   | Responsibility                                                             | Default host endpoint          |
| -------------- | -------------------------------------------------------------------------- | ------------------------------ |
| `postgres`     | Durable application and control-plane data                                 | PostgreSQL on `localhost:3004` |
| `api_backend`  | Authentication, organization/device APIs, and direct telemetry persistence | `http://localhost:3001`        |
| `frontend`     | Operator web application                                                   | `http://localhost:3000`        |
| `talos_relay`  | TLS relay for agent/viewer session transport                               | `127.0.0.1:17443`              |
| `talos_server` | Agent connections and interactive control-plane APIs                       | `http://localhost:3002`        |

Every endpoint in this table is bound to `127.0.0.1` on the host by default. Compose service
listeners still bind their private container interfaces so the five services can communicate. To
make a specific endpoint reachable from another host, set its reviewed override from
`apps/.env.example` (`TALOS_FRONTEND_HOST_BIND`, `TALOS_API_HOST_BIND`,
`TALOS_RMM_SERVER_HOST_BIND`, or `RMM_RELAY_HOST_BIND`) and configure firewall, TLS, public URLs,
CORS, and authentication for that deployment. PostgreSQL remains loopback-only unless
`TALOS_POSTGRES_HOST_BIND` is explicitly changed.

On every `community:up`, the launcher first starts only PostgreSQL and waits for its Compose health
check. It then builds a disposable `api_backend` container and runs Prisma's non-interactive
`migrate deploy` command against that database; migrations already recorded as successful are
skipped. The five persistent services are started only after migrations succeed. A readiness,
API-image build, or migration failure exits non-zero and leaves PostgreSQL running for inspection;
the API, frontend, relay, and control server are not started. After correcting the reported
failure, re-run `community:up`. A migration that Prisma recorded as failed may require operator
recovery before a retry can proceed.

The final phase also uses Compose `--wait`: PostgreSQL, the API, frontend, relay, and control server
must all pass their health checks within two minutes. The API and frontend probes make local HTTP
requests from their containers. The two Rust services prove that their configured sockets are
listening; the relay does not receive a synthetic HTTP session that would pollute its pending-session
state. A zero exit from `community:up` therefore means all five persistent containers reached their
declared readiness condition, not merely that Docker created them.

The disposable migration container reuses the existing `api_backend` service definition and is
removed when it exits, so it does not add a sixth service to the persistent Community topology or a
second Compose model.

`community:config` validates the shared Compose model with the Community integration settings but
does not start containers. The launcher uses the dedicated `talos-community` Compose project, so
its lifecycle does not stop the full `talos-dev` project. `community:stop` stops only the five core
containers. `community:down` removes the Community containers and network; its persisted volumes are
retained.

The Community and full-development projects are lifecycle-isolated but are not intended to run at
the same time: the shared definition publishes the same host ports and retains a fixed PostgreSQL
container name.

## Core and optional features

| Capability                                                                 | Community default                                         | Full development stack      | Required optional services                                                         |
| -------------------------------------------------------------------------- | --------------------------------------------------------- | --------------------------- | ---------------------------------------------------------------------------------- |
| Organizations, users, sites, devices, policy data, and audit data          | Available                                                 | Available                   | None                                                                               |
| Agent registration and interactive relay sessions                          | Available                                                 | Available                   | None                                                                               |
| Direct snapshot, event, remediation-status, and patch-progress persistence | Available through the API fallback                        | Available                   | None                                                                               |
| Kafka-buffered telemetry ingestion and consumer projections                | Disabled                                                  | Available                   | `redpanda-0`, `topic-init`, `talos_telemetry_producer`, `talos_telemetry_consumer` |
| Telemetry topic inspection                                                 | Disabled                                                  | Available                   | `console` and `redpanda-0`                                                         |
| Autonomous AI-runner jobs and evidence capture                             | Disabled                                                  | Available                   | `talos_ai_runner`                                                                  |
| Local blob storage for feature-upgrade ISO media                           | Disabled; configure external Azure Blob storage if needed | Available with the emulator | `azurite`                                                                          |

To use external Azure Blob storage from Community Edition, set
`TALOS_COMPOSE_AZURE_STORAGE_CONNECTION_STRING` in `apps/.env` to a connection string reachable
from the API container. This Compose-only name is deliberate: the host-native
`AZURE_STORAGE_CONNECTION_STRING` example points at a host port and must not override container
routing.

The launcher explicitly sets the Compose-only integration controls
`TALOS_COMPOSE_TELEMETRY_PRODUCER_URL` and `TALOS_COMPOSE_AI_RUNNER_URL` to empty
strings. Empty means disabled. In the normal full-stack workflow, leaving those variables unset
retains their existing in-Compose service URLs. This distinction is intentional: Compose defaults
apply only when a variable is unset, not when it is explicitly empty. In the full telemetry
pipeline, the consumer projects patch progress durably into the API rather than serving a
replica-local progress cache.

Use the existing `bun run dev` workflow when working on Kafka telemetry and its consumer
projections, the AI runner, or Azurite-backed feature-upgrade media. It still starts the full
development stack.

## Direct UDP discovery is opt-in

STUN is disabled by default. A fresh worker or Viewer therefore does not contact a public STUN
provider. In the normal `RMM_VIEWER_TRANSPORT=auto` mode, interactive features use the negotiated
TLS and end-to-end encrypted relay fallback when direct UDP discovery is unavailable. Forcing
`RMM_VIEWER_TRANSPORT=quic` instead reports the missing STUN configuration as a connection error.

An operator who runs or explicitly trusts a STUN service may set `RMM_STUN_SERVER` to one
`hostname-or-IPv4:port` value on both worker and Viewer hosts, for example
`stun.community.example:3478`. URL schemes, credentials, paths, query strings, fragments, IPv6
literals, and port zero are rejected. STUN discloses the endpoint's source IP and UDP activity to
that service; it does not authenticate the Talos session or replace the relay certificate and
end-to-end encryption controls.

The web frontend and Viewer use local operating-system font stacks. They do not fetch Google Fonts
or another font CDN at runtime.

## Endpoint updates are opt-in

Community endpoint binaries contain no Talos-hosted update URL. The supervisor makes no update
request until `RMM_UPDATE_BASE_URL` points to an update API in the operator's deployment. The
desktop viewer uses explicit `RMM_VIEWER_UPDATE_BASE_URL` configuration when present; otherwise,
after opening an `rmm:` session it persists and reuses that session's API base. With neither source,
viewer update checks make no network request.

Configuring an endpoint only selects where to request updates. Manifests must still be signed by the
key embedded in the client, and downloaded packages must match the digest in the signed manifest.
See [ADR-0007](architecture/decisions/0007-opt-in-self-hosted-updates.md) for resolution order,
rollout, and trust-boundary details.

## Scaling and availability limitation

Community Edition is a single-host, single-replica topology, not a high-availability deployment.
PostgreSQL owns durable application data, but live agent connections, session coordination, and
other transient routing state are process-local. Do not horizontally scale `talos_server` or place
multiple replicas behind an arbitrary load balancer: a request routed to a replica that does not own
the relevant connection cannot complete the session.

The optional Redpanda pipeline is also omitted, so Community Edition does not provide Kafka-backed
buffering or independent telemetry-consumer scaling. A production-scale topology requires an
explicit design for shared connection routing, managed durable dependencies, failure recovery,
backups, observability, and replica-aware session affinity.

## Security boundary

The shared file is a development Compose topology, not a production hardening profile. Loopback
publication prevents another host from reaching its default database credential and development
services, but it is not an authentication boundary against other local processes. Replace database
defaults, keep only required ports exposed, manage secrets outside the repository, configure
backups, and perform a deployment-specific security review before remote exposure.

The API itself listens on loopback when run directly and trusts no forwarding proxy by default.
Compose sets the API listener to its private container interface while keeping the host publication
on loopback. A reverse-proxy deployment may set `API_TRUSTED_PROXIES` to an explicit comma-separated
IP/CIDR allowlist. Talos rejects blanket trust, hostnames, and numeric hop counts so a directly
reachable client cannot choose the address used by Express rate limiting via `X-Forwarded-For`.
Audit and installer code consume only Express's trusted address/protocol/hostname interpretation,
never forwarding headers directly. Reverse-proxy deployments must also set `PUBLIC_API_URL` and,
when the frontend has a different origin, `RMM_INSTALLER_PUBLIC_FRONTEND_URL`; the API cannot infer
an external proxy port or path safely from its container socket.
