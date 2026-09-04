# ADR-0011: Traefik Community edge and ACME ownership

- Status: accepted
- Date: 2026-08-28
- Owners: Talos maintainers

## Context

Talos Community Edition exposes a web frontend, an HTTP API, the RMM control/WebSocket service, and
a raw TLS relay. A straightforward self-hosted installation must place these endpoints behind one
public IP address, acquire and renew publicly trusted certificates, and avoid requiring operators to
edit several application and proxy configurations by hand.

The development Compose topology publishes service ports directly and makes the relay terminate TLS
from operator-provided files. That is useful for development but is not the supported public edge.
Mounting Traefik's unrestricted Docker socket would also give an internet-facing proxy control over
the container daemon. Sharing Traefik's ACME account file or extracted private keys with the relay
would unnecessarily expand the certificate trust boundary.

The repository normally requires immutable release inputs. The owner has explicitly selected the
Traefik publisher's moving `latest` image for new installations to minimize routine version
maintenance and accepts the compatibility risk. This exception must remain limited to Traefik.

## Options considered

### Publish every service directly

Rejected. It expands the public attack surface, requires several public ports and certificates, and
duplicates proxy-trust policy across services.

### Let Traefik discover services through the Docker socket

Rejected for the default deployment. The file provider can express Talos's small, static topology
without granting the edge container Docker-daemon authority.

### Pass relay TLS through to `talos_relay`

This preserves end-to-end TLS to the relay process but requires a second certificate issuance path
or securely exporting ACME key material into another container. It is retained as a possible
advanced operator configuration, not the default.

### Terminate public TLS at Traefik

Selected. `talos_relay` already supports `RMM_RELAY_TLS_TERMINATED=true` and can receive plaintext
only on the private Compose network. No relay port is published on the host.

## Decision

The production Community deployment has one Traefik edge service attached to the internal
`talos_edge` network. Traefik uses reviewed static and dynamic files and does not mount the Docker
socket. Only its public entrypoints are published.

The default IPv4 network is `172.31.240.0/24`, with Traefik fixed at `172.31.240.2`; the API trusts
only that address. The launcher may select a non-overlapping private subnet and usable proxy address
as one validated pair. It must not widen proxy trust to a named private range or the full edge
subnet. Public host ports default to TCP 80/443, while application container ports remain private.

The default remote topology derives dedicated frontend, API, RMM-control, and relay DNS names from
operator configuration. HTTP routers forward the web, API, and RMM HTTP/WebSocket hosts. A TCP
router matches the relay hostname using SNI, terminates TLS, and forwards the decrypted stream to
the relay's private port. Traefik overwrites forwarding metadata, and the API trusts only the
private-network proxy range selected by the launcher.

Traefik owns ACME account registration, challenge handling, certificate storage, and renewal. Its
ACME state lives in a dedicated persistent volume with restrictive access and is included in backup
and restore procedures. HTTP-01 on the port-80 entrypoint is the default challenge, with Let's
Encrypt production and staging directories selected explicitly. Custom-certificate and local-only
modes are separate configurations with no ACME resolver or state mount; they do not weaken the
public default.

Dynamic routes use quoted environment-derived host matchers, strict SNI, TLS 1.2 or newer, and no
catch-all Host, SNI, or path rule. Traefik deletes aliasing request-header names, does not trust
internet forwarding headers, rejects encoded path delimiters/control characters, and does not
expose its API or dashboard. Unknown names therefore do not reach an application service.

New installs and explicit `talos-server update` operations resolve the official `traefik:latest`
tag. The launcher records the returned immutable digest, image-reported version, resolution time,
and previous known-good digest. Ordinary start/restart operations reuse the recorded digest rather
than silently resolving the moving tag. An update promotes the new digest only after routing,
certificate, and Talos health checks succeed; otherwise it rolls back to the previous digest. This
is a compensating control, not a claim that the input is reproducible.

## Trust boundary and failure behavior

- Traefik is internet-facing and can observe public connection metadata and terminated plaintext.
- A compromised Traefik can intercept Talos traffic but cannot control Docker through a mounted
  daemon socket or read unrelated application/database secrets.
- The relay accepts plaintext only from the private Compose network and is never host-published in
  this topology.
- ACME state contains private key material. Loss can cause reissuance/rate-limit pressure; theft
  permits endpoint impersonation until certificates are revoked or expire.
- DNS, NAT, challenge reachability, renewal, and certificate-expiry failures must be visible through
  launcher status and diagnostics.
- Unknown hostnames/SNI fail closed and the Traefik dashboard is not publicly enabled.

## Consequences

Positive:

- one public IP can serve all supported Talos endpoints;
- operators receive automatic public certificate issuance and renewal;
- application containers do not receive ACME private keys;
- the edge does not receive Docker-daemon authority;
- an explicit update has a known rollback image.

Costs and risks:

- fresh installs are not reproducible with respect to Traefik because `latest` can move;
- upstream changes can break new installs without a Talos repository change;
- TLS is plaintext between Traefik and the relay on a private network;
- multiple DNS records and correct public reachability are required for remote mode;
- the project must keep a public exception issue and periodically verify the compensating controls.

## Rollout

1. Add the production Compose and internal network contract.
2. Add file-provider routes, ACME state, staging support, and custom/local modes.
3. Add launcher resolution, digest recording, health promotion, and rollback.
4. Verify HTTP, WebSocket, and relay traffic plus certificate renewal on a disposable public test
   deployment.
5. Convert dependency-risk key DR-012 to a public issue before the Community release.

## Rollback

Roll back an unsuccessful proxy update to the recorded previous digest and retain the same ACME
volume. If the edge design itself must be removed, operators may explicitly configure an external
reverse proxy and operator-managed relay certificate; do not restore public host ports for every
internal service as the default.
