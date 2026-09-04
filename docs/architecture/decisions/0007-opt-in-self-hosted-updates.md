# ADR-0007: Opt-in self-hosted update endpoints

- Status: accepted
- Date: 2026-08-17
- Owners: Talos maintainers

## Context

The endpoint supervisor and desktop viewer contained a development Talos Cloud update URL as a
compiled fallback. Linux and macOS example/package configuration repeated that endpoint. A fresh
Community Edition install could therefore contact infrastructure outside the operator's deployment
without the operator configuring updates.

The viewer already persists the API base carried by an `rmm:` session link so later update checks
can use the same control plane. The updater wrote that file but never read it, and instead fell back
to the compiled development endpoint.

Updates are a security-sensitive remote call and installation boundary. Removing the fallback must
not weaken the existing embedded manifest trust key, signed-manifest verification, package digest
verification, or URL normalization.

## Options considered

### Keep a hosted fallback and document it

This preserves automatic updates for hosted development but makes an external request the default
behavior of a self-hosted binary. Documentation does not provide operator consent.

### Compile separate hosted and Community binaries

Different defaults could be selected at build time, but separate binaries increase release and
verification drift. A Community artifact could still be built with the wrong default.

### Disable update requests until an endpoint is configured

One binary can serve hosted and self-hosted deployments. Operators explicitly configure their own
endpoint, and hosted packaging can supply its endpoint through deployment configuration rather than
source code.

## Decision

Talos has no compiled update endpoint and makes no update request when none is configured.

The supervisor resolves the first valid value from its `--update-base-url` argument,
`RMM_UPDATE_BASE_URL`, `API_BACKEND_URL`, or `INTERNAL_API_URL`. Its worker watchdog continues to
operate when updates are disabled. Linux and macOS example/package environment files omit
`RMM_UPDATE_BASE_URL` by default and explain how to opt in to a self-hosted update API.

The viewer resolves explicit environment configuration first and then reads the API base persisted
by `remember_update_api_base` from an `rmm:` session. Resolution happens for each check, so a session
opened after viewer startup can enable checks against that same self-hosted control plane. Without
either source, background and manual checks return without network access.

Both clients accept only absolute HTTP or HTTPS URLs with a host and without embedded credentials,
query, or fragment. They remove trailing slashes and append `/rmm/updates` exactly once. The update
service may still run over HTTP for explicitly configured private/local development; production
operators should use HTTPS.

Endpoint selection does not confer artifact trust. Both clients continue to verify every manifest
with the embedded public key before accepting its version or downloading/applying its package. A
verified manifest must then match the requested product, platform, architecture, channel, ring,
install mode, and canonical package name. Downloads are bounded by the signed byte count, recheck
that count on disk, and verify the package SHA-256 before staging. The update API independently
rejects manifests placed in the wrong product/architecture slot or whose byte count differs from
the selected package.

## Trust boundary and failure behavior

Environment/CLI values and the viewer's session URL are configuration inputs, not proof that an
endpoint is trusted. Scheme, authority, credential, query, and fragment validation prevents local
file URLs and ambiguous/credential-bearing destinations. A configured endpoint can observe update
request metadata and can withhold or replay responses, but it cannot authorize an unsigned package.
The existing version comparison also rejects a signed version that is not newer.

An absent, blank, or invalid endpoint fails closed as disabled. The supervisor logs that state and
continues local service monitoring. The viewer reports no available update and can begin using a
later persisted session API base without restart.

## Consequences

Positive:

- fresh Community artifacts do not call a Talos-hosted development update service;
- hosted and self-hosted deployments use the same verified binary and an explicit endpoint;
- the viewer's persisted session API setting now affects the updater as intended;
- signed manifests and digest verification remain the artifact trust boundary.

Costs and limitations:

- a fresh Linux supervisor package cannot bootstrap its worker until the operator configures an
  update endpoint, because that installer currently packages the supervisor rather than a worker;
- operators are responsible for serving compatible signed manifests and packages;
- the persisted viewer endpoint follows the most recently recorded session API until explicit
  environment configuration overrides it.

## Rollout

1. Deploy the self-hosted update API and its packages signed by the key embedded in the clients.
2. Set `RMM_UPDATE_BASE_URL` in endpoint supervisor configuration before relying on worker bootstrap
   or automatic updates.
3. Open a viewer session whose `api` parameter names the self-hosted control plane, or set
   `RMM_VIEWER_UPDATE_BASE_URL` explicitly.
4. Confirm logs name the configured endpoint and that unsigned/wrong-key manifests remain rejected.

## Rollback

Reintroducing a compiled hosted fallback is not an acceptable Community rollback. Operators can
disable requests by removing the endpoint configuration. If this resolution order must be reverted,
restore the prior explicit configuration behavior while retaining a `None`/disabled final fallback
and all signature/digest verification.
