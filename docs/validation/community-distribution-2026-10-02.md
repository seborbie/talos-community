# Community distribution validation — 2026-10-02

This records implementation evidence, not release approval. The distribution branch is stacked on
PR48 commit `6e1c9c22bc400c403c0079a20b337649d67861c3`; it introduces no lockfile changes.
PR41/40 and the separate PR49 Tauri repair remain independent integration work.

## Executed checks

- Locked controller Rust tests: 49 unit tests and one real executable test passed on macOS ARM64.
  The Linux/Windows missing-Docker executable test is intentionally platform-specific.
- `cargo fmt --all --check` and focused controller Clippy with `-D warnings` passed.
- The Linux x64 launcher was built and its locked tests executed in an amd64 Docker builder.
- The bundle, edge and release-pipeline focused tests passed: 39 tests, including actual Bash/jq
  candidate identity, controller/full signing-scope rejection, and exact archive-selection checks. TypeScript script checks
  and formatting passed. Temporary diagnostic logging was removed.
- `bun run quality` passed toolchain/workspace/frozen-lock/protocol checks, static and licence
  checks, 171 script tests, 215 API tests, 30 frontend tests and native frontend placeholder tests,
  production web builds, and JS audit. It then failed in the existing macOS `permission-flow`
  Swift bundle codesign step: “resource fork, Finder information, or similar detritus not allowed”.
  The full workspace Rust checks/tests therefore were not completed. No gate was disabled.
- Standalone JS and Rust audits passed on the PR48 base; Rust reported 13 existing allowed warnings.
- Some subsequent local filesystem-heavy test runs exceeded Bun's existing five-second timeout.
  A temporary checkout outside Documents, with the reviewed excluded libvpx source reconstructed,
  passed the complete 185-test script suite at the normal timeout before the final two archive
  regression tests were added. The standalone signing-secret policy scan also passed in the
  original checkout (946 files). No timeout policy was changed.

## Actual local runtime acceptance

The test host was macOS ARM64 with existing Docker Desktop, a Linux daemon and Compose 5.4.0.
The **native Linux x64 launcher** ran under amd64 emulation with Docker CLI/socket access only in
its temporary test harness. Application containers never received the Docker socket.
Locally built production Dockerfiles supplied preloaded ARM64 application images. This was not a
released archive or anonymous GHCR pull, and does not certify the final release's image bytes.

An interrupted-install fixture supplied validated configuration, generated protected secrets,
resolved official Traefik digest and a pre-migration journal. Actual installer orchestration then
ran database preflight, backup, migrations and service startup. It exposed a real address conflict:
Docker assigned the relay the static trusted Traefik address before Traefik started. The journal
correctly required recovery after migrations. The new non-overlapping IPAM pool and explicit
preserve-volume network recreation resolved that conflict.

The rebuilt launcher restored the real pre-migration PostgreSQL backup and reached healthy state
with a cleared journal. The following assertions then passed:

- Frontend and API HTTPS responses using the generated certificate with `curl --cacert` and local
  resolution; no TLS bypass or certificate-store change.
- First-account registration, then closed registration and successful login.
- Stop/start and repeat quickstart, followed by preserve-data uninstall and restart.
- The same user could log in after restart; installation identity and generated secrets persisted.
- Published ports were exactly IPv4 loopback 8080 and 8443.
- Explicit confirmed removal deleted only the disposable test state and named volumes.

Synthetic credentials/auth tokens and certificate private keys were kept private and removed.
Only non-secret statuses, port mappings and result text were retained outside the repository.
No public image/release publication, persistent credential creation, trust-store change, or
repository/package security-setting change occurred.

## Still required before publication

Run the unchanged hosted quality/security gates on the final integration commit. Complete
qualified human installer/release review and confirm binary/container licensing and source
publication approval. The existing publication decision covers a source-only alpha; this report
cannot extend its scope. Inspect protected environment/tag rules before dispatching any release.

Configure four repository-linked public GHCR packages through the owner's approved process.
The publication pipeline must pass the actual anonymous exact-digest check and released-archive
Linux smoke test; neither has run yet. No new persistent PAT is required.

Actual clean Windows Docker Desktop startup, double-click/repeat/failure behavior, restrictive
DACLs, browser certificate trust and organisation SmartScreen approval remain unverified here.
Hosted Windows CI verifies build/help and command dispatch, not Docker Desktop integration.
ARM64 runtime support, public ACME and real relay/WebSocket traffic are not demonstrated by this
local test. Clean-host upgrade and deliberately failed-update recovery evidence also remain
release requirements. Full native-client scope additionally needs its protected signer,
PFX/password and independently reviewed updater-key fingerprint; controller-only scope does not.
