#!/bin/sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
APPS_ROOT="$REPO_ROOT/apps"
VIEWER_ROOT="$APPS_ROOT/talos_viewer"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$APPS_ROOT/installer/artifacts/$BUILD_PROFILE}"
CERTS_DIR="${MACOS_CERTS_DIR:-$APPS_ROOT/certs}"
SIGNING_DIR="${SIGNING_DIR:-${MACOS_SIGNING_DIR:-$CERTS_DIR}}"
MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH="${RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH:-$CERTS_DIR/talos-manifest-signing.key.pem}"
MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH="${RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH:-$CERTS_DIR/talos-manifest-signing-public.der}"
MANIFEST_SIGNING_PFX_PATH="${RMM_MANIFEST_SIGNING_PFX_PATH:-$CERTS_DIR/Talos Manifest Signing.pfx}"
PKG_IDENTIFIER="${MACOS_VIEWER_PKG_IDENTIFIER:-com.talos.viewer.pkg}"
PKG_FILE_NAME="${MACOS_VIEWER_PKG_FILE_NAME:-Talos.Viewer.macos.pkg}"
PKG_PATH="$ARTIFACT_DIR/$PKG_FILE_NAME"
PKG_SIGNING_IDENTITY="${MACOS_PKG_SIGNING_IDENTITY:-${MACOS_VIEWER_PKG_SIGNING_IDENTITY:-}}"
MACOS_SIGNING_KEYCHAIN="${MACOS_SIGNING_KEYCHAIN:-}"
DEFAULT_CODESIGN_CERT_SHA256="57614DC858DC4A04D2870A53E138C5249AA174B350C53239FDD6142BAF3C2253"
MACOS_CODESIGN_IDENTITY="${MACOS_SIGNING_IDENTITY:-${MACOS_CODESIGN_IDENTITY:-${MACOS_VIEWER_CODESIGN_IDENTITY:-}}}"
MACOS_CODESIGN_CERT_SHA256="${MACOS_CODESIGN_CERT_SHA256:-${MACOS_VIEWER_CODESIGN_CERT_SHA256:-$DEFAULT_CODESIGN_CERT_SHA256}}"
TAURI_CLI_VERSION="2.10.1"
TAURI_BUNDLES="${TAURI_BUNDLES:-app}"
TAURI_BUILD_ARGS="${TAURI_BUILD_ARGS:-}"
MACOS_CARGO_PROFILE="${MACOS_CARGO_PROFILE:-debug}"
APP_NAME="${MACOS_VIEWER_APP_NAME:-Talos Viewer.app}"
FRONTEND_BUILD_HASH_PATH="$VIEWER_ROOT/.dist-build-hash"
SIGNING_IDENTITY=""

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
export COPYFILE_DISABLE="${COPYFILE_DISABLE:-1}"

require_macos() {
  if [ "$(uname -s)" != "Darwin" ]; then
    echo "build-macos-viewer.sh must be run on macOS." >&2
    exit 1
  fi
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required" >&2
    exit 1
  fi
}

print_usage() {
  cat <<EOF_USAGE
Usage: $0 [--debug|--release]

Builds debug binaries by default for faster local iteration.

Options:
  --debug      Build a Tauri debug app bundle (default)
  --release    Build a Tauri release app bundle
  -h, --help   Show this help
EOF_USAGE
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --debug)
        MACOS_CARGO_PROFILE="debug"
        ;;
      --release)
        MACOS_CARGO_PROFILE="release"
        ;;
      -h|--help)
        print_usage
        exit 0
        ;;
      *)
        echo "Unknown option: $1" >&2
        print_usage >&2
        exit 1
        ;;
    esac
    shift
  done

  case "$MACOS_CARGO_PROFILE" in
    debug|release)
      ;;
    *)
      echo "MACOS_CARGO_PROFILE must be 'debug' or 'release', got '$MACOS_CARGO_PROFILE'." >&2
      exit 1
      ;;
  esac
}

package_version() {
  sed -n 's/^version = "\(.*\)"/\1/p' "$1" | head -n 1
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

file_size() {
  stat -f%z "$1"
}

normalize_keychain_path() {
  value="$(printf '%s' "$1" | tr -d '"' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  if [ -z "$value" ]; then
    return
  fi
  case "$value" in
    /*)
      printf '%s\n' "$value"
      ;;
    *)
      if [ -f "$value" ]; then
        printf '%s\n' "$value"
      elif [ -f "$HOME/Library/Keychains/$value" ]; then
        printf '%s\n' "$HOME/Library/Keychains/$value"
      else
        printf '%s\n' "$value"
      fi
      ;;
  esac
}

normalize_fingerprint() {
  printf '%s' "$1" | tr -d '[:space:]:' | tr '[:lower:]' '[:upper:]'
}

find_codesign_identity_by_sha256() {
  target_fingerprint="$(normalize_fingerprint "$1")"
  if [ -z "$target_fingerprint" ]; then
    echo "MACOS_CODESIGN_CERT_SHA256 must not be empty." >&2
    exit 1
  fi

  tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/talos-viewer-codesign-certs.XXXXXX")"
  certs_pem="$tmp_dir/certs.pem"
  if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
    security find-certificate -a -p "$MACOS_SIGNING_KEYCHAIN" > "$certs_pem"
    identities="$(security find-identity -v -p codesigning "$MACOS_SIGNING_KEYCHAIN" 2>/dev/null || true)"
  else
    security find-certificate -a -p > "$certs_pem"
    identities="$(security find-identity -v -p codesigning 2>/dev/null || true)"
  fi

  awk '
    /-----BEGIN CERTIFICATE-----/ {
      n += 1
      file = sprintf("%s/cert-%05d.pem", dir, n)
    }
    file != "" {
      print > file
    }
    /-----END CERTIFICATE-----/ {
      close(file)
      file = ""
    }
  ' dir="$tmp_dir" "$certs_pem"

  for cert_path in "$tmp_dir"/cert-*.pem; do
    if [ ! -f "$cert_path" ]; then
      continue
    fi
    cert_sha256="$(openssl x509 -in "$cert_path" -noout -fingerprint -sha256 | sed 's/^.*=//' | tr -d ':' | tr '[:lower:]' '[:upper:]')"
    if [ "$cert_sha256" != "$target_fingerprint" ]; then
      continue
    fi

    cert_sha1="$(openssl x509 -in "$cert_path" -noout -fingerprint -sha1 | sed 's/^.*=//' | tr -d ':' | tr '[:lower:]' '[:upper:]')"
    cert_subject="$(openssl x509 -in "$cert_path" -noout -subject | sed 's/^subject=//')"
    rm -rf "$tmp_dir"

    if printf '%s\n' "$identities" | grep -qi "$cert_sha1"; then
      printf '%s\n' "$cert_sha1"
      return
    fi

    echo "Code signing certificate was found, but no matching private-key identity is available for codesign." >&2
    echo "  SHA-256: $target_fingerprint" >&2
    echo "  Subject: $cert_subject" >&2
    echo "Import the certificate private key into the login keychain, or set MACOS_SIGNING_KEYCHAIN to the keychain that contains it." >&2
    exit 1
  done

  rm -rf "$tmp_dir"
  echo "Code signing certificate not found in the macOS keychain search list." >&2
  echo "  Expected SHA-256: $target_fingerprint" >&2
  echo "Set MACOS_CODESIGN_CERT_SHA256 or MACOS_CODESIGN_IDENTITY to override the default signing identity." >&2
  exit 1
}

resolve_codesign_identity() {
  if [ -n "$MACOS_CODESIGN_IDENTITY" ]; then
    SIGNING_IDENTITY="$MACOS_CODESIGN_IDENTITY"
  else
    SIGNING_IDENTITY="$(find_codesign_identity_by_sha256 "$MACOS_CODESIGN_CERT_SHA256")"
  fi
  echo "Using macOS code signing identity: $SIGNING_IDENTITY"
}

codesign_app_bundle() {
  bundle_path="$1"
  if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
    codesign --force --deep --sign "$SIGNING_IDENTITY" --keychain "$MACOS_SIGNING_KEYCHAIN" --timestamp=none "$bundle_path"
  else
    codesign --force --deep --sign "$SIGNING_IDENTITY" --timestamp=none "$bundle_path"
  fi
  codesign --verify --deep --strict --verbose=2 "$bundle_path"
}

frontend_build_hash() {
  (
    cd "$VIEWER_ROOT"
    {
      for path in \
        "$APPS_ROOT/package.json" \
        "$APPS_ROOT/bun.lock" \
        "$APPS_ROOT/bunfig.toml" \
        "$APPS_ROOT/talos_protocol_types/package.json" \
        "$APPS_ROOT/talos_protocol_types/src" \
        package.json vite.config.ts svelte.config.js tsconfig.json index.html src public; do
        if [ -e "$path" ]; then
          find "$path" -type f -print
        fi
      done
    } | LC_ALL=C sort | while IFS= read -r file_path; do
      printf '%s\n' "$file_path"
      shasum -a 256 "$file_path"
    done | shasum -a 256 | awk '{print $1}'
  )
}

ensure_frontend_dist() {
  next_hash="$(frontend_build_hash)"
  previous_hash=""
  if [ -f "$FRONTEND_BUILD_HASH_PATH" ]; then
    previous_hash="$(cat "$FRONTEND_BUILD_HASH_PATH")"
  fi

  if [ "$next_hash" = "$previous_hash" ] && [ -f "$VIEWER_ROOT/dist/index.html" ]; then
    echo "Viewer frontend dist is up to date; skipping Vite build."
    return
  fi

  (
    cd "$VIEWER_ROOT"
    bun run build
  )
  printf '%s\n' "$next_hash" > "$FRONTEND_BUILD_HASH_PATH"
}

require_manifest_public_key() {
  if [ ! -f "$MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH" ]; then
    echo "Manifest signing public key not found: $MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH" >&2
    exit 1
  fi
}

extract_manifest_signing_key() {
  output_path="$1"
  if [ ! -f "$MANIFEST_SIGNING_PFX_PATH" ]; then
    return 1
  fi
  if [ -z "${RMM_MANIFEST_SIGNING_PFX_PASSWORD:-}" ]; then
    return 1
  fi
  openssl pkcs12 \
    -in "$MANIFEST_SIGNING_PFX_PATH" \
    -nocerts \
    -nodes \
    -passin env:RMM_MANIFEST_SIGNING_PFX_PASSWORD 2>/dev/null |
    openssl pkey -out "$output_path" 2>/dev/null
}

manifest_signing_key_arg() {
  if [ -f "$MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH" ]; then
    printf '%s\n' "$MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH"
    return
  fi

  tmp_key="$(mktemp "${TMPDIR:-/tmp}/talos-manifest-key.XXXXXX")"
  if extract_manifest_signing_key "$tmp_key"; then
    printf '%s\n' "$tmp_key"
    return
  fi

  rm -f "$tmp_key"
  echo "Manifest signing private key not found: $MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH" >&2
  echo "Expected signing material in apps/certs. Add talos-manifest-signing.key.pem, set RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH, or set RMM_MANIFEST_SIGNING_PFX_PASSWORD for: $MANIFEST_SIGNING_PFX_PATH" >&2
  exit 1
}

sign_manifest() {
  manifest_path="$1"
  signature_path="$2"
  key_path="$(manifest_signing_key_arg)"
  tmp_sig="$(mktemp)"
  openssl dgst -sha256 -sign "$key_path" -out "$tmp_sig" "$manifest_path"
  openssl base64 -A -in "$tmp_sig" -out "$signature_path"
  case "$key_path" in
    "${TMPDIR:-/tmp}"/*)
      rm -f "$key_path"
      ;;
  esac
  rm -f "$tmp_sig"
}

target_profile_dir() {
  if [ "$MACOS_CARGO_PROFILE" = "release" ]; then
    printf '%s\n' "release"
  else
    printf '%s\n' "debug"
  fi
}

tauri_build_profile_args() {
  if [ "$MACOS_CARGO_PROFILE" = "release" ]; then
    return
  fi
  printf '%s\n' "--debug"
}

write_update_manifest() {
  arch="$1"
  version="$2"
  manifest_path="$ARTIFACT_DIR/Talos.Viewer.$arch.Update.manifest.json"
  signature_path="$ARTIFACT_DIR/Talos.Viewer.$arch.Update.manifest.sig"
  package_sha="$(sha256_file "$PKG_PATH")"
  package_size="$(file_size "$PKG_PATH")"
  published_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  cat > "$manifest_path" <<EOF_MANIFEST
{
  "product": "viewer",
  "platform": "macos",
  "arch": "$arch",
  "channel": "stable",
  "version": "$version",
  "minimumSupportedVersion": "$version",
  "severity": "normal",
  "publishedAtUtc": "$published_at",
  "rolloutPercentage": 100,
  "package": {
    "fileName": "$PKG_FILE_NAME",
    "sizeBytes": $package_size,
    "sha256": "$package_sha"
  },
  "contents": [
    "$APP_NAME"
  ],
  "requiresRestart": true,
  "installMode": "pkg"
}
EOF_MANIFEST
  sign_manifest "$manifest_path" "$signature_path"
}

find_app_bundle() {
  profile_dir="$(target_profile_dir)"
  for root in "$APPS_ROOT/target" "$REPO_ROOT/target" "$VIEWER_ROOT/src-tauri/target"; do
    if [ ! -d "$root" ]; then
      continue
    fi
    found="$(find "$root" -path "*/$profile_dir/bundle/macos/$APP_NAME" -type d -print -quit 2>/dev/null || true)"
    if [ -n "$found" ]; then
      printf '%s\n' "$found"
      return
    fi
  done

  echo "Unable to find $APP_NAME after Tauri build." >&2
  exit 1
}

remove_viewer_bundle_copies_in_dir() {
  bundle_dir="$1"
  if [ ! -d "$bundle_dir" ]; then
    return
  fi
  for bundle in "$bundle_dir"/Talos\ Viewer.app "$bundle_dir"/Talos\ Viewer\ [0-9]*.app; do
    if [ ! -e "$bundle" ]; then
      continue
    fi
    chmod -R u+w "$bundle" >/dev/null 2>&1 || true
    rm -rf "$bundle" >/dev/null 2>&1 || true
    if [ -e "$bundle" ]; then
      echo "Unable to remove stale viewer bundle: $bundle" >&2
      echo "Check ownership/permissions, then remove it before rebuilding." >&2
      exit 1
    fi
  done
}

remove_numbered_viewer_bundles_in_dir() {
  bundle_dir="$1"
  if [ ! -d "$bundle_dir" ]; then
    return
  fi
  for bundle in "$bundle_dir"/Talos\ Viewer\ [0-9]*.app; do
    if [ ! -e "$bundle" ]; then
      continue
    fi
    chmod -R u+w "$bundle" >/dev/null 2>&1 || true
    rm -rf "$bundle" >/dev/null 2>&1 || true
    if [ -e "$bundle" ]; then
      echo "Unable to remove duplicate viewer bundle: $bundle" >&2
      echo "Check ownership/permissions, then remove it before packaging." >&2
      exit 1
    fi
  done
}

clean_stale_viewer_bundles() {
  bundle_list="$(mktemp "${TMPDIR:-/tmp}/talos-viewer-bundle-dirs.XXXXXX")"
  for root in "$APPS_ROOT/target" "$REPO_ROOT/target" "$VIEWER_ROOT/src-tauri/target"; do
    if [ ! -d "$root" ]; then
      continue
    fi
    find "$root" -path "*/bundle/macos" -type d -print 2>/dev/null >> "$bundle_list"
  done
  while IFS= read -r bundle_dir; do
    remove_viewer_bundle_copies_in_dir "$bundle_dir"
  done < "$bundle_list"
  rm -f "$bundle_list"
}

clean_numbered_viewer_bundles() {
  bundle_list="$(mktemp "${TMPDIR:-/tmp}/talos-viewer-bundle-dirs.XXXXXX")"
  for root in "$APPS_ROOT/target" "$REPO_ROOT/target" "$VIEWER_ROOT/src-tauri/target"; do
    if [ ! -d "$root" ]; then
      continue
    fi
    find "$root" -path "*/bundle/macos" -type d -print 2>/dev/null >> "$bundle_list"
  done
  while IFS= read -r bundle_dir; do
    remove_numbered_viewer_bundles_in_dir "$bundle_dir"
  done < "$bundle_list"
  rm -f "$bundle_list"
}

update_manifest() {
  manifest_path="$ARTIFACT_DIR/manifest.json"
  package_sha="$(sha256_file "$PKG_PATH")"
  package_size="$(file_size "$PKG_PATH")"
  generated_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  if [ -f "$manifest_path" ]; then
    tmp_manifest="$manifest_path.tmp"
    bun -e '
const fs = require("fs");
const [manifestPath, tmpPath, profile, generatedAtUtc, fileName, sizeBytes, sha256] = process.argv.slice(1);
let manifest = {};
try {
  manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
} catch {
  manifest = {};
}
manifest.profile = manifest.profile || profile;
manifest.generatedAtUtc = generatedAtUtc;
manifest.viewer = manifest.viewer || {};
manifest.viewer.macosInstaller = {
  fileName,
  sizeBytes: Number(sizeBytes),
  sha256
};
manifest.updates = manifest.updates || {};
for (const arch of ["macos-arm64", "macos-x64"]) {
  const key = arch === "macos-arm64" ? "viewerMacosArm64" : "viewerMacosX64";
  manifest.updates[key] = {
    manifest: artifact(`Talos.Viewer.${arch}.Update.manifest.json`),
    signature: artifact(`Talos.Viewer.${arch}.Update.manifest.sig`),
    package: artifact(fileName)
  };
}
fs.writeFileSync(tmpPath, `${JSON.stringify(manifest, null, 2)}\n`);

function artifact(name) {
  const p = require("path").join(require("path").dirname(manifestPath), name);
  const st = fs.statSync(p);
  const crypto = require("crypto");
  return {
    fileName: name,
    sizeBytes: st.size,
    sha256: crypto.createHash("sha256").update(fs.readFileSync(p)).digest("hex")
  };
}
' "$manifest_path" "$tmp_manifest" "$BUILD_PROFILE" "$generated_at" "$PKG_FILE_NAME" "$package_size" "$package_sha"
    mv "$tmp_manifest" "$manifest_path"
  else
    cat > "$manifest_path" <<EOF_MANIFEST
{
  "profile": "$BUILD_PROFILE",
  "generatedAtUtc": "$generated_at",
  "viewer": {
    "macosInstaller": {
      "fileName": "$PKG_FILE_NAME",
      "sizeBytes": $package_size,
      "sha256": "$package_sha"
    }
  },
  "updates": {
    "viewerMacosArm64": {
      "manifest": {
        "fileName": "Talos.Viewer.macos-arm64.Update.manifest.json",
        "sizeBytes": $(file_size "$ARTIFACT_DIR/Talos.Viewer.macos-arm64.Update.manifest.json"),
        "sha256": "$(sha256_file "$ARTIFACT_DIR/Talos.Viewer.macos-arm64.Update.manifest.json")"
      },
      "signature": {
        "fileName": "Talos.Viewer.macos-arm64.Update.manifest.sig",
        "sizeBytes": $(file_size "$ARTIFACT_DIR/Talos.Viewer.macos-arm64.Update.manifest.sig"),
        "sha256": "$(sha256_file "$ARTIFACT_DIR/Talos.Viewer.macos-arm64.Update.manifest.sig")"
      },
      "package": {
        "fileName": "$PKG_FILE_NAME",
        "sizeBytes": $package_size,
        "sha256": "$package_sha"
      }
    },
    "viewerMacosX64": {
      "manifest": {
        "fileName": "Talos.Viewer.macos-x64.Update.manifest.json",
        "sizeBytes": $(file_size "$ARTIFACT_DIR/Talos.Viewer.macos-x64.Update.manifest.json"),
        "sha256": "$(sha256_file "$ARTIFACT_DIR/Talos.Viewer.macos-x64.Update.manifest.json")"
      },
      "signature": {
        "fileName": "Talos.Viewer.macos-x64.Update.manifest.sig",
        "sizeBytes": $(file_size "$ARTIFACT_DIR/Talos.Viewer.macos-x64.Update.manifest.sig"),
        "sha256": "$(sha256_file "$ARTIFACT_DIR/Talos.Viewer.macos-x64.Update.manifest.sig")"
      },
      "package": {
        "fileName": "$PKG_FILE_NAME",
        "sizeBytes": $package_size,
        "sha256": "$package_sha"
      }
    }
  }
}
EOF_MANIFEST
  fi
}

build_app_bundle() {
  tauri_config_override="$(mktemp "${TMPDIR:-/tmp}/talos-viewer-tauri-config.json.XXXXXX")"
  cleanup_tauri_config() {
    rm -f "$tauri_config_override"
  }
  trap cleanup_tauri_config EXIT INT TERM
  cat > "$tauri_config_override" <<'EOF_TAURI_CONFIG'
{
  "build": {
    "beforeBuildCommand": "true"
  }
}
EOF_TAURI_CONFIG

  (
    cd "$APPS_ROOT"
    bun install --frozen-lockfile --filter talos_viewer --no-progress
  )
  (
    cd "$VIEWER_ROOT"
    ensure_frontend_dist
    clean_stale_viewer_bundles
    local_tauri_cli="$VIEWER_ROOT/node_modules/@tauri-apps/cli/tauri.js"
    local_tauri_manifest="$VIEWER_ROOT/node_modules/@tauri-apps/cli/package.json"
    if [ ! -f "$local_tauri_cli" ] || [ ! -f "$local_tauri_manifest" ]; then
      echo "The frozen talos_viewer @tauri-apps/cli dependency is missing; run bun install --frozen-lockfile from apps/." >&2
      exit 1
    fi
    local_tauri_version="$(sed -n 's/^[[:space:]]*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$local_tauri_manifest" | head -n 1)"
    if [ "$local_tauri_version" != "$TAURI_CLI_VERSION" ]; then
      echo "Expected local @tauri-apps/cli $TAURI_CLI_VERSION, found ${local_tauri_version:-unknown}." >&2
      exit 1
    fi
    bun --bun "$local_tauri_cli" build $(tauri_build_profile_args) --config "$tauri_config_override" --bundles "$TAURI_BUNDLES" $TAURI_BUILD_ARGS -- --locked
    clean_numbered_viewer_bundles
  )
  cleanup_tauri_config
  trap - EXIT INT TERM
}

build_pkg() {
  app_bundle="$1"
  version="$2"
  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/talos-viewer-pkg.XXXXXX")"
  cleanup_pkg_work_dir() {
    rm -rf "$work_dir"
  }
  trap cleanup_pkg_work_dir EXIT INT TERM
  pkg_root="$work_dir/root"
  staged_app="$pkg_root/Applications/$APP_NAME"
  component_pkg="$work_dir/component.pkg"
  component_plist="$work_dir/component.plist"
  pkg_scripts="$work_dir/scripts"
  rm -f "$PKG_PATH"
  mkdir -p "$pkg_root/Applications"
  ditto --norsrc "$app_bundle" "$staged_app"
  xattr -cr "$pkg_root" 2>/dev/null || true
  find "$pkg_root" -name '._*' -type f -delete 2>/dev/null || true
  mkdir -p "$pkg_scripts"
  cat > "$component_plist" <<EOF_COMPONENT_PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>RootRelativeBundlePath</key>
    <string>Applications/$APP_NAME</string>
    <key>BundleIsRelocatable</key>
    <false/>
    <key>BundleIsVersionChecked</key>
    <false/>
    <key>BundleHasStrictIdentifier</key>
    <true/>
    <key>BundleOverwriteAction</key>
    <string>upgrade</string>
  </dict>
</array>
</plist>
EOF_COMPONENT_PLIST
  cat > "$pkg_scripts/preinstall" <<EOF_PREINSTALL
#!/bin/sh
set -u

find /Applications -maxdepth 1 -type d \\( \\
  -name "Talos Viewer.app" -o \\
  -name "Talos Viewer [0-9]*.app" \\
\\) -exec rm -rf {} + >/dev/null 2>&1 || true

exit 0
EOF_PREINSTALL
  cat > "$pkg_scripts/postinstall" <<EOF_POSTINSTALL
#!/bin/sh
set -u

APP_PATH="/Applications/$APP_NAME"
LOG_DIR="/Library/Logs/Talos"
LOG_PATH="\$LOG_DIR/talos_viewer_pkg_install.log"

mkdir -p "\$LOG_DIR" >/dev/null 2>&1 || true
log() {
  printf '%s %s\\n' "\$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "\$*" >> "\$LOG_PATH" 2>/dev/null || true
}

find /Applications -maxdepth 1 -type d -name "Talos Viewer [0-9]*.app" -exec rm -rf {} + >/dev/null 2>&1 || true

if [ ! -d "\$APP_PATH" ]; then
  log "viewer app not found after install: \$APP_PATH"
  exit 0
fi

CONSOLE_UID="\$(/usr/bin/stat -f '%u' /dev/console 2>/dev/null || printf '0')"
CONSOLE_USER="\$(/usr/bin/stat -f '%Su' /dev/console 2>/dev/null || printf '')"
if [ "\$CONSOLE_UID" = "0" ] || [ "\$CONSOLE_USER" = "loginwindow" ] || [ -z "\$CONSOLE_USER" ]; then
  log "no active console user; skipping viewer launch"
  exit 0
fi

RELAUNCH_SCRIPT="/tmp/talos-viewer-relaunch-\$\$.sh"
cat > "\$RELAUNCH_SCRIPT" <<EOF_RELAUNCH
#!/bin/sh
set -u

APP_PATH="\$APP_PATH"
LOG_PATH="\$LOG_PATH"
CONSOLE_UID="\$CONSOLE_UID"
CONSOLE_USER="\$CONSOLE_USER"

log() {
  printf '%s %s\\n' "\$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "\$*" >> "\$LOG_PATH" 2>/dev/null || true
}

sleep 2
attempt=1
while [ "\$attempt" -le 10 ]; do
  if [ -d "\$APP_PATH" ]; then
    if /bin/launchctl asuser "\$CONSOLE_UID" /usr/bin/sudo -u "\$CONSOLE_USER" /usr/bin/open -na "\$APP_PATH" >/dev/null 2>&1; then
      log "launched viewer after install for \$CONSOLE_USER on attempt \$attempt"
      rm -f "\$0"
      exit 0
    fi
    if /bin/launchctl asuser "\$CONSOLE_UID" /usr/bin/open -na "\$APP_PATH" >/dev/null 2>&1; then
      log "launched viewer after install via launchctl fallback on attempt \$attempt"
      rm -f "\$0"
      exit 0
    fi
  fi
  sleep 1
  attempt=\$((attempt + 1))
done

log "failed to launch viewer after install for \$CONSOLE_USER"
rm -f "\$0"
exit 0
EOF_RELAUNCH
chmod 0755 "\$RELAUNCH_SCRIPT"
nohup "\$RELAUNCH_SCRIPT" >/dev/null 2>&1 &
log "scheduled viewer relaunch for \$CONSOLE_USER"

exit 0
EOF_POSTINSTALL
  chmod 0755 "$pkg_scripts/preinstall"
  chmod 0755 "$pkg_scripts/postinstall"

  pkgbuild \
    --root "$pkg_root" \
    --install-location "/" \
    --component-plist "$component_plist" \
    --scripts "$pkg_scripts" \
    --identifier "$PKG_IDENTIFIER" \
    --version "$version" \
    "$component_pkg"

  if [ -n "$PKG_SIGNING_IDENTITY" ]; then
    if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
      productbuild --package "$component_pkg" --sign "$PKG_SIGNING_IDENTITY" --keychain "$MACOS_SIGNING_KEYCHAIN" "$PKG_PATH"
    else
      productbuild --package "$component_pkg" --sign "$PKG_SIGNING_IDENTITY" "$PKG_PATH"
    fi
  else
    productbuild --package "$component_pkg" "$PKG_PATH"
  fi

  cleanup_pkg_work_dir
  trap - EXIT INT TERM
}

parse_args "$@"
echo "Using macOS Cargo profile: $MACOS_CARGO_PROFILE"

require_macos
require_command bun
require_command ditto
require_command find
require_command mktemp
require_command pkgbuild
require_command productbuild
require_command openssl
require_command security
require_command codesign
require_command shasum
require_command stat

if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
  MACOS_SIGNING_KEYCHAIN="$(normalize_keychain_path "$MACOS_SIGNING_KEYCHAIN")"
  if [ ! -f "$MACOS_SIGNING_KEYCHAIN" ]; then
    echo "macOS signing keychain not found: $MACOS_SIGNING_KEYCHAIN" >&2
    echo "Set MACOS_SIGNING_KEYCHAIN to a real keychain path, for example: $HOME/Library/Keychains/login.keychain-db" >&2
    exit 1
  fi
fi

mkdir -p "$ARTIFACT_DIR"
require_manifest_public_key
resolve_codesign_identity
export RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH="$MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH"
viewer_version="$(package_version "$VIEWER_ROOT/src-tauri/Cargo.toml")"
if [ -z "$viewer_version" ]; then
  echo "Unable to read Talos Viewer version." >&2
  exit 1
fi

build_app_bundle
app_bundle="$(find_app_bundle)"
codesign_app_bundle "$app_bundle"
build_pkg "$app_bundle" "$viewer_version"
write_update_manifest "macos-arm64" "$viewer_version"
write_update_manifest "macos-x64" "$viewer_version"
update_manifest

echo "Built Talos Viewer macOS package:"
echo "  $PKG_PATH"
