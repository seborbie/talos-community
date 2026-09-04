# RMM Protocol Versioning

Golden fixtures live under `fixtures/current` and `fixtures/old`. Every protocol change that touches agent, server, viewer, or framed stream wire data should update or add fixtures before the change ships.

## Compatibility Rules

- Additive JSON fields must be optional, have `#[serde(default)]`, or be represented as `Option<T>`.
- Do not remove, rename, or make an optional field required without a human-approved revision or major version bump.
- Struct payloads ignore unknown fields by default. Keep this behavior unless strict validation is explicitly required at the boundary.
- Enum payloads reject unknown variants unless the enum has an explicit fallback. `OperationErrorCode` maps unknown values to `Unknown`; most command/request enums intentionally remain strict because an unknown operation cannot be safely executed.
- Prefer string capability/profile identifiers for negotiated features when older peers can safely ignore a new option.
- For binary frames, append new message types instead of changing existing frame layout. If a payload layout must change, add a new message type and keep the old parser path while supported.

## Fixture Policy

- Current fixtures must match exactly what current code serializes.
- Old fixtures should cover missing optional fields, ignored unknown fields, and any explicit enum fallback behavior.
- Keep fixtures small and deterministic; avoid real secrets, certificates, hostnames, or raw snapshot blobs.
- Reviewers should be able to understand a fixture without running a formatter or generator.

## Version Bumps

- Ordinary additive protocol work should bump the minor component for each touched shipped crate.
- Wire-incompatible changes require human approval for a revision or major bump before implementation is finalized.
- When a new peer must refuse older/newer peers, add an explicit protocol version field and a fixture for the refusal behavior.
