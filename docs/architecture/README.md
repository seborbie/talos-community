# Talos architecture

This directory records the current architecture, intended boundaries, and consequential decisions.
It is descriptive, not a claim that every target state has already been implemented.

- [State ownership and scaling boundaries](state-ownership.md)
- [Dependency risk register](dependency-risk-register.md)

## Current system inventory

The repository contains:

- a Bun workspace for the Express API, SvelteKit frontend, and three Svelte/Tauri frontends;
- a Cargo workspace for the control server, endpoint worker, viewer/native helpers, relay, telemetry
  pipeline, AI runner, supervisors/updaters, protocol, collectors, and shared libraries;
- PostgreSQL, Redpanda, and object-storage dependencies in the full development profile;
- Windows, macOS, and Linux packaging and release tooling.

## Architectural direction

The top-five debt-remediation program, in implementation order, is:

1. Replace fragmented JavaScript package management with a strict Bun-native workspace and one
   repository-wide quality/build contract.
2. Establish tests and internal module seams around oversized entry points.
3. Establish machine-readable contracts and generated or mechanically verified consumers.
4. Separate process-local connection state from durable/shared control-plane state.
5. Simplify deployment boundaries, with a low-dependency Community Edition profile.

Changing process topology comes last because module seams, characterization tests, and contracts are
needed to make that change safely.

## Decisions

- [ADR-0001: Bun-native isolated workspace](decisions/0001-bun-isolated-workspace.md)
- [ADR-0002: Generate viewer session contracts from Rust](decisions/0002-generated-session-contracts.md)
- [ADR-0003: Keyed, partition-aware telemetry consumption](decisions/0003-keyed-partition-aware-telemetry.md)
- [ADR-0004: Layered production Community topology with selectable PostgreSQL](decisions/0004-community-edition-topology.md)
- [ADR-0005: PostgreSQL-owned generic remediation dispatch](decisions/0005-durable-remediation-dispatch.md)
- [ADR-0006: Cargo-owned Windows installer product versions](decisions/0006-cargo-owned-windows-installer-versions.md)
- [ADR-0007: Opt-in self-hosted update endpoints](decisions/0007-opt-in-self-hosted-updates.md)
- [ADR-0008: Explicit Community updater-manifest signing keys](decisions/0008-community-manifest-signing-keys.md)
- [ADR-0009: Fail-closed local credentials and network exposure](decisions/0009-fail-closed-local-credentials-and-network-exposure.md)
- [ADR-0010: Fail closed on runtime-assembled Windows executables](decisions/0010-disable-runtime-sfx-assembly.md)

New decisions that affect process boundaries, protocols, state ownership, security boundaries,
storage, or build/release architecture must be recorded here before implementation is considered
complete.
