# Dependency risk register

Last reviewed: 2026-08-19
Owner: Talos maintainers
Next review due: 2026-11-17

This register tracks dependency findings and narrow release-input exceptions that cannot currently
be removed without an upstream platform migration, an unsupported override, or a larger subsystem
replacement. JavaScript exceptions are named explicitly in `bun run audit:js`; every other Bun
advisory fails that command. Rust entries below are informational maintenance findings, not
vulnerability allowlists: `cargo audit` must report zero vulnerable crates. Findings remain visible
or narrowly identified so a fixed upstream release can be adopted promptly.

The `DR-*` entries below link to public tracking issues created for source publication on
2026-09-04. Existing review/expiry dates remain in force.

## Current baseline

| Tracking key | Dependency path                                                     | Finding                                                                                                                                                                                       | Exposure and current control                                                                                                                                                                                                                                                                                                                                                          | Exit condition                                                                                                                                                                                               |
| ------------ | ------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [DR-001](https://github.com/seborbie/talos-community/issues/2)       | Tauri/Wry Linux UI -> GTK3/GLib 0.18                                | RUSTSEC-2024-0411 through RUSTSEC-2024-0420, RUSTSEC-2024-0370, RUSTSEC-2024-0429                                                                                                             | These are Linux desktop UI/build dependencies selected by the current Tauri runtime. Network-facing Talos services do not depend on GTK. Keep Tauri/Wry current and test the supported desktop targets.                                                                                                                                                                               | Upgrade when Tauri offers a supported runtime without the unmaintained GTK3 bindings; review every Tauri update.                                                                                             |
| [DR-002](https://github.com/seborbie/talos-community/issues/3)       | Vendored Samsa -> rsasl -> core2; Samsa/Kafka -> rand 0.8           | RUSTSEC-2026-0105, RUSTSEC-2026-0097; `core2` is also yanked                                                                                                                                  | The telemetry consumer uses a pinned, locally patched Samsa because the upstream crate is no longer sufficient for Talos. The rand finding requires a custom logger implementation using `rand::rng`; Talos does not provide one. Kafka input remains authenticated and validated.                                                                                                    | Replace vendored Samsa/legacy Kafka dependencies with a maintained client, then remove `core2`, old rand, and this entry.                                                                                    |
| [DR-003](https://github.com/seborbie/talos-community/issues/4)       | `get_if_addrs` -> `get_if_addrs-sys` -> `gcc`                       | RUSTSEC-2025-0121                                                                                                                                                                             | The unmaintained crate is a build dependency for endpoint address discovery, not a runtime parser. Builds are pinned and exercised on supported targets.                                                                                                                                                                                                                              | Move address discovery to a maintained platform abstraction.                                                                                                                                                 |
| [DR-004](https://github.com/seborbie/talos-community/issues/5)       | `local-ip-address` -> `neli` -> `getset` -> `proc-macro-error2`     | RUSTSEC-2026-0173                                                                                                                                                                             | This is compile-time macro tooling behind local address discovery. Release inputs are pinned.                                                                                                                                                                                                                                                                                         | Upgrade or replace `local-ip-address` when its dependency graph removes the macro crate.                                                                                                                     |
| [DR-005](https://github.com/seborbie/talos-community/issues/6)       | Windows collector -> `registry`                                     | RUSTSEC-2025-0026                                                                                                                                                                             | The crate is used only on the Windows endpoint collector. Windows builds and collector behavior remain in the CI/release test matrix.                                                                                                                                                                                                                                                 | Replace registry access with a maintained Windows API crate.                                                                                                                                                 |
| [DR-006](https://github.com/seborbie/talos-community/issues/7)       | TLS PEM parsing in protocol/relay/worker/viewer and vendored Samsa  | RUSTSEC-2025-0134                                                                                                                                                                             | `rustls-pemfile` is unmaintained but has no reported vulnerability. Certificate inputs are bounded configuration/session material, and the active TLS stack is current.                                                                                                                                                                                                               | Migrate to `rustls-pki-types` PEM APIs and remove the dependency.                                                                                                                                            |
| [DR-007](https://github.com/seborbie/talos-community/issues/8)       | Tauri -> `urlpattern` -> `unic-*` 0.9                               | RUSTSEC-2025-0075, RUSTSEC-2025-0080, RUSTSEC-2025-0081, RUSTSEC-2025-0098, RUSTSEC-2025-0100                                                                                                 | These unmaintained Unicode tables are transitive build/runtime support inside Tauri URL-pattern matching. Talos does not parse authorization decisions from URL-pattern text.                                                                                                                                                                                                         | Adopt the first supported Tauri/urlpattern graph without `unic-*` 0.9.                                                                                                                                       |
| [DR-008](https://github.com/seborbie/talos-community/issues/9)       | Prisma CLI 6.19.3 -> `@prisma/config` -> `deepmerge-ts` 7.1.5       | GHSA-ggr8-5vv4-36mx (high)                                                                                                                                                                    | This package is reached through the development-only Prisma schema/generation toolchain, not the deployed API runtime. The latest compatible Prisma 6 release and current Prisma 7 still pin the affected version exactly; forcing the fixed major would violate the parent package's declared range. Keep Prisma generation out of request-time code and review every Prisma update. | Remove the audit exception as soon as Prisma publishes a compatible dependency graph containing a fixed `deepmerge-ts`; tracked by DR-008.                                                                   |
| [DR-009](https://github.com/seborbie/talos-community/issues/10)       | SvelteKit 2.70.2 -> `cookie` 0.6.0                                  | GHSA-pxg6-pf52-xh8x (low)                                                                                                                                                                     | The latest stable SvelteKit 2 line permits this version. The vulnerable behavior concerns cookie name/path/domain serialization; Talos application code currently makes no `cookies.set` or `cookies.delete` calls. Avoid adding those calls without re-evaluating this entry and keep SvelteKit current.                                                                             | Remove the audit exception when a stable compatible SvelteKit release resolves to a fixed cookie major; tracked by DR-009.                                                                                   |
| [DR-010](https://github.com/seborbie/talos-community/issues/11)       | Windows installer -> Microsoft Edge WebView2 evergreen bootstrapper | The vendor's evergreen URL is a mutable release input, so it cannot satisfy the repository rule that release inputs be digest-pinned.                                                         | The bootstrapper is downloaded over TLS to a temporary file and accepted only when Windows reports a currently valid Authenticode signature from Microsoft Corporation. The same verification runs for every cached copy. Scope is limited to the WebView2 prerequisite; VC++ redistributables remain immutable and digest-pinned. Owner: Talos maintainers. Expiry: 2026-11-17.      | Replace the bootstrapper with a reviewed, versioned WebView2 distribution and published/pinned digest, or consume a Microsoft-signed immutable vendor manifest that binds the selected artifact to a digest. |
| [DR-011](https://github.com/seborbie/talos-community/issues/12)       | Svelte 5.55.9 transform -> Rollup 4 through Vite 7.3.6              | Generated component output carries `/* @__PURE__ */` annotations at positions Rollup cannot retain, producing annotation and secondary source-map diagnostics even though the build succeeds. | The Vite custom logger suppresses only the exact generated annotation/source-map messages from the five registered Svelte sources and fails if the 45-warning ceiling grows. Every other warning reaches Vite unchanged, the policy has negative tests, Svelte checks require zero diagnostics, and production builds remain mandatory. Owner: Talos maintainers. Expiry: 2026-11-30. | Remove the handler and this entry when the supported Svelte/Vite/Rollup graph no longer emits the warning; verify by building without the filter before every dependency upgrade.                            |
| [DR-012](https://github.com/seborbie/talos-community/issues/13)       | Community edge -> official `traefik:latest` image                    | The owner-selected image tag is mutable and violates the repository rule that release inputs must not use floating `latest` tags.                                                             | Resolution occurs only during install or explicit update. The launcher records the immutable registry digest, reported version, resolution time, and previous known-good digest; routine restarts reuse the record, and promotion requires edge/TLS/Talos health checks with rollback. No other Community image may float. Owner: Talos maintainers. Expiry: 2027-08-28.                  | Move to an owner-approved immutable Traefik release policy, or renew the exception after reviewing upstream tag behavior and executing install/update/rollback verification.                                  |

### DR-010 exception evidence

- **Exact unmet rule:** `ENGINEERING_QUALITY.md` requires release inputs to be pinned. The Microsoft
  WebView2 evergreen bootstrapper URL may return a newer Microsoft release without a repository
  change.
- **Why the compliant alternative is currently impractical:** the existing Burn prerequisite flow
  intentionally installs Microsoft's evergreen runtime, and the evergreen bootstrapper is not
  published with a stable artifact digest at that URL. Moving to a fixed-version runtime changes
  installation, servicing, and bundle-size behavior and needs a separately tested release change.
- **Scope and risk:** a new Microsoft-signed bootstrapper can enter a release without ordinary diff
  review; trust therefore rests on Microsoft's signing identity and the Windows trust chain. The
  exception does not apply to any other prerequisite or downloaded build tool.
- **Compensating controls:** transactional download, TLS, valid Authenticode status, signer
  organization equal to Microsoft Corporation, and re-verification of cached files on every build.
- **Owner/tracking/expiry:** Talos maintainers; `DR-010`; 2026-11-17. Convert this key to a public
  issue before the Community release.

### DR-012 exception evidence

- **Exact unmet rule:** `ENGINEERING_QUALITY.md` requires release inputs not to use floating
  `latest` tags. New Community installs intentionally resolve the official `traefik:latest` tag.
- **Why the compliant alternative is currently impractical:** the owner has selected the moving
  stable publisher tag to minimize manual Traefik version maintenance and accepts that a fresh
  install may receive a newer proxy without a Talos source change.
- **Scope and risk:** only the Community Traefik edge may float. An upstream compatibility defect or
  compromised publisher tag can affect new installs and explicit updates. Existing installs do not
  silently change during ordinary restart.
- **Compensating controls:** resolve through the configured registry, record the immutable digest and
  reported version, reuse the digest for routine lifecycle operations, retain the previous digest,
  and promote an update only after routing, certificate, and Talos health checks pass.
- **Owner/tracking/expiry:** Talos maintainers; `DR-012`; 2027-08-28. Convert this key to a public
  issue before the Community release.

## Review procedure

At least quarterly, and before every release:

1. Run `bun run audit:js` and `cargo audit` from `apps/` with current advisory databases.
2. Treat every unregistered Bun advisory and every Rust vulnerability as a release blocker; update
   the narrowest owning direct dependency first.
3. Re-evaluate each JavaScript exception with `bun why <package>` and each Rust informational
   finding against the inverse dependency tree with
   `cargo tree -i <crate> --target all`.
4. Remove entries whose dependency path has disappeared; update controls, owner, and due date for
   anything that remains.
5. Re-evaluate mutable release-input exceptions against the current vendor distribution options,
   execute their compensating verification, and block release after their expiry date.
6. Do not add a new entry merely to turn a failing vulnerability audit green. A vulnerability
   exception additionally requires a concrete exposure analysis, compensating control, maintainer
   owner, tracking key, and review date.

## DR-013: yanked chacha20 in the QUIC graph

Tracking: [https://github.com/seborbie/talos-community/issues/14](https://github.com/seborbie/talos-community/issues/14). Owner: Sebastian Orbe / Talos maintainers.
Review due: 2026-09-11. The 2026-09-04 Rust audit reports `chacha20` 0.10.1 as yanked through
`quinn` 0.11.11 -> `quinn-proto` 0.11.17 -> `rand` 0.10.2 in the viewer/worker graph.
Investigate the upstream yank reason and a compatible non-yanked resolution, then run QUIC/session
regressions and Rust gates. This records an existing warning, grants no vulnerability exception,
and does not establish exploitability. Rust audit must continue to report zero vulnerabilities.
