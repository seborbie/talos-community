# ADR-0018: Coordinate native Tauri JavaScript and Rust updates

- Status: proposed; awaiting integration and qualified release review
- Date: 2026-10-05
- Owner: Talos maintainers

## Context

Replacement dependency PR #51 updates the three native APIs/CLIs to 2.12.1 and the viewer's
JavaScript deep-link plugin to 2.6.1. The shared Cargo lock still resolved Tauri 2.11.2 and
deep-link 2.4.6. Upstream native builds require matching JavaScript/Rust major and minor
versions; successful frontend asset builds alone do not establish that compatibility. Three
new workspace regression cases fail on the proposed graph.

Tauri's preflight discovers JavaScript packages through npm. In this isolated Bun workspace,
`npm list` returns the root package without installed Tauri entries, so that discovery does
not provide complete mismatch coverage. An isolated CLI experiment stopped at a deliberately
failing compiler runner, not at a version diagnostic; it did not build or sign an installer.

The previous #49 repair remains scoped to CLI 2.11.5. The replacement group must align its new
release script/contract pin without changing the prerequisite PR. Dependabot's deletion of the
old #47 branch also closed its Vite dependent #43, so recovery must preserve both changes as
separate reviewable proposals.

## Options considered

1. Retain the mismatched Rust graph and rely on frontend CI. This leaves the native packaging
   boundary unverified and contradicts upstream's compatibility rule.
2. Defer the Tauri 2.12 and plugin updates while landing unrelated group updates. This is the
   rollback path if coordinated native validation fails.
3. Update the native JavaScript/Rust counterparts together and directly verify their locked
   versions, while preserving the existing security and release repair ancestry.

## Decision

Keep the replacement's exact native API/CLI 2.12.1 pins. Raise all three native Tauri manifest
minimums, including viewer test features, to 2.12.1 and the viewer deep-link minimum to 2.6.1.
Generate Cargo.lock through targeted Cargo updates. Move the macOS viewer script and release
contract together to exact CLI 2.12.1; retain frozen installs and Cargo `--locked`.

Wry 0.57 returns WebView2 COM 0.39 / Windows core 0.62 types. Align the viewer's direct
WebView2 and Windows core dependencies to that graph so its accelerator-key settings cast
uses the same COM identities. Retain the separate Windows 0.61 Direct3D/Media Foundation
bindings, whose modules use their own matching trait imports. A compile-only Windows fixture
reproduced the old graph's missing `cast` method before alignment.

Add workspace regressions requiring matching major/minor versions for each native API and
installed JavaScript/Rust plugin pair. Require one reviewed workspace Rust release for those
pairs, and matching viewer/Wry WebView2 and Windows core crate identities. The unused JavaScript
shortcut package does not add a new native plugin or capability.
Keep the Vite 8 migration in a separate recovered draft.

## Consequences and compatibility

The native framework and deep-link dependencies change, including upstream macOS behavior.
Both published Rust releases require Rust 1.90, below Talos's pinned 1.95.0, and retain their
reviewed Apache-2.0 OR MIT licensing. Generated dependency changes must pass licence/source
policy, current advisory checks, full platform Clippy/tests and frontend build gates.

No first-party authorization, protocol, signing identity, credentials, endpoint or updater
policy changes. Native signing, installer/update/rollback execution and independent qualified
AI or human review under [the review policy](../../review-policy.md) are still required before a
supported release. Tauri 2.12 remains on the GTK/GLib 0.18 graph;
this decision does not resolve or suppress the tracked Linux GLib finding.

## Rollout

Recover the shared #48/#49/#47 ancestry in a maintainer-owned branch, verify the new regression
failures before the native alignment, and run all applicable gates on the published head.
Stack the replacement group after #49 and the recovered Vite proposal after that group.
Retarget dependents before deleting any merged prerequisite branch. Obtain qualified release
review, re-review the final main-based diff, and refresh CI before integration. No release is
authorized by this ADR.

## Rollback

Restore the prior JavaScript API/plugin/CLI pins, Rust manifest minimums, Bun/Cargo locks and
script/contract pins together. Keep the compatibility regression: the 2.11 API and 2.4 deep-link
graph must continue to match its Rust counterparts. Do not add
`--ignore-version-mismatches`, widen version ranges or suppress advisory checks as a rollback.

## References

- [Upstream native mismatch rule](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-cli/src/info/plugins.rs)
- [Upstream build preflight](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-cli/src/build.rs)
- [Tauri CLI 2.12.1](https://github.com/tauri-apps/tauri/releases/tag/tauri-cli-v2.12.1)
- [Tauri 2.12.1 metadata](https://crates.io/api/v1/crates/tauri/2.12.1)
- [Deep-link 2.6.1 metadata](https://crates.io/api/v1/crates/tauri-plugin-deep-link/2.6.1)
