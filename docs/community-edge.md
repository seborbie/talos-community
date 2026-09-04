# Talos Community edge, DNS, and TLS

Talos Community uses Traefik as its only supported public ingress. Traefik accepts HTTP, HTTPS,
WebSocket, and relay TLS traffic on one host, obtains or loads certificates, and reaches the four
Talos application services only through the private `talos_edge` Compose network. It does not use
Docker discovery, mount the Docker socket, or expose its dashboard.

This is a single-host design. Do not run two Traefik replicas against the same ACME state.

## Traffic and trust boundaries

| Public name and protocol                            | Traefik route                         | Private destination                      |
| --------------------------------------------------- | ------------------------------------- | ---------------------------------------- |
| `TALOS_FRONTEND_DOMAIN`, HTTPS                      | operator UI                           | `frontend:3000` over HTTP                |
| `TALOS_API_DOMAIN`, HTTPS                           | authentication and application API    | `api_backend:3001` over HTTP             |
| `TALOS_CONTROL_DOMAIN`, HTTPS/WSS                   | agent and interactive control         | `talos_server:17110` over HTTP/WebSocket |
| `TALOS_RELAY_DOMAIN`, raw TLS                       | TLS is terminated using the relay SNI | `talos_relay:443` as plaintext TCP       |
| the three HTTP names, port 80                       | known hosts redirect to HTTPS         | no application plaintext route           |
| any configured name, `/.well-known/acme-challenge/` | Traefik's internal HTTP-01 responder  | no application route                     |

TCP routers are evaluated before HTTP routers on the shared HTTPS entrypoint. Only the relay SNI
matches the TCP router. TLS options use `sniStrict: true`, and there is no catch-all Host, SNI, or
path router, so unknown names fail closed. The relay's decrypted hop never leaves `talos_edge`, and
the relay has no certificate/key mount or host port.

Traefik receives untrusted internet traffic. It deletes aliasing request-header names and does not
trust client-supplied forwarding metadata. It also rejects encoded path delimiters and control
characters instead of accepting Traefik's permissive defaults. The API trusts only Traefik's fixed
private address, `172.31.240.2` by default, rather than the whole Docker/private range. A client or
another container therefore cannot choose the address used for audit and rate limiting merely by
sending `X-Forwarded-For`.

The default network pair is:

```dotenv
TALOS_EDGE_SUBNET=172.31.240.0/24
TALOS_TRAEFIK_IPV4=172.31.240.2
```

The launcher must validate that both values are IPv4, the proxy address is a usable member of the
subnet, and the subnet does not overlap a host route or another Docker network. If it does overlap,
select a new private `/24` and a usable address together; never broaden `API_TRUSTED_PROXIES` to
work around a collision.

## Choose exactly one edge mode

| Mode                  | Compose overlay                              | Certificate owner               | Public by default      |
| --------------------- | -------------------------------------------- | ------------------------------- | ---------------------- |
| Public ACME (default) | `infra/compose.community-traefik.yml`        | Traefik + Let's Encrypt HTTP-01 | Yes                    |
| Custom certificate    | `infra/compose.community-traefik-custom.yml` | Operator                        | Yes                    |
| Local self-signed     | `infra/compose.community-traefik-local.yml`  | Operator/launcher               | No; IPv4 loopback only |

Never combine these three overlays. Every mode uses the file provider and the same explicit route
set. Custom and local modes do not initialize or mount ACME state.

## Domains, DNS, NAT, and firewalls

Choose four distinct DNS names. A typical deployment uses:

```dotenv
TALOS_FRONTEND_DOMAIN=talos.example.net
TALOS_API_DOMAIN=api.talos.example.net
TALOS_CONTROL_DOMAIN=control.talos.example.net
TALOS_RELAY_DOMAIN=relay.talos.example.net
```

Create an `A` record for every name pointing to the same public IPv4 address. Create `AAAA` records
only after Docker, the host firewall, the router/NAT layer, and the ISP path all accept IPv6 on the
same host; a broken `AAAA` record can break both users and ACME validation. The shipped overlay
publishes IPv4. A dual-stack deployment requires a reviewed IPv6 Compose/network override and an
external test from an IPv6-only client.

Forward inbound TCP 80 and 443 to the Talos host. Permit outbound DNS and HTTPS so Traefik can
contact the ACME directory and registry. Do not forward or permit public access to ports 3000,
3001, 17110, the relay container port, PostgreSQL, Docker, or Traefik's internal health entrypoint.
No UDP ingress is required for this relay path.

The host mappings are configurable:

```dotenv
TALOS_EDGE_BIND_ADDRESS=0.0.0.0
TALOS_EDGE_HTTP_PORT=80
TALOS_EDGE_HTTPS_PORT=443
```

Public ACME still requires Let's Encrypt to reach external TCP 80, and normal clients expect 443.
Nonstandard host ports are appropriate only when NAT maps external 80/443 to those ports, or for
custom/local deployments whose public URLs include the chosen port. The launcher should reject a
nonstandard public-ACME port unless that external mapping has been made explicit.

For split DNS, internal clients should resolve all four public names to the internal address that
reaches the same Traefik entrypoints. Do not point internal DNS directly at an application
container. Verify all four records from both an external network and every important internal DNS
view before enrollment.

## Public ACME mode

The default uses Let's Encrypt's production directory and HTTP-01 challenge. Add this to the
protected installation environment:

```dotenv
TALOS_ACME_EMAIL=hostmaster@example.net
TALOS_ACME_CA_SERVER=https://acme-v02.api.letsencrypt.org/directory
```

Use a monitored mailbox. Port 80 must remain reachable after installation because renewals use the
same challenge. Traefik creates `/acme/acme.json` with mode `0600` in the private,
Traefik-only `talos_traefik_acme` volume. That file contains account and certificate private keys:
encrypt its backups, restrict restore access, and never attach or paste it into diagnostics.

Before production, validate DNS/NAT on a disposable installation using Let's Encrypt staging:

```dotenv
TALOS_ACME_CA_SERVER=https://acme-staging-v02.api.letsencrypt.org/directory
```

Staging certificates are not publicly trusted. Use a separate disposable ACME volume, then return
to the production URL for the real installation. Repeatedly deleting production ACME state and
retrying can exhaust issuer rate limits.

The owner-approved DR-012 exception permits only the publisher's `traefik:latest` input. A new
install or explicit update resolves it, records the immutable registry digest, image-reported
version, resolution time, and previous known-good digest, and supplies the digest through
`TALOS_TRAEFIK_IMAGE`. Ordinary start/restart uses `pull_policy: missing` and the recorded digest;
it must not silently resolve `latest`. Promote a new digest only after edge, TLS, and Talos health
checks pass. On failure, restore the previous digest while retaining the same ACME volume.

## Custom-certificate mode

Provide absolute host paths to one PEM full chain and its matching unencrypted PEM private key:

```dotenv
TALOS_CUSTOM_TLS_CERT_PATH=/etc/talos/certs/talos-fullchain.pem
TALOS_CUSTOM_TLS_KEY_PATH=/etc/talos/certs/talos-key.pem
```

The certificate must cover all four configured names and include required intermediates after the
leaf certificate. Restrict the key to the installation administrator and Traefik, renew it before
expiry, and reload/restart Traefik after atomically replacing both files. Do not store the key in
the source tree or environment file. Custom mode does not fall back to ACME.

Before replacing a certificate, verify that its public key matches the key, every SAN is present,
the chain is complete, and the validity window is current. Retain the previous pair for rollback in
a protected location. A failed reload keeps the prior in-memory certificate only until Traefik is
restarted, so treat parser/key errors as urgent.

## Local self-signed mode

Local mode binds only `127.0.0.1`, defaults to the four `*.talos.localhost` names, and never
contacts ACME. Generate a short-lived certificate outside the repository:

```sh
umask 077
mkdir -p /etc/talos/local-certs
openssl req -x509 -newkey rsa:3072 -sha256 -days 30 -nodes \
  -keyout /etc/talos/local-certs/talos-key.pem \
  -out /etc/talos/local-certs/talos-fullchain.pem \
  -subj /CN=talos.localhost \
  -addext 'subjectAltName=DNS:talos.localhost,DNS:api.talos.localhost,DNS:control.talos.localhost,DNS:relay.talos.localhost'
chmod 600 /etc/talos/local-certs/talos-key.pem
```

Then configure:

```dotenv
TALOS_LOCAL_TLS_CERT_PATH=/etc/talos/local-certs/talos-fullchain.pem
TALOS_LOCAL_TLS_KEY_PATH=/etc/talos/local-certs/talos-key.pem
```

Verify that all four names resolve to `127.0.0.1` on the local machine. Import the certificate (or
a deliberately created local CA) only into test trust stores that need to connect. Never disable
TLS verification in Talos, and never change the local overlay to `0.0.0.0`; use public ACME or
custom mode for remote access.

If local ports differ from 80/443, set both port variables and include the HTTPS port in every
`TALOS_PUBLIC_*` URL, the WSS control URL, and the relay address.

## Compose file lists

For the default bundled database plus public ACME edge:

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

For external PostgreSQL, omit only `compose.community-postgres.yml`. For another edge mode,
replace only the Traefik overlay. Reuse the exact same file list for `ps`, `logs`, `restart`, and
`down`; never use `down --volumes` during routine operation or update.

The protected application URLs must agree with the four domains:

```dotenv
TALOS_PUBLIC_FRONTEND_URL=https://talos.example.net
TALOS_PUBLIC_API_URL=https://api.talos.example.net
TALOS_PUBLIC_RMM_API_URL=https://control.talos.example.net
TALOS_AGENT_SERVER_URL=wss://control.talos.example.net/agent/ws
TALOS_PUBLIC_RELAY_ADDRESS=relay.talos.example.net:443
```

## Health, renewal, backup, and recovery

`docker compose ... ps` must show Traefik and all four application services healthy. The health
endpoint is private and the dashboard/API remain disabled. From a network outside the deployment,
test all three HTTPS names, an authenticated WebSocket/control operation, and a relay session. Also
test an unknown HTTP Host and unknown TLS SNI; they must not reach an application.

Monitor the served certificate on all four names at least daily and alert at 30, 14, and 7 days
before expiry. Treat these structured Traefik log events as actionable: ACME account/permission
errors, challenge failure, renewal failure, a nonexistent resolver, dynamic-file parse failure, or
backend routing failure. Status tooling must show the recorded Traefik digest/version, container
health, each hostname's served certificate expiry/issuer, ACME mode/directory, the last bounded
certificate error, and whether ports 80/443 are reachable without printing ACME or key material.

`talos-server backup` and `talos-server restore` address the stable
`talos-community_talos_traefik_acme` volume directly with an immutable, network-disabled helper
image; they do not require a Traefik container to exist. Backup quiesces a running Traefik first.
Restore stages the protected input inside the volume, sets mode `0600`, atomically replaces
`acme.json`, and verifies the resulting mode before Traefik starts. If operating Compose without the
launcher, use an independently reviewed digest-pinned backup tool while Traefik is stopped or use a
storage-consistent method, and restore with mode `0600`. If state is lost and no backup exists, stop
repeated restarts, confirm all DNS and challenge paths once, create fresh private state, and allow
one controlled reissuance. Check issuer rate-limit guidance before retrying.

Certificate issuance and renewal cannot be proven by repository tests because they require real
public DNS, routable ports, and an external CA. The release gate is a disposable public staging
deployment followed by one production issuance, a forced renewal rehearsal before expiry, external
HTTP/HTTPS/WebSocket/relay tests, forwarding-header spoof tests, and a bad-image rollback that
retains ACME state.

## Troubleshooting order

1. Confirm the same Compose file list and protected environment were used for `config`, `up`, and
   diagnostics; never paste rendered configuration because it contains secrets.
2. Resolve all four names externally and internally, and remove unreachable `AAAA` records.
3. Confirm TCP 80/443 reach Traefik and no other process or forwarding rule owns them.
4. Inspect bounded Traefik logs for file-provider, ACME, permission, and backend errors.
5. Check served SNI, chain, SAN, issuer, and expiry from outside the network.
6. Check private service health without publishing its port.
7. If an explicit Traefik update caused the failure, roll back to the recorded known-good digest
   without deleting or replacing the ACME volume.

The architecture and accepted floating-image exception are recorded in
[ADR-0011](architecture/decisions/0011-traefik-community-edge-and-acme.md) and
[DR-012](architecture/dependency-risk-register.md#dr-012-exception-evidence).
