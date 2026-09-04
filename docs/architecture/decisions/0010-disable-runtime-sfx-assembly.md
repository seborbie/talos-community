# ADR-0010: Fail closed on runtime-assembled Windows executables

Status: accepted
Date: 2026-08-28

## Context

The installer API can concatenate a 7-Zip SFX stub, request-specific configuration containing an
enrollment token, and a prebuilt Burn archive. Every response is a different executable. Any
offline Authenticode signature on the stub or embedded bundle does not authenticate the resulting
outer executable, while signing each response would require online access to a code-signing key in
the API trust boundary.

That design conflicts with Community Edition's fail-closed defaults and with the repository rule
that release artifacts have reviewed, immutable inputs. The frontend also presented the generated
file as an ordinary Windows download without explaining that it was unsigned.

## Options considered

### Sign each scoped executable in the API

Rejected. It places release signing authority in an internet-facing service and turns an API
compromise into arbitrary trusted-code signing.

### Keep runtime assembly enabled and document the warning

Rejected. A warning does not protect operators who reasonably expect a downloaded executable to
retain the publisher signature of its source artifacts.

### Immediately delete the compatibility route

Not selected yet. Existing private development deployments may still depend on it while the
replacement bootstrap protocol is built.

### Default-disable runtime assembly and migrate to an immutable bootstrapper

Selected. It removes the unsafe default without pretending the final enrollment protocol already
exists.

## Decision

The request-time `/rmm/installers/profiles/:id/download-exe` route is guarded by
`RMM_ENABLE_UNSIGNED_SCOPED_INSTALLERS=true`. The default, including Community Edition, returns a
stable `UNSIGNED_SCOPED_INSTALLERS_DISABLED` error before issuing enrollment material or reading SFX
artifacts. The Community UI no longer calls or advertises that route.

Private development operators may opt in temporarily. Documentation must label the result as
unsigned and unsuitable for public distribution. No runtime service may receive an Authenticode
private key.

The target design is one immutable Burn bootstrapper produced in the reviewed release environment.
Initial official Community binaries may be intentionally Authenticode-unsigned when that state is
disclosed with checksums and provenance; a fork or future official release may sign the same
immutable bytes using owner-controlled infrastructure. The bootstrapper will redeem a high-entropy,
short-lived, single-use bootstrap code for scoped enrollment material over the operator's
authenticated API. Tokens must not be embedded in a modified executable or persisted in command
history longer than necessary.

## Consequences

- Community and default deployments cannot accidentally distribute runtime-mutated executables.
- Existing operators using the legacy route must explicitly acknowledge the behavior while they
  migrate.
- Issuing enrollment JSON and the platform-specific Linux/macOS flows remain available.
- Windows self-service installation is not publication-ready until code redemption and native
  Windows end-to-end verification are complete. Authenticode is a separate publisher-identity
  choice and does not authorize weakening the enrollment protocol.

## Rollout and rollback

1. Ship the default-disabled guard and remove the frontend action.
2. Inventory any private deployment that explicitly enables the compatibility flag.
3. Implement and threat-model the single-use bootstrap-code exchange, then add clean-Windows
   install, replay, expiry, revocation, and signature regressions.
4. Remove SFX runtime assembly and its flag after the compatibility window.

Rollback means temporarily setting the explicit flag in a controlled private environment. It does
not mean enabling runtime signing or treating the generated executable as a public release.
