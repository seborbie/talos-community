# Deploy Talos Community Edition with production Compose

This guide describes the production application/database layers and their default public Traefik
edge. The base and database files publish no host ports; exactly one reviewed edge overlay provides
ingress. Read the [edge, DNS, and TLS guide](community-edge.md) before remote deployment. For source
development, use the [development Community workflow](community-edition.md) instead.

## What runs

The production base runs four persistent Talos services and two one-shot database jobs:

| Service              | Purpose                                                           | Private interface |
| -------------------- | ----------------------------------------------------------------- | ----------------- |
| `api_backend`        | Authentication, organization/device APIs, and durable persistence | `3001/tcp`        |
| `frontend`           | Operator web application                                          | `3000/tcp`        |
| `talos_relay`        | Agent/viewer session relay behind edge TLS termination            | `443/tcp`         |
| `talos_server`       | Agent connections and interactive control APIs                    | `17110/tcp`       |
| `database_preflight` | Validates configuration and runs non-destructive `SELECT 1`       | None; one-shot    |
| `database_migrate`   | Runs committed Prisma migrations                                  | None; one-shot    |

The default database overlay adds one PostgreSQL 16 container and the durable
`talos_postgres_data` volume. Redpanda, the telemetry producer/consumer, Azurite, and the AI runner
are not included.

Community Edition supports one Compose host and one replica of each Talos service. Live agent and
session routing state is process-local; horizontal scaling is not supported.

## Host prerequisites

- A supported Linux host with Docker Engine and Docker Compose v2.
- A reasonable starting point is 4 CPU cores, 8 GiB RAM, and 20 GiB plus retained database,
  artifact, log, and backup capacity. Actual sizing depends on device count and retention.
- Stable storage for Docker volumes and a separate protected backup destination.
- For remote access, four DNS names and inbound TCP 80/443 firewall/NAT rules required by the
  Traefik layer.
- Outbound HTTPS for image pulls, ACME, and any operator-configured storage/provider integrations.

The initial release does not install or privilege a container runtime automatically.

## Protected configuration

The native [`talos-server` launcher](../apps/talos_appliance/README.md) generates this file from a
versioned protected request. The manual steps below remain useful for direct Compose validation;
create any manual environment file outside the source checkout, set its mode to `0600`, and back
it up securely. Never reuse the example labels below as credentials.

```dotenv
# Every Talos image must be the exact digest emitted by one release. Tags alone are unsupported.
TALOS_API_BACKEND_IMAGE=registry.example/talos-api-backend@sha256:<64-hex-digest>
TALOS_FRONTEND_IMAGE=registry.example/talos-frontend@sha256:<64-hex-digest>
TALOS_RELAY_IMAGE=registry.example/talos-relay@sha256:<64-hex-digest>
TALOS_SERVER_IMAGE=registry.example/talos-server@sha256:<64-hex-digest>
# The launcher resolves the reviewed traefik:latest exception and persists its immutable digest.
TALOS_TRAEFIK_IMAGE=traefik@sha256:<64-hex-digest>

# Generate these independently with a cryptographically secure random generator.
TALOS_JWT_SECRET=<independent-random-value>
TALOS_APP_ENCRYPTION_KEY=<independent-persistent-random-value>
TALOS_RMM_SERVER_API_KEY=<independent-random-value>

# Public routes supplied by the edge deployment.
TALOS_FRONTEND_DOMAIN=talos.example.net
TALOS_API_DOMAIN=api.talos.example.net
TALOS_CONTROL_DOMAIN=control.talos.example.net
TALOS_RELAY_DOMAIN=relay.talos.example.net
TALOS_PUBLIC_FRONTEND_URL=https://talos.example.net
TALOS_PUBLIC_API_URL=https://api.talos.example.net
TALOS_PUBLIC_RMM_API_URL=https://control.talos.example.net
TALOS_PUBLIC_SOURCE_URL=https://github.com/seborbie/talos-community
TALOS_AGENT_SERVER_URL=wss://control.talos.example.net/agent/ws
TALOS_PUBLIC_RELAY_ADDRESS=relay.talos.example.net:443

# Default public ACME mode. Use a monitored mailbox.
TALOS_ACME_EMAIL=hostmaster@example.net

# The edge overlay sets API_TRUSTED_PROXIES to this exact private address.
TALOS_EDGE_SUBNET=172.31.240.0/24
TALOS_TRAEFIK_IPV4=172.31.240.2
```

`TALOS_APP_ENCRYPTION_KEY` is part of durable application state. Losing it can make encrypted
database fields unreadable. Do not rotate it as part of an ordinary update.

Set `TALOS_PUBLIC_SOURCE_URL` to the exact corresponding-source repository for the deployed build.
The frontend defaults to the official repository above; forks must point it at their own source.
Complete first-user registration on a trusted local/restricted network before exposing a new
installation publicly; the first caller on an empty database can claim its initial account.

Do not paste resolved `docker compose config` output into diagnostics or issues: it contains
container environment values. Use `docker compose ... config --quiet` for validation and
`docker compose ... config --images` when only image identities are needed.

## Default: bundled PostgreSQL

Add these values to the protected environment file:

```dotenv
TALOS_POSTGRES_USER=talos
TALOS_POSTGRES_DATABASE=talos
# Generate a long URL-safe value. The launcher uses a base64url/hex alphabet.
TALOS_POSTGRES_PASSWORD=<independent-url-safe-random-value>
```

From the release-bundle directory, use this exact file list:

```sh
docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-postgres.yml \
  -f infra/compose.community-traefik.yml \
  config --quiet

docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-postgres.yml \
  -f infra/compose.community-traefik.yml \
  up --detach --wait --wait-timeout 180
```

PostgreSQL is healthy before preflight starts. Preflight must finish before migrations, and
migrations must finish before the API. The frontend and control server wait for API readiness.
Re-running the command is safe: Prisma skips migrations already recorded as successful. Replace
only the Traefik overlay filename when using custom-certificate or local mode.

The database has no host port. Do not add one for routine administration; use `docker compose exec`
or a protected temporary maintenance path.

## External PostgreSQL

Omit `infra/compose.community-postgres.yml`; no YAML edit is required. Add exactly one connection
configuration to the protected environment file:

```dotenv
TALOS_DATABASE_URL=postgresql://talos_app:<percent-encoded-password>@db.example.net:5432/talos?sslmode=verify-full&connect_timeout=5
```

`sslmode=verify-full` is recommended. `verify-ca` and `require` are accepted for providers whose
connection model requires them. `disable`, `allow`, and `prefer` fail closed. `connect_timeout` must
be an integer from 1 through 30 seconds. The released container must trust the server certificate;
a private database CA or client certificate requires a reviewed mount/configuration overlay that
is not part of the initial base.

Use the base and selected edge files:

```sh
docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-traefik.yml \
  config --quiet

docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-traefik.yml \
  up --detach --wait --wait-timeout 180
```

If `TALOS_DATABASE_URL` is absent or invalid, `database_preflight` exits with a redacted,
configuration-specific error. API-dependent services cannot start. Transient connection failures
receive at most five Docker restarts in addition to the initial attempt; each connection attempt is
bounded by the URL timeout.

The current single database role performs both migrations and runtime queries. It therefore needs
`CONNECT` on the database, `USAGE` and `CREATE` on the application schemas, and ownership or the
equivalent create/alter/drop privileges for objects managed by committed Prisma migrations. It
must not be a PostgreSQL superuser and should have no unrelated database access. Network policy
should accept connections only from the Talos host.

The database operator owns availability, encryption at rest, backups, retention, point-in-time
recovery, maintenance windows, and restore testing.

## Edge, DNS, firewall, and proxy boundary

The base and database files expose no host socket. The selected Traefik overlay:

- join `talos_edge`;
- publish only its reviewed HTTP/HTTPS and TCP relay entrypoints;
- route to `frontend:3000`, `api_backend:3001`, `talos_server:17110`, and `talos_relay:443`;
- terminate public TLS for the relay before forwarding decrypted TCP;
- set `TALOS_API_TRUSTED_PROXIES` to the exact private proxy address/CIDR;
- persist ACME state without exposing its private keys to Talos application containers.

Do not expose the database, API, control server, frontend, or relay directly around Traefik. The
[edge guide](community-edge.md) covers public ACME, custom certificates, local self-signed mode,
DNS, NAT, IPv4/IPv6, forwarded headers, WebSockets, certificate backup, and renewal recovery.

## Stop, restart, and inspect

Always reuse the same mode-specific file list. These examples show bundled mode:

```sh
docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-postgres.yml \
  -f infra/compose.community-traefik.yml \
  ps

docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-postgres.yml \
  -f infra/compose.community-traefik.yml \
  down --timeout 60
```

`down` retains the named PostgreSQL volume. Never add `--volumes` during stop, update, rollback, or
troubleshooting.

## Bundled database backup

Create a logical dump before every schema-changing update and on a tested schedule. Protect the
output with `umask 077`, encryption, off-host retention, and integrity verification. The command
below keeps the password out of process arguments and writes the dump on the operator host:

```sh
umask 077
docker compose \
  --env-file /etc/talos/talos.env \
  -f infra/compose.community.yml \
  -f infra/compose.community-postgres.yml \
  -f infra/compose.community-traefik.yml \
  exec --no-TTY postgres \
  pg_dump --format=custom --no-owner --username talos --dbname talos \
  > /protected/backups/talos-YYYYMMDD-HHMM.dump
```

Back up `/etc/talos/talos.env` separately with equal protection. A database dump without the
matching `TALOS_APP_ENCRYPTION_KEY` is not a complete application backup. Regularly restore into a
disposable installation and verify representative records and a login; a file existing is not
proof that it is recoverable.

## Restore and disaster recovery

Restore is destructive to the selected target. Confirm the target, retain the original volume or
managed snapshot, and stop Talos application services first. For bundled mode, start only a clean
PostgreSQL target, restore the logical dump with `pg_restore`, then run the normal `up --wait`
command so migrations and health checks execute in order. Do not restore over a newer live schema.

For external mode, follow the provider's isolated-restore procedure and point a copied protected
configuration at the restored database. Validate TLS, preflight, schema compatibility, login, and
representative durable data before changing production DNS or connection state.

## Updates, failed migrations, and rollback

An update must record the current image digests, create and verify a database/configuration backup,
then replace all Talos image values with one reviewed release set. Do not mix image versions from
different releases. Run `config --quiet`, then `up --detach --wait`.

If preflight fails, inspect only its bounded logs and correct connectivity, TLS, credentials, or
privileges. If `database_migrate` fails, keep application services stopped and inspect the Prisma
migration record and database logs. Never mark a failed migration resolved without reviewing what
SQL committed and what did not.

If no schema migration ran, restoring the prior image digests is sufficient. If an incompatible
migration ran, restore the pre-update dump and protected configuration into a clean target before
starting the old image set. PostgreSQL major upgrades use a separately tested logical
dump/restore; changing the major image tag against an existing volume is unsupported.

## Current verification boundary

Repository tests validate the exact services, private port model, database selection, migration
ordering, environment ownership, runtime frontend configuration, PostgreSQL volume contract,
redacted URL preflight, the three edge-mode contracts, narrow proxy trust, fail-closed routes, and
the sole floating-image exception. Docker Compose model rendering is checked for database and edge
modes, and the templates are live-parsed against the current reviewed `traefik:latest` before a
release.

Clean released-image installation, real ACME issuance/renewal, external DNS/SNI/forwarding-spoof
tests, external TLS database integration, backup/restore, prior-schema upgrade/rollback, graceful
shutdown under load, and retained-volume restart remain release gates. They must pass with the
actual release bundle before the first Community release is promoted.
