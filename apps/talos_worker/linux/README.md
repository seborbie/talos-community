# Talos RMM Agent on Linux

This is the Linux managed endpoint. It uses the same `talos_worker` binary and server WebSocket protocol as Windows, but reports Windows-only features as unavailable.

## Build

From `apps/`:

```sh
cargo build -p talos_worker --release --target x86_64-unknown-linux-gnu
cargo build -p talos_supervisor --release --target x86_64-unknown-linux-gnu
```

The Linux build path does not require the Windows desktop capture stack, libvpx, libyuv, ConPTY, or Windows service dependencies.

## Install as a supervised systemd agent

Copy the built supervisor binary and Linux folder to the target host, then run:

```sh
sudo ./apps/talos_worker/linux/install.sh ./apps/target/x86_64-unknown-linux-gnu/release/talos_supervisor
sudo editor /etc/talos/rmm-agent.env
sudo systemctl start talos-supervisor.service
sudo systemctl status talos-supervisor.service
sudo systemctl status talos-worker.service
```

The installer bootstraps `/opt/talos/supervisor/talos_supervisor` and `talos-supervisor.service`. After you configure a self-hosted update endpoint, the supervisor downloads the worker update package, installs it under `/opt/talos/worker`, creates `talos-worker.service`, checks for worker updates every 24 hours, and watches the worker service every 60 seconds. Without an update endpoint, it makes no update requests and continues its local watchdog checks.

Existing legacy `talos-rmm-agent.service` installs are disabled during supervisor installation so only one worker service is active.

Required environment:

- `RMM_SERVER_URL`: Talos `rmm_server` agent WebSocket URL. The example defaults to the same-host Community endpoint, `ws://127.0.0.1:3002/agent/ws`.
- `RMM_AGENT_TOKEN`: enrollment token accepted by `rmm_server`.
- `RMM_AGENT_ID_PATH`: persistent agent id path. The service defaults to `/etc/talos/rmm_agent_id.txt`.
- `RMM_SHELL_USER`: desired dedicated account for interactive system shell sessions. The worker creates `talos` by default, or uses the account specified by this variable.
- `RMM_STUN_SERVER`: optional operator-controlled `hostname-or-IPv4:port` for direct public-UDP
  discovery. It is absent by default; interactive file transfer uses the relay fallback without a
  third-party STUN request.

Supervisor environment is stored in `/etc/talos/talos-supervisor.env`. Important defaults:

- `RMM_UPDATE_BASE_URL`: optional self-hosted update API base. It is absent by default, which disables update requests.
- `RMM_WORKER_INSTALL_DIR`: worker install directory, defaulting to `/opt/talos/worker`.
- `RMM_WORKER_SERVICE_NAME`: worker service managed by the supervisor, defaulting to `talos-worker`.
- `RMM_SUPERVISOR_STARTUP_JITTER_SECS`: startup update jitter. This is `0` for now.
- `RMM_SUPERVISOR_UPDATE_INTERVAL_SECS`: worker update interval. This defaults to `86400`.
- `RMM_SUPERVISOR_MONITOR_INTERVAL_SECS`: watchdog interval. This defaults to `60`.

The shell user must have an interactive shell such as `/bin/bash` or `/bin/sh`. The agent refuses to start an interactive shell as UID 0 unless `RMM_SHELL_ALLOW_ROOT=1` is explicitly set.

## Supported MVP features

- Enrollment/check-in through `rmm_server`.
- Inventory and health telemetry: hostname, distro, kernel, IPs, CPU, memory, disk, uptime, logged-in users where `who` is available, and last-seen.
- Policy-gated remote command execution using `/bin/sh -lc` with `RMM_COMMAND_TIMEOUT_SECS`.
- Interactive system shell through the Talos shell protocol, backed by a Unix PTY and running as `RMM_SHELL_USER`.
- File transfer over the existing QUIC/relay file-transfer protocol.
- Unsupported remote desktop, remote registry, and chat.

Remote desktop, remote registry, and chat sessions are intentionally reported or treated as unsupported on Linux.

## Uninstall

```sh
sudo ./apps/talos_worker/linux/uninstall.sh
```
