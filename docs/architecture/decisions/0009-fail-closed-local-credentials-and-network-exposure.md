# ADR-0009: Fail-closed local credentials and network exposure

- Status: accepted
- Date: 2026-08-19
- Owners: Talos maintainers

## Context

The committed environment example necessarily contains recognizable credential markers. The API
previously accepted those markers as real JWT and service credentials, and the Windows development
setup replaced them with the same fixed values in every checkout. A caller who knew the published
`SERVICE_KEY` could use the service minting route to obtain an agent-scoped machine JWT. Auth routes
also ignored `TOKEN_TTL` and `MACHINE_TOKEN_TTL`, issuing seven-day user tokens and one-year machine
tokens despite the documented one-hour and 30-day configuration.

The API also treated `JWT_SECRET` as an implicit fallback for application-data encryption. That
coupled two different cryptographic purposes, made JWT rotation capable of rendering encrypted data
unreadable, and let the encryption boundary start without its own persistent key. Native debug
preflight validated only API token settings even though it also started the RMM server, telemetry
consumer/producer, and AI runner, each with separate service credentials.

Compose published the API, PostgreSQL, Kafka-compatible broker, schema registry, console, Azurite,
RMM control server, telemetry producer, and AI runner on every host interface. Several are
development services with no independent network authentication. The API also unconditionally
trusted one forwarding-proxy hop, so a directly reachable client could influence the address used
by Express rate limiting through `X-Forwarded-For`.
Some audit and installer code bypassed that policy by reading `X-Forwarded-For`,
`X-Forwarded-Proto`, or `X-Forwarded-Host` directly. Host-native Rust listener defaults also bound
to every interface in code even though the documented examples and Compose host publication were
loopback-oriented.

## Options considered

### Document that examples are unsafe

Documentation preserves compatibility but does not prevent an accidental copy-and-run deployment.
The fixed values are public facts and cannot safely authorize any process.

### Silently replace examples during every startup

Automatic rotation would break already-running service peers, invalidate JWTs on restart, and make
credential backup and deployment reproducibility ambiguous.

### Reject examples and make generation an explicit installation step

Startup can fail before opening a network service or mutating Community infrastructure. A setup
tool may generate credentials once when it creates the ignored local environment file, while an
existing operator-owned file remains untouched unless replacement is explicitly requested.

### Retain broad binds and rely on host firewalls

This is convenient for remote testing but makes the absence or drift of an external firewall a
direct exposure. Local development and a single-host Community topology do not require broad host
publication.

## Decision

One TypeScript policy owns the credential markers that have appeared in public examples or the old
Windows bootstrap. API module initialization rejects any configured credential variable containing
one of those values, including optional service credentials. It also continues to require
`JWT_SECRET`, `APP_ENCRYPTION_KEY`, `TOKEN_TTL`, and `MACHINE_TOKEN_TTL`.
`APP_ENCRYPTION_KEY` is mandatory, must differ from `JWT_SECRET`, and is the only input to
application-data key derivation. The Community launcher applies the same policy and additionally
requires `RMM_SERVER_API_KEY` before certificate validation, PostgreSQL startup, or migration. Full
development applies the same fail-closed check. Native debug additionally requires the general API
service key, RMM-server key, telemetry service key, and AI-runner service key before starting any
infrastructure. Diagnostics name variables but never credential values.

The service machine-token route retains a second defensive example-value check and is disabled when
`SERVICE_KEY` is absent. Registration and login call the shared user-token signer; both user- and
service-initiated machine-token routes call the shared machine-token signer. Consequently,
`TOKEN_TTL` and `MACHINE_TOKEN_TTL` are the sole configured duration authorities.

Windows development setup generates independent values for JWT signing, persistent application
encryption, RMM-server, general service, telemetry, AI-runner, and agent boundaries using .NET's
cryptographic random-number generator. It writes them only while creating or explicitly replacing
ignored `apps/.env`; it does not rotate an existing file implicitly. Native debug mode preserves
every valid operator value. For a missing required credential or a configured public example
marker, it generates a cryptographic local replacement once and stores it in the ignored,
owner-readable `apps/.env.debug.local` file. The file is reused across debug restarts, so its
debug-only `APP_ENCRYPTION_KEY` is persistent rather than silently rotated. Diagnostics disclose
only repaired variable names, never their values. Optional provider placeholders are unset instead
of being converted into fake provider credentials.

Host-native API and Rust example listeners default to loopback. Every Compose `ports` entry binds
to `127.0.0.1` by default, while container listeners remain available on the private Compose
network. Each published service has a named host-bind override for deliberate remote testing.
Redpanda has a separate advertised-host override because binding and client discovery are different
decisions.

Express trusts no forwarding proxy by default. Audit addresses and request-derived installer
origins use only Express's proxy-policy-aware `req.ip`, `req.protocol`, and `req.hostname`
interpretation; application code never reads forwarding headers directly. Explicit configured
HTTP(S) public URLs take precedence and reject credentials, query strings, and fragments.
`API_TRUSTED_PROXIES` may contain only an explicit
comma-separated allowlist of IP addresses, CIDRs, or Express's `loopback`, `linklocal`, and
`uniquelocal` named ranges. Boolean blanket trust, hostnames, wildcards, and hop counts are rejected.

## Security boundary and limitations

The marker list prevents reuse of credentials already disclosed by this repository; it is not an
entropy estimator, rotation system, or secret manager. Operators remain responsible for generating,
delivering, backing up, and rotating strong deployment credentials. A locally generated `.env`
still concentrates sensitive configuration in one ignored file, and the API remains a high-impact
control-plane process.

`APP_ENCRYPTION_KEY` is persistent key material. Losing or changing it without a data migration
prevents decryption of existing protected values. It must be backed up and rotated separately from
JWT-signing keys.

Loopback publication protects against remote hosts, not malicious processes on the same machine.
The development PostgreSQL username/password and emulator key remain documented defaults because
their services are loopback-only; production must replace them. Changing a `*_HOST_BIND` value to a
non-loopback address is an explicit expansion of the deployment trust boundary and requires
firewall, TLS, authentication, origin, and public-endpoint review.

An explicit proxy allowlist delegates client-address interpretation to those network peers. A proxy
must overwrite untrusted forwarding headers, and its actual source address must match the allowlist.
Rate limiting is an abuse-control layer, not a replacement for authentication.
Non-loopback or reverse-proxy deployments must configure their public API/frontend base URLs because
the backend cannot infer an external port or path safely from its private socket.

## Consequences

Positive:

- copying the example file without replacing credentials fails before API or Community startup;
- JWT signing and persistent data encryption have independent required keys;
- a repository-known service key cannot authorize machine-token minting;
- documented token lifetimes and issued token lifetimes agree;
- default local infrastructure is not remotely reachable;
- direct API exposure cannot spoof rate-limit identity through forwarding headers by default;
- audit and installer code cannot bypass the configured proxy trust policy;
- Windows checkouts no longer share credentials.

Costs and risks:

- existing checkouts that retained an old fixed value must generate a new one before starting;
- remote agent/viewer and multi-host development require explicit bind and endpoint configuration;
- installations behind a reverse proxy must configure its exact source range;
- changing JWT or service credentials requires coordinated peer rollout and may invalidate sessions;
- changing the application encryption key requires an explicit protected-data migration.

## Rollout

1. Generate unique credentials, store `APP_ENCRYPTION_KEY` durably, and update all peers before
   deploying the startup policy.
2. Keep host binds on loopback, then expose only endpoints required by the deployment.
3. If a reverse proxy is used, verify it overwrites forwarding headers and configure its narrowest
   stable address/CIDR in `API_TRUSTED_PROXIES`.
4. Confirm user and machine JWT `exp - iat` values match the configured TTLs.
5. Exercise Community config validation before starting or migrating a persistent environment.

## Rollback

Do not roll back to public example credentials or blanket proxy trust. A compatibility rollback may
temporarily restore a required listener bind while retaining loopback host publication, or remove an
incorrect proxy allowlist to return to direct-client addressing. Restore service availability with
new coordinated credentials rather than weakening the startup rejection policy.
