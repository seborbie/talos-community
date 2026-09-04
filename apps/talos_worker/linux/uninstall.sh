#!/usr/bin/env sh
set -eu

if command -v systemctl >/dev/null 2>&1; then
  systemctl disable --now talos-supervisor.service 2>/dev/null || true
  systemctl disable --now talos-worker.service 2>/dev/null || true
  systemctl disable --now talos-rmm-agent.service 2>/dev/null || true
  rm -f /etc/systemd/system/talos-supervisor.service
  rm -f /etc/systemd/system/talos-worker.service
  rm -f /etc/systemd/system/talos-rmm-agent.service
  systemctl daemon-reload
fi

echo "Removed Talos Linux services. Preserved /etc/talos configuration, /var/lib/talos state, and installed binaries."
