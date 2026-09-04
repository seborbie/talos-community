# Talos RMM Agent on macOS

The macOS agent installs each Talos component as a standalone `.app` bundle. The worker LaunchDaemon starts `Talos Worker.app` directly; the supervisor reads `/Library/Preferences/Talos/rmm-agent.env` and writes those values into launchd `EnvironmentVariables` instead of using a worker shell wrapper.

Installed app bundles:

- `/Library/Talos/Supervisor/Talos Supervisor.app`
- `/Library/Talos/Worker/Talos Worker.app`
- `/Library/Talos/Worker/Talos Worker Helper.app`
- `/Library/Talos/Worker/Talos Worker Chat.app`
- `/Applications/Talos Permissions Helper.app`

Supported macOS features in this phase:

- System information plus startup, periodic, and dashboard-requested telemetry snapshots.
- Policy-approved one-shot shell commands.
- Interactive system shell sessions running as the root LaunchDaemon user.
- File transfer over the existing Talos transfer transports.
- Remote desktop over the modern `modern_gpu` ATX2/BGRA atlas-command viewer protocol, with legacy IVF/VP8 as a fallback profile.
- Session chat with a user-visible worker chat app launched in the active console session.

Remote registry, Developer ID signing, notarization, and TCC/MDM privacy payloads are intentionally deferred. Remote desktop currently supports the primary display only and requires manual Screen Recording and Accessibility approval for the helper.

## Build

From the repository root on macOS:

```sh
./scripts/build-macos-agent.sh
```

The script builds `aarch64-apple-darwin` and `x86_64-apple-darwin`, embeds the local manifest verification public key, signs update manifests with the matching private key, creates universal Mach-O executables with `lipo`, and packages them into `.app` bundles. The generic package is written to `apps/installer/artifacts/<profile>/Talos.Agent.macos-universal.pkg`.

The universal app bundles are written to `apps/installer/artifacts/<profile>/macos-universal/`. Update artifacts remain separate as `macos-arm64` and `macos-x64` and contain only app bundles:

- `Talos.Worker.<arch>.Update.zip`: Worker, Worker Helper, and Worker Chat apps.
- `Talos.Supervisor.<arch>.Update.zip`: Supervisor app.

Set `BUILD_PROFILE`, `ARTIFACT_DIR`, `MACOS_CARGO_PROFILE`, or `MACOS_SIGNING_IDENTITY` to override the defaults. By default, the script builds Cargo debug binaries for faster local iteration; pass `--release` or set `MACOS_CARGO_PROFILE=release` when you need optimized release binaries. When `MACOS_SIGNING_IDENTITY` is provided, also set `RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH` and `RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH` so the update trust key is embedded and manifests are signed.

Talos Worker Helper links VP8 for the legacy remote desktop fallback stream. The macOS build script builds a cached, static, VP8-only libvpx for each Apple target under `MACOS_CARGO_TARGET_DIR/macos-deps`, so it does not depend on Homebrew `pkg-config` state or a single-architecture Homebrew libvpx install. The default libvpx 1.13.0 source archive is pinned by SHA-256; downloads, existing archive caches, and compiled-prefix provenance stamps are checked before reuse.

Set `MACOS_DEPS_DIR` to relocate the cache. If `MACOS_LIBVPX_VERSION` or `MACOS_LIBVPX_URL` changes, also set `MACOS_LIBVPX_SHA256` to the reviewed archive's 64-character digest. The script intentionally refuses a custom source without that digest and rejects a stale cached archive or compiled prefix whose recorded source digest differs.

The macOS scripts create signed local packages but do not yet submit them to Apple's notarization service or staple a ticket. Notarization requires the release owner's Apple credentials and is a release blocker: do not publish either macOS installer until an authorized maintainer notarizes, staples, and Gatekeeper-verifies the final package.

## Local Install

Install the generic package directly:

```sh
sudo installer -pkg ./apps/installer/artifacts/dev/Talos.Agent.macos-universal.pkg -target /
```

Then edit:

- `/Library/Preferences/Talos/rmm-agent.env`
- `/Library/Preferences/Talos/talos-supervisor.env`

The package defaults the agent connection to the Community stack on the same host:

```sh
RMM_SERVER_URL='ws://127.0.0.1:3002/agent/ws'
```

For a remote self-hosted deployment, replace that value with its externally reachable WebSocket
endpoint. To opt in to updates, also configure its update API, for example
`RMM_UPDATE_BASE_URL='https://talos.example/rmm/updates'`.

The package omits `RMM_UPDATE_BASE_URL` by default, so it makes no update requests until you add
your self-hosted endpoint to `/Library/Preferences/Talos/talos-supervisor.env`.

An enrollment token is still required in `RMM_AGENT_TOKEN`.

The dashboard macOS install command downloads this package, writes the scoped enrollment environment files, and restarts the supervisor automatically.

For local app-bundle testing without the package, `install.sh` accepts `Talos Supervisor.app` plus any other generated Talos app bundles:

```sh
sudo ./apps/talos_worker/macos/install.sh \
  ./apps/installer/artifacts/dev/macos-universal/Talos\ Supervisor.app \
  ./apps/installer/artifacts/dev/macos-universal/Talos\ Worker.app \
  ./apps/installer/artifacts/dev/macos-universal/Talos\ Worker\ Helper.app \
  ./apps/installer/artifacts/dev/macos-universal/Talos\ Worker\ Chat.app \
  ./apps/installer/artifacts/dev/macos-universal/Talos\ Permissions\ Helper.app
```

## Permissions

The package installs `/Applications/Talos Permissions Helper.app` and opens it after install in the active console user session.

Grant these app bundles on dev devices:

- Full Disk Access: `/Library/Talos/Worker/Talos Worker.app`
- Screen Recording: `/Library/Talos/Worker/Talos Worker Helper.app`
- Accessibility: `/Library/Talos/Worker/Talos Worker Helper.app`

The Full Disk Access approval belongs to the worker app bundle because `talos_worker` performs file operations. Screen Recording and Accessibility belong to the helper app bundle because it performs ScreenCaptureKit capture and CoreGraphics input injection.

## Remote Desktop

Remote desktop uses the existing viewer stack with `modern_gpu` selected by default. In that mode the helper sends ATX2/BGRA atlas-command display records, including dirty row-band updates for changed frames and bounded chunks for large displays. The `legacy` profile remains available and uses VP8/IVF stream framing. The root worker opens `Talos Worker Helper.app` in the active console user session with `launchctl asuser`, then talks to it over authenticated Unix domain sockets.

Unsupported Windows-only controls, including secure attention and session switching, are logged and ignored on macOS.

## Worker Chat

Worker chat launches lazily after the technician sends the first chat message. The root worker opens `/Library/Talos/Worker/Talos Worker Chat.app` in the active Aqua console session with `launchctl asuser`. Chat logs prefer `/Library/Logs/Talos/talos_worker_chat.log` and fall back to the user temp directory if that path is not writable.

## launchd Checks

```sh
sudo launchctl print system/com.talos.talos-supervisor
sudo launchctl print system/com.talos.talos-worker
tail -f /Library/Logs/Talos/talos_supervisor.log
tail -f /Library/Logs/Talos/talos_worker.log
```

## Remote System Shell

Interactive shell sessions require the worker to run as root from the `com.talos.talos-worker` LaunchDaemon. macOS user-session shells are not supported in this phase; shell sessions run as the root LaunchDaemon user with `HOME=/var/root` and `/bin/zsh` when available.

If the dashboard cannot open a macOS system shell, confirm:

```sh
sudo launchctl print system/com.talos.talos-worker
sudo tail -n 200 /Library/Logs/Talos/talos_worker.log
```

Common failures are a non-root worker process, PTY creation errors, or an unsupported request for `run_as=user`.

## Uninstall

```sh
sudo ./apps/talos_worker/macos/uninstall.sh
```
