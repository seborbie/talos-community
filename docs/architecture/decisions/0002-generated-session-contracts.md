# ADR-0002: Generate viewer session contracts from Rust

- Status: accepted
- Date: 2026-08-17
- Owners: Talos maintainers

## Context

Interactive-session payloads are serialized by `talos_protocol` and returned by `talos_server`,
but the Tauri viewer independently maintained equivalent TypeScript shapes inside its oversized
`App.svelte`. Field renames, optionality, platform variants, and numeric representations could
therefore drift without either language reporting an error.

The protocol crate is already the shared Rust source for the server, worker, and viewer-native
code. It is the narrowest existing authority for these wire types.

## Options considered

### Continue handwritten TypeScript types

This has no generator cost, but preserves two sources of truth and relies on reviewers noticing
cross-language drift.

### Make an OpenAPI document authoritative for every RMM endpoint immediately

OpenAPI remains the intended source for the complete public HTTP surface. Converting the entire
control API in one change would combine contract design, handler changes, client generation, and
compatibility work across a large uncharacterized surface.

### Generate the live-session TypeScript slice from Rust

The existing Serde types remain authoritative for the currently migrated session-capability
responses. `ts-rs` derives TypeScript declarations using those same Serde renames and optionality
rules. This creates an enforceable seam now while broader OpenAPI adoption proceeds incrementally.

## Decision

Talos generates the following TypeScript contract slice from `talos_protocol`:

- agent platform and feature capabilities;
- local and reflex network addresses;
- remote-desktop display profiles;
- remote-desktop, shell, file-transfer, registry, and chat capability responses.

The generated package is `apps/talos_protocol_types` (`@talos/protocol-types`). The Tauri viewer
imports these types instead of declaring local copies. Generation uses the exactly pinned
`ts-rs` 12.0.1 crate and writes `src/generated.ts` atomically.

`bun run contracts:generate` updates the file. `bun run contracts:check` performs a read-only drift
check and is required locally and in CI. Generated files must never be hand-edited.

## Consequences

Positive:

- Serde field names and TypeScript field names now come from one declaration;
- Rust contract changes fail CI until the generated artifact and viewer agree;
- the viewer god file loses duplicated protocol declarations;
- additional protocol types can migrate incrementally.

Costs and limitations:

- `ts-rs` is a build-time dependency and raises the generator's MSRV to 1.88 (below the pinned
  Talos Rust 1.95 toolchain);
- this slice generates compile-time types, not runtime validation;
- HTTP paths, status codes, authentication, and error responses still require an OpenAPI contract;
- JavaScript cannot safely represent every Rust `u64`; the file-transfer byte threshold is
  intentionally generated as `number` because the existing JSON/viewer protocol already uses a
  JavaScript number and its configured values are bounded well below `Number.MAX_SAFE_INTEGER`.

## Rollout

1. Add conditional TypeScript derives to the shared Rust types.
2. Generate and publish the private workspace package.
3. Replace the viewer's handwritten session-capability types.
4. Enforce drift checks in the aggregate quality command and CI.
5. Migrate additional high-churn boundaries and add runtime/OpenAPI validation in later slices.

## Rollback

Restore the viewer-local declarations, remove its workspace dependency, and remove the generator
feature. No wire format or application data migration is involved.

## References

- [`ts-rs` official crate documentation](https://docs.rs/ts-rs/12.0.1/ts_rs/)
- [Serde field attributes](https://serde.rs/field-attrs.html)
- [OpenAPI Specification](https://spec.openapis.org/oas/latest.html)
