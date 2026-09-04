# ADR-0012: Native Community appliance launcher and durable orchestration journal

- Status: accepted
- Date: 2026-08-28
- Owners: Talos maintainers

## Context

Talos Community is a single-host application composed of released frontend, API, control-server,
relay, PostgreSQL, and Traefik containers. Asking an operator to install Bun, clone the source
repository, generate several independent secrets, edit Compose YAML, sequence migrations, and
invent backup/rollback behavior is not an appliance-like deployment.

The production topology in ADR-0004 and edge boundary in ADR-0011 define stable Compose layers.
They intentionally do not own host prerequisite discovery, protected configuration, image
selection, operation recovery, backup, diagnostics, or data removal. Those responsibilities need
one versioned implementation rather than a collection of shell and PowerShell scripts.

Install, update, migration, restore, Docker control, certificate state, and secret storage are
safety-sensitive boundaries. A launcher failure must leave enough durable evidence to decide
whether automatic rollback is safe. It must not install a privileged container runtime or
silently resolve moving images during an ordinary restart.

## Options considered

### Keep documented manual Compose commands

This has no additional binary, but leaves validation, secret handling, file lists, migration
ordering, backup verification, and rollback decisions to each operator. The resulting deployments
would not have one supportable state model.

### Use a Bun or shell launcher

This would be quick to author but adds a runtime or shell-language boundary to clean hosts.
Quoting, platform variation, cancellation, and secret-bearing environment handling would be harder
to make uniform. It also conflicts with the downloadable single-program objective.

### Add orchestration to the existing talos_server control service

Rejected. That service runs inside the topology it would need to create, stop, replace, and
recover. Giving it host Docker authority would collapse the application and host-administration
trust boundaries.

### Add a separate native launcher

Selected. A small host-side Rust package can compile to one binary, embed the reviewed deployment
assets, use direct argument vectors, and keep orchestration modules separate from the existing
control service.

## Decision

### Process and package boundary

The Cargo package is talos_appliance; its installed binary is talos-server. The distinct package
name avoids collision with the existing talos_server control-plane package while the
operator-facing name matches the appliance.

The first supported hosts are Linux x86-64 and Windows x64. Other operating systems and
architectures fail closed before an operation. Docker Engine and Docker Compose v2 are explicit
prerequisites. The launcher locates an absolute Docker CLI path, verifies the daemon and Compose
major version, and never installs or enables a container runtime.

Docker administration is equivalent to host-root authority. The launcher and its state directory
are therefore an administrator boundary, not a tenant-facing API. It never exposes a daemon
socket to Traefik or an application container.

### Embedded deployment contract

The binary embeds the exact production base, optional PostgreSQL overlay, three mutually exclusive
Traefik overlays, and their file-provider configuration. At runtime it writes those versioned
assets into its protected state directory. An operator cannot select arbitrary additional YAML
through the launcher.

Configuration schema version 1 covers:

- one release version and update channel;
- four digest-qualified Talos images;
- bundled or externally managed PostgreSQL;
- four DNS names and one explicit edge mode;
- HTTP/HTTPS ports, the edge subnet, and exact proxy address;
- ACME email/directory or absolute custom-certificate paths; and
- the installation and backup directories.

An external database URL is accepted only in the protected request and moved into protected secret
state. The persisted non-secret configuration contains only its canonical scheme, host, effective
port, and database. That identity is re-derived from protected secrets on every load and must remain
unchanged during update, while username/password, TLS mode, and timeout rotation remain possible.
The URL must use PostgreSQL TLS and a bounded connection timeout; identity-override query parameters
are rejected.

### Secret and filesystem boundary

Independent JWT, application-encryption, RMM-server, and bundled-PostgreSQL values come from the OS
CSPRNG. They never enter process arguments. Compose receives them through one protected generated
environment file with an explicit service-side allowlist.

Writes use same-directory create-new temporary files, flush before rename, and reject symlink
traversal and non-regular targets. Unix directories and files use 0700 and 0600. On Windows, the
launcher replaces inherited DACLs through the Windows security API with a protected, inheritable
allowlist granting full control only to the object owner, BUILTIN Administrators, and SYSTEM.
Failure to construct or apply that ACL aborts the operation; Windows never falls back to inherited
permissions.

Local-only mode generates a 30-day self-signed certificate with all four configured SANs, stores
its key under the same protection, and renews it before the final three days. Public ACME state
remains owned by the Traefik volume. Custom certificate keys remain operator-owned protected
files.

### Commands and subprocesses

Install, start, stop, status, update, backup, restore, diagnostics, and uninstall consume one state
model. Every child process receives a program plus individual argument vector; there is no
command-shell interpolation. Operations have timeouts, kill an overrun child, drain bounded
output, and redact exact secret values and database URLs before reporting failure.

The launcher clears the caller environment before every child and restores only the Windows process
runtime variables `SYSTEMROOT`, `WINDIR`, `TEMP`, and `TMP` when present. It does not carry `HOME`,
`PATH`, `USERPROFILE`, `XDG_CONFIG_HOME`, or any `DOCKER_*`/`COMPOSE_*` endpoint, context, credential,
plugin, or Compose routing configuration into privileged Docker operations.

Ordinary starts pull only a missing recorded digest. New installs and explicit updates alone pull
the owner-approved traefik:latest exception, resolve it to an official repository digest, record
its reported version and resolution time, and pass the digest to Compose. Talos images are always
digest-qualified.

### Durable state, sequencing, and concurrency

The launcher supports one Compose host and serializes mutations with a create-new operation lock.
Configuration, secrets, embedded assets, image identities, previous known-good version, verified
backup name, lifecycle, active operation, and durable checkpoint live on disk. Process-local
collections are not the source of truth.

Install and update order is:

1. validate configuration and prerequisites;
2. materialize protected state and reviewed assets;
3. pull and resolve images;
4. make bundled PostgreSQL healthy when selected;
5. run the non-destructive database preflight;
6. durably record that migration is about to start;
7. run committed migrations;
8. start services and require Compose health; and
9. promote the candidate images and clear the journal.

An interruption before the migration-start checkpoint can restore the protected previous
configuration and known-good image digests. Once migration may have started, application services
stop and the verified backup is required unless the operation changed only Traefik. The launcher
never claims that reversing container images reverses a database schema.

### Backup, restore, diagnostics, and removal

Bundled mode creates and verifies a custom-format PostgreSQL logical dump. External mode requires
an operator/provider-created protected backup path and records that ownership. A complete backup
also contains protected configuration, secret state, the durable journal, and either ACME or local
certificate state. Every entry is size-recorded and SHA-256 verified before promotion.

Restore requires the installation identifier, validates every manifest path and hash, preserves a
pre-restore configuration recovery copy, and refuses to alter an external database until its
operator confirms that restore is complete. Bundled restore recreates the selected database,
loads the verified dump, reruns preflight/migrations, and requires health.

Diagnostics include bounded service state, versions/digests, routes, disk capacity, and redacted
recent logs. They never contain the protected environment, database URL, or certificate private
state. Exact served-certificate expiry probing is not implemented by this launcher revision and
remains an explicit release gate.

Uninstall defaults to stopping containers while preserving state and volumes. Data removal needs
the exact installation identifier, verifies the state marker, refuses broad paths and nested
symlinks, removes Compose volumes explicitly, then removes only the selected state root. Removing
the launcher program itself remains the host package manager's responsibility.

## Consequences

Positive:

- a clean supported host needs one Talos binary plus Docker Engine/Compose, not Bun or a checkout;
- setup and recovery rules have one tested implementation;
- normal restarts cannot silently move Traefik or Talos images;
- migration rollback boundaries and durable-data ownership are explicit;
- secrets, diagnostic output, and destructive paths have focused negative tests.

Costs and limitations:

- Docker administrators can inspect container environments and remain inside the host trust
  boundary;
- externally managed PostgreSQL backup/restore stays the database operator's responsibility;
- Windows ACL code and packages require Windows CI plus qualified security review;
- actual released-image installation, public ACME issuance/renewal, served-certificate expiry,
  relay/WebSocket flow, and Windows installer execution require platform/system tests;
- package-manager registration, service auto-start, and removal are packaging responsibilities,
  not self-modification by the running binary.

## Rollout

1. Land the launcher package, schemas, tests, ADR, and operator example.
2. Build unsigned Linux x86-64 and Windows x64 binaries in the release pipeline.
3. Run Windows ACL tests as an unprivileged and administrative user and inspect the resulting DACL.
4. Run disposable bundled and external-database installs using actual release images.
5. Exercise interrupted checkpoints, backup/restore, preserve-data uninstall, bad Traefik update,
   ACME renewal, and public HTTP/WebSocket/relay traffic.
6. Promote the launcher only after those release gates and qualified review of the Windows ACL,
   update, migration, and destructive-operation boundaries.

## Rollback

Before migration starts, restore the recorded configuration, protected secrets, and previous image
digests, then require service health. After migration may have started, keep application services
stopped and restore the named verified backup. A Traefik-only update may restore its prior digest
while retaining ACME state.

Removing the launcher release does not remove Docker volumes or protected state. Operators can
retain the prior launcher binary alongside its compatible configuration schema during rollout.
Never use down --volumes as an ordinary application rollback.
