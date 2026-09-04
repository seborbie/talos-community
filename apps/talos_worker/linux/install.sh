#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SUPERVISOR_BIN_SRC="${1:-}"

if [ -z "$SUPERVISOR_BIN_SRC" ]; then
  SUPERVISOR_BIN_SRC="$SCRIPT_DIR/../../target/x86_64-unknown-linux-gnu/release/talos_supervisor"
fi

if [ ! -f "$SUPERVISOR_BIN_SRC" ]; then
  echo "talos_supervisor binary not found: $SUPERVISOR_BIN_SRC" >&2
  echo "Build first with: cargo build --locked -p talos_supervisor --release --target x86_64-unknown-linux-gnu" >&2
  exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "systemctl is required for the current Talos Linux supervisor installer." >&2
  exit 1
fi

for unit in talos-supervisor.service talos-worker.service talos-rmm-agent.service; do
  if systemctl list-unit-files "$unit" >/dev/null 2>&1 || systemctl status "$unit" >/dev/null 2>&1; then
    systemctl disable --now "$unit" >/dev/null 2>&1 || true
  fi
  rm -f "/etc/systemd/system/$unit"
  systemctl reset-failed "$unit" >/dev/null 2>&1 || true
done

install -d -m 0755 \
  /opt/talos/supervisor \
  /opt/talos/worker \
  /etc/talos \
  /var/lib/talos/updates \
  /var/log/talos \
  /etc/systemd/system
install -m 0755 "$SUPERVISOR_BIN_SRC" /opt/talos/supervisor/talos_supervisor
install -m 0644 "$SCRIPT_DIR/talos-supervisor.service" /etc/systemd/system/talos-supervisor.service

if [ ! -f /etc/talos/rmm-agent.env ]; then
  install -m 0600 "$SCRIPT_DIR/rmm-agent.env.example" /etc/talos/rmm-agent.env
  echo "Created /etc/talos/rmm-agent.env; edit RMM_SERVER_URL and RMM_AGENT_TOKEN before starting." >&2
fi

if [ ! -f /etc/talos/talos-supervisor.env ]; then
  install -m 0600 "$SCRIPT_DIR/talos-supervisor.env.example" /etc/talos/talos-supervisor.env
  echo "Created /etc/talos/talos-supervisor.env; updates remain disabled until RMM_UPDATE_BASE_URL points to your self-hosted API." >&2
fi

systemctl daemon-reload
systemctl enable talos-supervisor.service

echo "Installed talos-supervisor.service."
echo "Interactive shell user will be provisioned by the worker (default: talos)."
echo "Start with: systemctl start talos-supervisor.service"
