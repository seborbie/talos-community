# ADR-0004: Layered production Community topology with selectable PostgreSQL

- Status: accepted
- Date: 2026-08-28
- Owners: Talos maintainers

## Context

Talos development includes PostgreSQL, the web/API control plane, relay and control server,
Redpanda, telemetry producer and consumer, Azurite, and the AI runner. A Community installation
does not need the optional telemetry, AI, or development-storage systems. It does need a deployment
definition that is independent from source builds and can be consumed by a no-Bun appliance
launcher.

The first Community launcher selected five services from `infra/docker-compose.dev.yml`. That
provided a useful low-dependency development path, but the shared definition still contained
source builds, development credentials and mounts, fixed host ports, and optional-service details.
It could not be described as a production deployment.

Operators also need two database choices. A default installation should create and retain a local
PostgreSQL database. An operator with managed PostgreSQL should be able to omit the local database
without editing YAML. Both modes must apply schema migrations before the API starts and must fail
closed when database readiness or migration fails.

The production frontend must be one released image. Its former `$env/static/public` API endpoints
were embedded during the image build, which would have required a separate image for every
installation domain.

Durable application state, live-session state, configuration, and credentials cross different
trust and recovery boundaries. The topology needs to make those boundaries visible before the
edge proxy and launcher are implemented.

## Options considered

### Continue selecting services from the development Compose file

This avoids another YAML model, but retains development builds, mounts, port publication, broad
configuration, and unrelated optional services in a production trust boundary.

### Use one monolithic production Compose file with PostgreSQL and the edge proxy

This makes the default command short, but external database users would have to edit or override
services manually. It also couples the application, database, and edge release cadences and makes
their security boundaries harder to test independently.

### Use a production base with explicit database and edge overlays

This creates a small amount of deliberate Compose layering. The file list, rather than YAML edits,
selects an operator-owned dependency. Each layer can have an exact contract and can be materialized
by the launcher.

## Decision

### Separate development and production models

`infra/docker-compose.dev.yml` remains the source-development topology. It may build local images
and include optional services.

`infra/compose.community.yml` is the production base. It references four required released-image
variables and contains no `build` entries or host-published ports. Its four long-running services
are exactly:

- `api_backend`;
- `frontend`;
- `talos_relay`;
- `talos_server`.

`database_preflight` and `database_migrate` are one-shot jobs that reuse the API image. Redpanda,
the telemetry pipeline, Azurite, and the AI runner are not part of the Community base.

The launcher and release bundle must supply immutable digest-qualified values for
`TALOS_API_BACKEND_IMAGE`, `TALOS_FRONTEND_IMAGE`, `TALOS_RELAY_IMAGE`, and
`TALOS_SERVER_IMAGE`. Compose requires those variables but cannot itself prove a registry digest;
the launcher/release validator owns that check.

### Stable edge and data interfaces

Every long-running application service joins the `talos_edge` network. The future Traefik overlay
must join that network and is the only supported layer that publishes host ports. The base uses
`expose` only as container-interface documentation; `expose` does not create host listeners.

Database clients additionally join the internal `talos_data` network. Bundled PostgreSQL joins
only `talos_data`, has no host port, and stores data in `talos_postgres_data`.

Traefik owns public TLS in the supported remote topology. `talos_relay` receives decrypted TCP on
its private port with `RMM_RELAY_TLS_TERMINATED=true`; it receives neither ACME state nor a TLS
private-key mount. The base leaves `API_TRUSTED_PROXIES` empty unless the edge layer supplies a
reviewed proxy allowlist.

### Database selection and validation

The default bundled mode combines:

```text
infra/compose.community.yml
infra/compose.community-postgres.yml
```

The overlay adds PostgreSQL 16 at the reviewed digest, a health check, its named volume, and the
database dependency edge. It requires a unique `TALOS_POSTGRES_PASSWORD`; no published password
default exists. The initial launcher must generate URL-safe password characters because Compose
cannot percent-encode a value while constructing the internal PostgreSQL URL.

External mode uses only `infra/compose.community.yml` and one `TALOS_DATABASE_URL`. The URL is
shared by preflight, migrations, and the API. It must:

- use the `postgres` or `postgresql` scheme;
- identify a user, host, and database;
- set `connect_timeout` between 1 and 30 seconds;
- set `sslmode=require`, `verify-ca`, or `verify-full`.

`verify-full` is recommended. `require` exists for managed services that encrypt connections but
do not expose a hostname-verifiable chain. Custom client-certificate and private-CA mounts are not
part of this base contract and require a reviewed overlay rather than an ad-hoc YAML edit.

The preflight validates configuration without printing the URL, then sends only `SELECT 1` through
Prisma. The URL stays in the child environment rather than its command arguments. Compose retries
the preflight with the bounded `on-failure:5` policy. The preflight neither creates nor alters
schema objects.

### Migration ordering and failure behavior

The startup graph is:

```text
bundled PostgreSQL healthy (bundled mode only)
  -> database_preflight completed successfully
  -> database_migrate completed successfully
  -> api_backend healthy
  -> frontend and talos_server
```

`database_migrate` runs `prisma migrate deploy` once and has `restart: no`. The API depends on
`service_completed_successfully`, so a failed migration leaves the database available for
diagnosis but prevents API-dependent services from starting. A failed Prisma migration is never
automatically marked resolved.

### Configuration and secret boundaries

No production service loads a broad repository `env_file`; every service has an explicit
environment allowlist. Database credentials reach only the two database jobs and API. JWT and
application-encryption keys reach only the API. The RMM server key reaches only the API and control
server. The relay receives no application or database credentials.

The launcher owns generation and atomic storage of installation secrets in a non-source-controlled
file with restrictive permissions. Compose interpolation puts necessary values into container
environments because the current applications do not yet implement file-backed secret inputs.
Operators with Docker-administration access can inspect those environments and are therefore in
the Talos host trust boundary. Commands, diagnostics, and logs must not render resolved Compose
configuration or secret values.

The frontend now reads public API endpoints through `$env/dynamic/public` at adapter runtime. Its
Dockerfile does not accept endpoint build arguments. One immutable frontend image can therefore be
used on different installation domains.

### State, backup, and single-host semantics

PostgreSQL owns durable control-plane and application data. `APP_ENCRYPTION_KEY` is durable
configuration required to read encrypted database values and must be backed up with the database.
The release/launcher state must also retain image identities and non-secret installation settings.

Agent connections, relay rendezvous, and interactive session routing remain transient,
process-local state. Community Edition therefore supports one replica of each application service
on one Compose host. Arbitrary horizontal scaling is unsupported.

Bundled backup uses PostgreSQL logical dumps plus the protected installation configuration.
External-database backup, retention, point-in-time recovery, and restore verification belong to
the database operator. Before an upgrade that can change schema, both modes require a verified
backup. A PostgreSQL major-version change uses a documented logical dump/restore migration; merely
changing the image major is unsupported.

## Consequences

Positive:

- production no longer inherits source builds or public development ports;
- bundled and external PostgreSQL are selected by a stable file list rather than YAML edits;
- migrations fail closed before the API starts;
- no optional AI, Kafka, or emulator services expand the Community boundary;
- the edge layer can route every application service over one stable network;
- ACME private keys remain isolated from the relay;
- one released frontend image supports installation-specific domains at runtime;
- durable database state and transient session state have explicit owners.

Costs and limitations:

- development and production Compose definitions deliberately duplicate some service settings and
  need contract tests to prevent security drift;
- direct Compose users must provide several validated values that the future launcher will
  generate;
- bundled PostgreSQL passwords are restricted to URL-safe generated characters until connection
  configuration no longer requires Compose string construction;
- one database role currently performs both migrations and runtime queries, so it needs DDL rights;
- externally managed databases using a private CA need an additional reviewed mount/config layer;
- clean-volume, upgrade, rollback, and restore tests need released images and remain release gates,
  not claims established by static Compose validation.

## Rollout

1. Add and statically validate the base and PostgreSQL overlay.
2. Add the non-destructive database preflight and tests for URL validation and secret redaction.
3. Move frontend service endpoints to runtime public configuration and build one generic image.
4. Add the Traefik/ACME overlay against `talos_edge`.
5. Make the launcher materialize the exact file list for the selected database mode and validate
   digest-qualified image references before invoking Compose.
6. Run clean-volume bundled and TLS external-database tests, then backup/restore and schema upgrade
   tests with released artifacts before promotion.

## Rollback

Stopping or removing the Compose project must retain `talos_postgres_data` by default. If no schema
migration ran, restore the previous image identities and restart using the same Compose file list.
If an incompatible schema migration ran, stop application services, restore the pre-upgrade dump
and protected configuration into a new/empty database target, then start the previous image set.
Never use `down --volumes` as an upgrade rollback mechanism.

The development launcher can temporarily remain available during rollout. Returning production
users to `docker-compose.dev.yml` is not an acceptable rollback because it would reintroduce source
builds, host ports, and development configuration into the production boundary.
