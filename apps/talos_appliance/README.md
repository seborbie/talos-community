# Talos Community server launcher

talos-server is the native Linux x86-64 and Windows x64 appliance launcher. It embeds the reviewed
production Compose layers, so a target host needs the released launcher binary, a protected JSON
request, Docker Engine, and Docker Compose v2. Bun and a source checkout are not runtime
prerequisites.

The initial binaries are unsigned. Verify the release checksums and provenance before execution
and expect Windows SmartScreen to warn until project-funded code signing is available.

## Install request

Start from [talos-server.example.json](talos-server.example.json), replace all four example registry
references with one Talos release's immutable digests, and set four distinct domains. For public
ACME, make DNS and inbound TCP 80/443 work before installation.

On Unix, protect the request before use:

    chmod 600 ./talos-server.json
    sudo ./talos-server install --config "$PWD/talos-server.json"

Windows applies an explicit protected DACL to secret-bearing launcher state. ACL application
failure aborts install/update; it never falls back to inherited permissions. Run the launcher from
an elevated terminal that can administer Docker:

    .\talos-server.exe install --config C:\protected\talos-server.json

Defaults:

- Linux state: /var/lib/talos-server
- Windows state: %ProgramData%\Talos\Server
- backup directory: state-directory/backups

Use --state-dir with an absolute path before the command to select another protected installation.
Use --docker with an absolute path only when automatic Docker CLI discovery is unsuitable. The
launcher detects prerequisites but never installs a privileged container runtime.

## Commands

    talos-server install --config request.json [--external-database-backup file]
    talos-server start
    talos-server stop
    talos-server status
    talos-server update --config request.json [--external-database-backup file]
    talos-server backup [--name name] [--external-database-backup file]
    talos-server restore backup-name --confirm installation-id [--external-database-restored]
    talos-server diagnostics [--name name]
    talos-server uninstall [--remove-data --confirm installation-id]

Status prints the installation ID needed for destructive restore/removal confirmations. Stop and
plain uninstall preserve protected state and Docker volumes. The --remove-data option removes the
selected Compose volumes and state root after marker, path, and symlink checks; it is not
recoverable without an off-host backup.

Public-ACME backup and restore address the stable `talos-community_talos_traefik_acme` named volume
directly, so they also work after `stop` has removed the Traefik container. The launcher uses an
immutable, network-disabled helper image with no other mount, stages restore data inside the volume,
sets mode `0600`, and atomically replaces `acme.json` before Traefik starts. A backup requested while
bundled PostgreSQL is stopped returns PostgreSQL to the stopped state even when backup creation
fails.

## Database modes

The example uses bundled PostgreSQL. The launcher creates an independent URL-safe password, starts
PostgreSQL before preflight/migrations, and includes a verified custom-format logical dump in every
backup.

For managed PostgreSQL, replace the database object with:

    {
      "mode": "external",
      "url": "postgresql://talos_app:percent-encoded-password@db.example.net:5432/talos?sslmode=verify-full&connect_timeout=5"
    }

The request is secret-bearing in this mode. The complete URL is moved to protected secret state and
never printed. The non-secret configuration stores only its canonical scheme, host, effective port,
and database so updates can rotate credentials without silently switching database targets.
Backup/update requires
--external-database-backup pointing to an already verified, owner-only provider backup. Restore
does not change that database; restore it to the intended target first, then pass
--external-database-restored.

## Edge modes

- public_acme: Traefik obtains and renews Let's Encrypt certificates. Ports remain 80/443.
- custom_certificate: set absolute certificate_path and private_key_path; the key must be
  owner-only.
- local: the launcher generates a protected 30-day self-signed certificate for all four local
  names and renews it during start before its final three days. The local overlay binds loopback.

Only install and explicit update resolve traefik:latest. The resolved official digest, reported
version, resolution time, and previous known-good deployment are durable. Start/restart reuses the
recorded digest. All Talos image inputs must already contain an sha256 digest.

## Recovery and diagnostics

An update first creates and verifies a backup. Failure before migration may restore the previous
configuration and image digests automatically. Once migration may have started, application
services stop and the recorded backup is required. Do not bypass that journal or mark a failed
Prisma migration resolved without reviewing the database.

Diagnostics are written below the configured backup directory. They contain bounded Compose
status/logs, versions, image digests, routing names, and disk capacity; exact secret values and
database URLs are removed. ACME/private-key state is never included. Served-certificate expiry
probing is not yet implemented, so retain external expiry monitoring described in
[the Community edge guide](../../docs/community-edge.md).

## Verification boundary

Run the focused package gates from apps:

    cargo test -p talos_appliance --locked
    cargo clippy -p talos_appliance --all-targets --locked -- -D warnings

The unit suite covers schema/URL/image validation, command construction, hostile input, redaction,
permissions, local certificate persistence, Compose layer selection, backup integrity, operation
transitions, rollback decisions, and destructive path guards.

Clean-host released-image installation, actual external PostgreSQL, public ACME
issuance/renewal/expiry, real relay/WebSocket traffic, restore into disposable infrastructure,
Windows DACL inspection, Windows installer execution, and service auto-start remain release gates.
They are not claimed by the local unit suite.
