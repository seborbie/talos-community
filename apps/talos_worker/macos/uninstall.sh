#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
  echo "uninstall.sh must be run as root" >&2
  exit 1
fi

SUPERVISOR_PLIST_TARGET="/Library/LaunchDaemons/com.talos.talos-supervisor.plist"
WORKER_PLIST_TARGET="/Library/LaunchDaemons/com.talos.talos-worker.plist"
PERMISSIONS_HELPER_PLIST_TARGET="/Library/LaunchAgents/com.talos.permissions-helper.plist"

launchctl bootout system/com.talos.talos-worker >/dev/null 2>&1 || true
launchctl bootout system/com.talos.talos-supervisor >/dev/null 2>&1 || true
CONSOLE_UID="$(stat -f %u /dev/console 2>/dev/null || echo 0)"
if [ "$CONSOLE_UID" != "0" ]; then
  launchctl bootout "gui/$CONSOLE_UID/com.talos.permissions-helper" >/dev/null 2>&1 || true
fi
rm -f "$WORKER_PLIST_TARGET" "$SUPERVISOR_PLIST_TARGET" "$PERMISSIONS_HELPER_PLIST_TARGET"
rm -rf "/Library/Talos/Worker" "/Library/Talos/Supervisor" "/Applications/Talos Permissions Helper.app"

echo "Talos RMM agent apps and launchd services removed. State and env files were left in /Library/Application Support/Talos and /Library/Preferences/Talos."
