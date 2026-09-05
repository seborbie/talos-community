# ADR-0014: Retain generated Cargo metadata for Dependabot

- Status: proposed; local repair awaiting integration
- Date: 2026-09-04
- Owner: Talos maintainers

## Context

The public source snapshot excludes reconstructed vpx-encode source pending notice review. Cargo
Dependabot aborts before resolving dependencies because a workspace member's Cargo.toml is absent.
Running setup in normal CI cannot repair a manifest fetch in GitHub's separate updater.

## Decision

Retain only the exact generated Cargo.toml, including upstream authors and MIT metadata. Keep the
implementation and README excluded. Acquisition accepts this manifest-only initial state only when
its digest matches the existing policy, reconstructs the remaining files, and preserves the manifest
on failure. Complete trees still require the exact file set and every original digest. Git preserves
digest-bound manifest/patch bytes. A workspace gate traverses all Git-visible local Cargo manifests,
and export regression tests verify the updater will receive them. Routine Cargo updates include
transitive dependencies.

## Alternatives and consequences

Vendoring the implementation would expand redistribution scope; ignoring the missing dependency
would hide part of the dependency graph. A custom updater would duplicate hosted functionality and
introduce credentials and a new maintenance burden. Retaining metadata avoids these changes and does
not execute untrusted updater code or grant additional workflow permissions. Dependency changes to
the generated manifest still require patch/policy regeneration and review. Source alpha limitations
and notice review requirements remain in effect.

## Rollout and rollback

Prepare and test locally under the owner's main-only preference. Integrate through the repository's
existing review/check requirements, then verify the hosted Cargo updater. This does not clear existing
alerts or assert every hosted CI job passes. Reverting the repair restores the prior acquisition
behavior but also restores the known Dependabot fetch failure; retain manual triage if rolled back.
