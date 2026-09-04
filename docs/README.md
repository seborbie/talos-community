# Talos documentation

Talos Community Edition is alpha software. Start with a disposable local evaluation environment.

## Getting started

- [Development setup](development.md): prerequisites, credentials, database setup, native builds,
  and the full development stack.
- [Community Edition](community-edition.md): the smaller evaluation stack, included features,
  optional services, networking, and current limitations.
- [Relay certificates](../apps/certs/README.md): local TLS and certificates for remote endpoints.
- [Screenshots](screenshots/community-edition/README.md): examples from an alpha build.

## Deployment and operation

- [Community deployment](community-deployment.md): production topology, bundled or external
  PostgreSQL, migrations, backup, restore, and rollback. Read its verification limits before use.
- [DNS, edge routing, and TLS](community-edge.md).
- [Native server launcher](../apps/talos_appliance/README.md).
- [Endpoint worker](../apps/talos_worker/README.md) and [installer tooling](../apps/installer/README.md).
- [Binary trust, signing, and updates](release-signing.md).

## Understanding and contributing

- [Architecture](architecture/README.md), [state ownership](architecture/state-ownership.md),
  and [engineering quality](../ENGINEERING_QUALITY.md).
- [Contribution guide](../CONTRIBUTING.md), [code of conduct](../CODE_OF_CONDUCT.md),
  [support](../SUPPORT.md), and [security reporting](../SECURITY.md).
- [Licensing and provenance](licensing-and-provenance.md) and [third-party notices](../THIRD_PARTY_NOTICES.md).
- [Dependency maintenance](dependency-maintenance.md): coverage, generated manifests, and update checks.
- [Release readiness](open-source-readiness.md), [source export](public-source-export.md),
  and [release process](community-release-process.md).
