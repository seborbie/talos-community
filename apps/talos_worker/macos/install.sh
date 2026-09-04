#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
  echo "install.sh must be run as root" >&2
  exit 1
fi

if [ "$#" -lt 1 ]; then
  echo "usage: $0 /path/to/Talos\\ Supervisor.app [/path/to/Talos\\ Worker.app ...]" >&2
  exit 1
fi

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SUPERVISOR_SOURCE_APP="$1"
shift

SUPERVISOR_INSTALL_DIR="/Library/Talos/Supervisor"
WORKER_INSTALL_DIR="/Library/Talos/Worker"
STATE_DIR="/Library/Application Support/Talos"
ENV_DIR="/Library/Preferences/Talos"
LOG_DIR="/Library/Logs/Talos"
SUPERVISOR_APP_TARGET="$SUPERVISOR_INSTALL_DIR/Talos Supervisor.app"
SUPERVISOR_EXE_TARGET="$SUPERVISOR_APP_TARGET/Contents/MacOS/talos_supervisor"
SUPERVISOR_PLIST_TARGET="/Library/LaunchDaemons/com.talos.talos-supervisor.plist"
WORKER_PLIST_TARGET="/Library/LaunchDaemons/com.talos.talos-worker.plist"
ENV_TARGET="$ENV_DIR/rmm-agent.env"
SUPERVISOR_ENV_TARGET="$ENV_DIR/talos-supervisor.env"
SUPERVISOR_WRAPPER_TARGET="$SUPERVISOR_INSTALL_DIR/run-talos-supervisor.sh"
MACOS_UPDATE_ACCOUNT_SOCKET="/var/run/talos/macos-update-account.sock"
INSTALL_LOG="$LOG_DIR/talos_agent_install.log"

log_install() {
  printf '%s %s\n' "$(/bin/date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$INSTALL_LOG" 2>/dev/null || true
}

wait_for_macos_update_account_socket() {
  attempts=0
  while [ "$attempts" -lt 30 ]; do
    if [ -S "$MACOS_UPDATE_ACCOUNT_SOCKET" ]; then
      log_install "macOS update account socket is ready at $MACOS_UPDATE_ACCOUNT_SOCKET"
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 1
  done
  log_install "macOS update account socket was not ready at $MACOS_UPDATE_ACCOUNT_SOCKET after ${attempts}s"
  return 1
}

require_app_executable() {
  app_path="$1"
  executable_name="$2"
  if [ ! -d "$app_path" ]; then
    echo "app bundle not found: $app_path" >&2
    exit 1
  fi
  if [ ! -f "$app_path/Contents/MacOS/$executable_name" ]; then
    echo "app executable not found: $app_path/Contents/MacOS/$executable_name" >&2
    exit 1
  fi
}

install_app_bundle() {
  source_app="$1"
  target_app="$2"
  executable_name="$3"
  require_app_executable "$source_app" "$executable_name"
  rm -rf "$target_app"
  /usr/bin/ditto --norsrc "$source_app" "$target_app"
  chmod 0755 "$target_app/Contents/MacOS/$executable_name"
  chown -R root:wheel "$target_app"
}

require_app_executable "$SUPERVISOR_SOURCE_APP" "talos_supervisor"

mkdir -p "$SUPERVISOR_INSTALL_DIR" "$WORKER_INSTALL_DIR" "$STATE_DIR/updates" "$ENV_DIR" "$LOG_DIR"

launchctl bootout system/com.talos.talos-worker >/dev/null 2>&1 || true
launchctl bootout system/com.talos.talos-supervisor >/dev/null 2>&1 || true

rm -f "$SUPERVISOR_INSTALL_DIR/talos_supervisor" \
  "$SUPERVISOR_INSTALL_DIR/updater" \
  "$SUPERVISOR_INSTALL_DIR/talos_supervisor.next" \
  "$SUPERVISOR_INSTALL_DIR/updater.next" \
  "$SUPERVISOR_INSTALL_DIR/talos_supervisor.previous" \
  "$SUPERVISOR_INSTALL_DIR/updater.previous"
rm -f "$WORKER_INSTALL_DIR/talos_worker" \
  "$WORKER_INSTALL_DIR/talos_worker_helper" \
  "$WORKER_INSTALL_DIR/talos_worker_chat" \
  "$WORKER_INSTALL_DIR/talos-rmm-agent" \
  "$WORKER_INSTALL_DIR/run-talos-worker.sh"

install_app_bundle "$SUPERVISOR_SOURCE_APP" "$SUPERVISOR_APP_TARGET" "talos_supervisor"

for source_app in "$@"; do
  app_name="$(basename "$source_app")"
  case "$app_name" in
    "Talos Worker.app")
      install_app_bundle "$source_app" "$WORKER_INSTALL_DIR/Talos Worker.app" "talos_worker"
      ;;
    "Talos Worker Helper.app")
      install_app_bundle "$source_app" "$WORKER_INSTALL_DIR/Talos Worker Helper.app" "talos_worker_helper"
      ;;
    "Talos Worker Chat.app")
      install_app_bundle "$source_app" "$WORKER_INSTALL_DIR/Talos Worker Chat.app" "talos_worker_chat"
      ;;
    "Talos Permissions Helper.app")
      install_app_bundle "$source_app" "/Applications/Talos Permissions Helper.app" "talos_permissions_helper"
      ;;
    *)
      echo "unsupported Talos app bundle: $source_app" >&2
      exit 1
      ;;
  esac
done

if [ ! -f "$ENV_TARGET" ]; then
  install -m 0600 "$SCRIPT_DIR/rmm-agent.env.example" "$ENV_TARGET"
fi
if [ ! -f "$SUPERVISOR_ENV_TARGET" ]; then
  install -m 0600 "$SCRIPT_DIR/talos-supervisor.env.example" "$SUPERVISOR_ENV_TARGET"
fi

cat > "$SUPERVISOR_WRAPPER_TARGET" <<'EOF'
#!/bin/sh
set -eu

ENV_FILE="/Library/Preferences/Talos/talos-supervisor.env"
if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$ENV_FILE"
  set +a
fi

exec "/Library/Talos/Supervisor/Talos Supervisor.app/Contents/MacOS/talos_supervisor"
EOF
chmod 0755 "$SUPERVISOR_WRAPPER_TARGET"

cat > "$SUPERVISOR_PLIST_TARGET" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.talos.talos-supervisor</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Library/Talos/Supervisor/run-talos-supervisor.sh</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>WorkingDirectory</key>
  <string>/Library/Talos/Supervisor</string>
  <key>StandardOutPath</key>
  <string>/Library/Logs/Talos/talos_supervisor.launchd.out.log</string>
  <key>StandardErrorPath</key>
  <string>/Library/Logs/Talos/talos_supervisor.launchd.err.log</string>
</dict>
</plist>
EOF
chmod 0644 "$SUPERVISOR_PLIST_TARGET"
chown root:wheel "$SUPERVISOR_PLIST_TARGET"
rm -f "$WORKER_PLIST_TARGET"

launchctl bootstrap system "$SUPERVISOR_PLIST_TARGET"
launchctl enable system/com.talos.talos-supervisor
launchctl kickstart -k system/com.talos.talos-supervisor
wait_for_macos_update_account_socket >/dev/null 2>&1 || true

echo "Talos macOS supervisor app installed. Edit $ENV_TARGET and $SUPERVISOR_ENV_TARGET if enrollment values are placeholders."
