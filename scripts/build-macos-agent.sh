#!/bin/sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
APPS_ROOT="$REPO_ROOT/apps"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$APPS_ROOT/installer/artifacts/$BUILD_PROFILE}"
UNIVERSAL_DIR="$ARTIFACT_DIR/macos-universal"
CERTS_DIR="${MACOS_CERTS_DIR:-$APPS_ROOT/certs}"
SIGNING_DIR="${MACOS_SIGNING_DIR:-$CERTS_DIR}"
MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH="${RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH:-$CERTS_DIR/talos-manifest-signing.key.pem}"
MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH="${RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH:-$CERTS_DIR/talos-manifest-signing-public.der}"
MANIFEST_SIGNING_PFX_PATH="${RMM_MANIFEST_SIGNING_PFX_PATH:-$CERTS_DIR/Talos Manifest Signing.pfx}"
MACOS_CARGO_TARGET_DIR="${MACOS_CARGO_TARGET_DIR:-/tmp/talos-macos-cargo-target}"
MACOS_CARGO_PROFILE="${MACOS_CARGO_PROFILE:-debug}"
MACOS_RUSTUP_TOOLCHAIN="${MACOS_RUSTUP_TOOLCHAIN:-1.95.0}"
MACOS_DEPS_DIR="${MACOS_DEPS_DIR:-$MACOS_CARGO_TARGET_DIR/macos-deps}"
DEFAULT_MACOS_LIBVPX_VERSION="1.13.0"
DEFAULT_MACOS_LIBVPX_URL="https://github.com/webmproject/libvpx/archive/refs/tags/v1.13.0.tar.gz"
# Digest of the GitHub tag archive reviewed on 2026-08-17.
DEFAULT_MACOS_LIBVPX_SHA256="cb2a393c9c1fae7aba76b950bb0ad393ba105409fe1a147ccd61b0aaa1501066"
MACOS_LIBVPX_VERSION="${MACOS_LIBVPX_VERSION:-$DEFAULT_MACOS_LIBVPX_VERSION}"
MACOS_LIBVPX_URL="${MACOS_LIBVPX_URL:-https://github.com/webmproject/libvpx/archive/refs/tags/v$MACOS_LIBVPX_VERSION.tar.gz}"
if [ -n "${MACOS_LIBVPX_SHA256:-}" ]; then
  MACOS_LIBVPX_SHA256="$(printf '%s' "$MACOS_LIBVPX_SHA256" | tr '[:upper:]' '[:lower:]')"
elif [ "$MACOS_LIBVPX_VERSION" = "$DEFAULT_MACOS_LIBVPX_VERSION" ] && [ "$MACOS_LIBVPX_URL" = "$DEFAULT_MACOS_LIBVPX_URL" ]; then
  MACOS_LIBVPX_SHA256="$DEFAULT_MACOS_LIBVPX_SHA256"
else
  MACOS_LIBVPX_SHA256=""
fi
MACOS_SIGNING_KEYCHAIN="${MACOS_SIGNING_KEYCHAIN:-}"
MACOS_CODESIGN_IDENTITY="${MACOS_CODESIGN_IDENTITY:-}"
MACOS_PKG_SIGNING_IDENTITY="${MACOS_PKG_SIGNING_IDENTITY:-}"
MACOS_NODE_BIN="${MACOS_NODE_BIN:-$(command -v node 2>/dev/null || true)}"
DEFAULT_CODESIGN_CERT_SHA256="57614DC858DC4A04D2870A53E138C5249AA174B350C53239FDD6142BAF3C2253"
MACOS_CODESIGN_CERT_SHA256="${MACOS_CODESIGN_CERT_SHA256:-$DEFAULT_CODESIGN_CERT_SHA256}"
PKG_IDENTIFIER="${MACOS_PKG_IDENTIFIER:-com.talos.agent}"
PKG_FILE_NAME="${MACOS_PKG_FILE_NAME:-Talos.Agent.macos-universal.pkg}"
PKG_PATH="$ARTIFACT_DIR/$PKG_FILE_NAME"
TARGET_DIR="$MACOS_CARGO_TARGET_DIR"
UNIVERSAL_BIN_DIR="$ARTIFACT_DIR/macos-universal-bin"
MACOS_ICON_ASSET_DIR="$APPS_ROOT/installer/assets/macos"
APP_ICON_FILE_NAME="talos-server-icon.icns"
APP_ICON_SOURCE="$MACOS_ICON_ASSET_DIR/$APP_ICON_FILE_NAME"
SUPERVISOR_APP_NAME="Talos Supervisor.app"
WORKER_APP_NAME="Talos Worker.app"
WORKER_HELPER_APP_NAME="Talos Worker Helper.app"
WORKER_CHAT_APP_NAME="Talos Worker Chat.app"
PERMISSIONS_HELPER_APP_NAME="Talos Permissions Helper.app"
PERMISSION_FLOW_RESOURCE_BUNDLE_NAME="PermissionFlow_PermissionFlow.bundle"
PERMISSION_FLOW_RESOURCE_BUNDLE_SOURCE=""
SUPERVISOR_BUNDLE_IDENTIFIER="${MACOS_SUPERVISOR_BUNDLE_IDENTIFIER:-com.talos.supervisor}"
WORKER_BUNDLE_IDENTIFIER="${MACOS_WORKER_BUNDLE_IDENTIFIER:-com.talos.worker}"
WORKER_HELPER_BUNDLE_IDENTIFIER="${MACOS_WORKER_HELPER_BUNDLE_IDENTIFIER:-com.talos.worker-helper}"
WORKER_CHAT_BUNDLE_IDENTIFIER="${MACOS_WORKER_CHAT_BUNDLE_IDENTIFIER:-com.talos.worker-chat}"
PERMISSIONS_HELPER_BUNDLE_IDENTIFIER="${MACOS_PERMISSIONS_HELPER_BUNDLE_IDENTIFIER:-com.talos.permissions-helper}"

export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export COPYFILE_DISABLE=1
export DITTONORSRC=1
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
if [ -n "$MACOS_NODE_BIN" ]; then
  export TALOS_FRONTEND_NODE="${TALOS_FRONTEND_NODE:-$MACOS_NODE_BIN}"
fi

require_macos() {
  if [ "$(uname -s)" != "Darwin" ]; then
    echo "build-macos-agent.sh must be run on macOS." >&2
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
  --debug      Build Cargo debug binaries (default)
  --release    Build Cargo release binaries
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

run_cargo() {
  cargo_subcommand="${1:-}"
  if [ "$cargo_subcommand" = "build" ]; then
    shift
    RUSTC="$(rustup which --toolchain "$MACOS_RUSTUP_TOOLCHAIN" rustc)" \
      rustup run "$MACOS_RUSTUP_TOOLCHAIN" cargo build --locked "$@"
    return
  fi
  RUSTC="$(rustup which --toolchain "$MACOS_RUSTUP_TOOLCHAIN" rustc)" \
    rustup run "$MACOS_RUSTUP_TOOLCHAIN" cargo "$@"
}

require_rustup_toolchain() {
  if ! rustup run "$MACOS_RUSTUP_TOOLCHAIN" cargo --version >/dev/null 2>&1; then
    echo "Rustup toolchain '$MACOS_RUSTUP_TOOLCHAIN' is required." >&2
    echo "Install it with: rustup toolchain install $MACOS_RUSTUP_TOOLCHAIN" >&2
    exit 1
  fi
}

append_rustflags() {
  if [ -n "${RUSTFLAGS:-}" ]; then
    export RUSTFLAGS="$RUSTFLAGS $1"
  else
    export RUSTFLAGS="$1"
  fi
}

configure_swift_runtime_link_path() {
  swiftc_path="$(xcrun --find swiftc 2>/dev/null || true)"
  if [ -z "$swiftc_path" ]; then
    return
  fi

  swift_usr_dir="$(CDPATH= cd -- "$(dirname -- "$swiftc_path")/.." && pwd)"
  swift_runtime_dir="$swift_usr_dir/lib/swift/macosx"
  if [ -d "$swift_runtime_dir" ]; then
    append_rustflags "-L native=$swift_runtime_dir"
  fi
}

run_vite_build() {
  app_name="$1"
  log_path="$2"
  force_build="${3:-0}"
  if [ "$force_build" = "1" ]; then
    echo "Rebuilding $app_name frontend assets..." >&2
    rm -rf dist
  elif [ -f "dist/index.html" ] && [ "${MACOS_FORCE_FRONTEND_BUILD:-0}" != "1" ]; then
    echo "Using existing $app_name dist. Set MACOS_FORCE_FRONTEND_BUILD=1 to rebuild frontend assets." | tee "$log_path" >&2
    return
  fi
  if [ ! -f "./node_modules/vite/bin/vite.js" ]; then
    echo "vite is required to build $app_name frontend; run bun install --cwd $APPS_ROOT" >&2
    exit 1
  fi
  if ! CI=1 bun --bun ./node_modules/vite/bin/vite.js build >"$log_path" 2>&1; then
    cat "$log_path" >&2
    exit 1
  fi
}

build_permissions_helper_frontend() {
  (
    cd "$APPS_ROOT"
    bun install --frozen-lockfile --filter talos_permissions_helper --no-progress
  )
  (
    cd "$APPS_ROOT/talos_permissions_helper"
    if [ -d "node_modules/@rollup" ]; then
      find "node_modules/@rollup" -name '*.node' -type f -exec codesign --force --sign "$SIGNING_IDENTITY" {} \; >/dev/null 2>&1 || true
    fi
    log_path="$ARTIFACT_DIR/talos_permissions_helper-frontend-build.log"
    run_vite_build "talos_permissions_helper" "$log_path" "1"
  )
}

build_worker_chat_frontend() {
  (
    cd "$APPS_ROOT"
    bun install --frozen-lockfile --filter talos_worker_chat --no-progress
  )
  (
    cd "$APPS_ROOT/talos_worker_chat"
    if [ -d "node_modules/@rollup" ]; then
      find "node_modules/@rollup" -name '*.node' -type f -exec codesign --force --sign "$SIGNING_IDENTITY" {} \; >/dev/null 2>&1 || true
    fi
    log_path="$ARTIFACT_DIR/talos_worker_chat-frontend-build.log"
    run_vite_build "talos_worker_chat" "$log_path" "1"
  )
}

make_jobs() {
  jobs="$(sysctl -n hw.ncpu 2>/dev/null || true)"
  if [ -n "$jobs" ]; then
    printf '%s\n' "$jobs"
  else
    printf '%s\n' "4"
  fi
}

libvpx_arch_for_target() {
  case "$1" in
    aarch64-apple-darwin)
      printf '%s\n' "arm64"
      ;;
    x86_64-apple-darwin)
      printf '%s\n' "x86_64"
      ;;
    *)
      echo "Unsupported macOS target for libvpx: $1" >&2
      exit 1
      ;;
  esac
}

require_macos_libvpx_digest() {
  if [ "${#MACOS_LIBVPX_SHA256}" -ne 64 ] || printf '%s' "$MACOS_LIBVPX_SHA256" | grep -q '[^0-9a-f]'; then
    echo "MACOS_LIBVPX_SHA256 must be an explicit 64-character SHA-256 digest when MACOS_LIBVPX_VERSION or MACOS_LIBVPX_URL is overridden." >&2
    exit 1
  fi
}

verify_macos_libvpx_archive() {
  archive_path="$1"
  actual_sha256="$(sha256_file "$archive_path" | tr '[:upper:]' '[:lower:]')"
  if [ "$actual_sha256" != "$MACOS_LIBVPX_SHA256" ]; then
    echo "libvpx source archive SHA-256 mismatch: $archive_path" >&2
    echo "  expected: $MACOS_LIBVPX_SHA256" >&2
    echo "  actual:   $actual_sha256" >&2
    return 1
  fi
}

ensure_macos_libvpx() {
  target="$1"
  require_macos_libvpx_digest
  arch="$(libvpx_arch_for_target "$target")"
  prefix="$MACOS_DEPS_DIR/libvpx-$MACOS_LIBVPX_VERSION-$target"
  archive="$MACOS_DEPS_DIR/src/libvpx-$MACOS_LIBVPX_VERSION.tar.gz"
  build_dir="$MACOS_DEPS_DIR/build/libvpx-$MACOS_LIBVPX_VERSION-$target"
  source_stamp="$prefix/.talos-source-sha256"
  mkdir -p "$MACOS_DEPS_DIR/src" "$MACOS_DEPS_DIR/build"

  if [ -f "$archive" ]; then
    verify_macos_libvpx_archive "$archive" || exit 1
  fi

  if [ -f "$prefix/lib/libvpx.a" ] && [ -f "$prefix/include/vpx/vpx_encoder.h" ] && [ -f "$source_stamp" ] && [ "$(tr -d '[:space:]' < "$source_stamp")" = "$MACOS_LIBVPX_SHA256" ]; then
    if lipo -info "$prefix/lib/libvpx.a" 2>/dev/null | grep -F "$arch" >/dev/null; then
      VPX_PREFIX="$prefix"
      return
    fi
    rm -rf "$prefix"
  fi

  if [ ! -f "$archive" ]; then
    echo "Downloading libvpx $MACOS_LIBVPX_VERSION for macOS builds..." >&2
    rm -f "$archive.tmp"
    curl -L --fail --retry 2 -o "$archive.tmp" "$MACOS_LIBVPX_URL"
    if ! verify_macos_libvpx_archive "$archive.tmp"; then
      rm -f "$archive.tmp"
      exit 1
    fi
    mv "$archive.tmp" "$archive"
  fi

  echo "Building static libvpx $MACOS_LIBVPX_VERSION for $target..." >&2
  rm -rf "$build_dir" "$prefix"
  mkdir -p "$build_dir"
  tar -xzf "$archive" -C "$build_dir" --strip-components 1
  (
    cd "$build_dir"
    CC="clang -arch $arch" CXX="clang++ -arch $arch" ./configure \
      --prefix="$prefix" \
      --target=generic-gnu \
      --disable-examples \
      --disable-tools \
      --disable-docs \
      --disable-unit-tests \
      --disable-vp9 \
      --enable-vp8 \
      --enable-static \
      --disable-shared
    make -j"$(make_jobs)"
    make install
  )
  printf '%s\n' "$MACOS_LIBVPX_SHA256" > "$source_stamp"

  if ! lipo -info "$prefix/lib/libvpx.a" 2>/dev/null | grep -F "$arch" >/dev/null; then
    echo "Built libvpx archive does not contain expected architecture '$arch': $prefix/lib/libvpx.a" >&2
    exit 1
  fi

  VPX_PREFIX="$prefix"
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

copy_bundle_without_metadata() {
  source_path="$1"
  destination_path="$2"
  ditto --norsrc "$source_path" "$destination_path"
}

remove_bundle_path() {
  bundle="$1"
  if [ ! -e "$bundle" ]; then
    return
  fi
  chmod -R u+w "$bundle" >/dev/null 2>&1 || true
  rm -rf "$bundle" >/dev/null 2>&1 || true
  if [ -e "$bundle" ]; then
    echo "Unable to remove stale app bundle: $bundle" >&2
    echo "Check ownership/permissions, then remove it before rebuilding." >&2
    exit 1
  fi
}

remove_agent_app_bundle_copies_in_dir() {
  bundle_dir="$1"
  if [ ! -d "$bundle_dir" ]; then
    return
  fi
  for bundle in \
    "$bundle_dir"/Talos\ Supervisor.app \
    "$bundle_dir"/Talos\ Supervisor\ [0-9]*.app \
    "$bundle_dir"/Talos\ Worker.app \
    "$bundle_dir"/Talos\ Worker\ [0-9]*.app \
    "$bundle_dir"/Talos\ Worker\ Helper.app \
    "$bundle_dir"/Talos\ Worker\ Helper\ [0-9]*.app \
    "$bundle_dir"/Talos\ Worker\ Chat.app \
    "$bundle_dir"/Talos\ Worker\ Chat\ [0-9]*.app \
    "$bundle_dir"/Talos\ Permissions\ Helper.app \
    "$bundle_dir"/Talos\ Permissions\ Helper\ [0-9]*.app
  do
    remove_bundle_path "$bundle"
  done
}

remove_numbered_agent_app_bundles_in_dir() {
  bundle_dir="$1"
  if [ ! -d "$bundle_dir" ]; then
    return
  fi
  for bundle in \
    "$bundle_dir"/Talos\ Supervisor\ [0-9]*.app \
    "$bundle_dir"/Talos\ Worker\ [0-9]*.app \
    "$bundle_dir"/Talos\ Worker\ Helper\ [0-9]*.app \
    "$bundle_dir"/Talos\ Worker\ Chat\ [0-9]*.app \
    "$bundle_dir"/Talos\ Permissions\ Helper\ [0-9]*.app
  do
    remove_bundle_path "$bundle"
  done
}

clean_agent_app_bundles() {
  remove_agent_app_bundle_copies_in_dir "$ARTIFACT_DIR/macos-arm64"
  remove_agent_app_bundle_copies_in_dir "$ARTIFACT_DIR/macos-x64"
  remove_agent_app_bundle_copies_in_dir "$UNIVERSAL_DIR"
  remove_agent_app_bundle_copies_in_dir "$ARTIFACT_DIR/macos-pkg-root/Library/Talos/Supervisor"
  remove_agent_app_bundle_copies_in_dir "$ARTIFACT_DIR/macos-pkg-root/Applications"
  remove_agent_app_bundle_copies_in_dir "$ARTIFACT_DIR/macos-pkg-root/Library/Talos/Worker"
}

clean_numbered_agent_app_bundles() {
  remove_numbered_agent_app_bundles_in_dir "$ARTIFACT_DIR/macos-arm64"
  remove_numbered_agent_app_bundles_in_dir "$ARTIFACT_DIR/macos-x64"
  remove_numbered_agent_app_bundles_in_dir "$UNIVERSAL_DIR"
  remove_numbered_agent_app_bundles_in_dir "$ARTIFACT_DIR/macos-pkg-root/Library/Talos/Supervisor"
  remove_numbered_agent_app_bundles_in_dir "$ARTIFACT_DIR/macos-pkg-root/Applications"
  remove_numbered_agent_app_bundles_in_dir "$ARTIFACT_DIR/macos-pkg-root/Library/Talos/Worker"
}

remove_loose_macos_binaries_in_dir() {
  dir="$1"
  rm -f \
    "$dir/talos_supervisor" \
    "$dir/talos_worker" \
    "$dir/talos_worker_helper" \
    "$dir/talos_worker_chat" \
    "$dir/talos_permissions_helper" \
    "$dir/talos_supervisor 2" \
    "$dir/talos_worker 2"
}

clean_loose_macos_artifact_binaries() {
  rm -rf "$UNIVERSAL_BIN_DIR"
  remove_loose_macos_binaries_in_dir "$ARTIFACT_DIR/macos-arm64"
  remove_loose_macos_binaries_in_dir "$ARTIFACT_DIR/macos-x64"
  remove_loose_macos_binaries_in_dir "$UNIVERSAL_DIR"
  remove_loose_macos_binaries_in_dir "$ARTIFACT_DIR/macos-pkg-root/Library/Talos/Supervisor"
  remove_loose_macos_binaries_in_dir "$ARTIFACT_DIR/macos-pkg-root/Library/Talos/Worker"
  remove_loose_macos_binaries_in_dir "$ARTIFACT_DIR/macos-pkg-root/Applications"
}

signing_private_key_path() {
  printf '%s\n' "$MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH"
}

signing_public_key_path() {
  printf '%s\n' "$MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH"
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
  key_path="$(signing_private_key_path)"
  if [ -f "$key_path" ]; then
    printf '%s\n' "$key_path"
    return
  fi

  tmp_key="$(mktemp "${TMPDIR:-/tmp}/talos-manifest-key.XXXXXX")"
  if extract_manifest_signing_key "$tmp_key"; then
    printf '%s\n' "$tmp_key"
    return
  fi

  rm -f "$tmp_key"
  echo "Manifest signing private key not found: $key_path" >&2
  echo "Expected signing material in apps/certs. Add talos-manifest-signing.key.pem, set RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH, or set RMM_MANIFEST_SIGNING_PFX_PASSWORD for: $MANIFEST_SIGNING_PFX_PATH" >&2
  exit 1
}

normalize_keychain_path() {
  value="$(printf '%s' "$1" | tr -d '"' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  if [ -z "$value" ]; then
    printf '%s\n' "$HOME/Library/Keychains/login.keychain-db"
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

  tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/talos-codesign-certs.XXXXXX")"
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
  if [ -n "${MACOS_SIGNING_IDENTITY:-}" ]; then
    SIGNING_IDENTITY="$MACOS_SIGNING_IDENTITY"
    PKG_SIGNING_IDENTITY="$MACOS_PKG_SIGNING_IDENTITY"
    return
  fi

  if [ -n "$MACOS_CODESIGN_IDENTITY" ]; then
    SIGNING_IDENTITY="$MACOS_CODESIGN_IDENTITY"
  else
    SIGNING_IDENTITY="$(find_codesign_identity_by_sha256 "$MACOS_CODESIGN_CERT_SHA256")"
  fi
  PKG_SIGNING_IDENTITY="$MACOS_PKG_SIGNING_IDENTITY"
  echo "Using macOS code signing identity: $SIGNING_IDENTITY"
}

manifest_contents_json() {
  first=1
  for content in "$@"; do
    if [ "$first" -eq 1 ]; then
      first=0
    else
      printf ',\n'
    fi
    printf '    "%s"' "$content"
  done
  printf '\n'
}

write_manifest() {
  product="$1"
  arch="$2"
  version="$3"
  package_file="$4"
  package_path="$5"
  manifest_path="$6"
  shift 6

  sha256="$(sha256_file "$package_path")"
  size_bytes="$(file_size "$package_path")"
  published_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  contents_json="$(manifest_contents_json "$@")"

  cat > "$manifest_path" <<EOF_MANIFEST
{
  "product": "$product",
  "platform": "macos",
  "arch": "$arch",
  "channel": "stable",
  "version": "$version",
  "minimumSupportedVersion": "$version",
  "severity": "normal",
  "publishedAtUtc": "$published_at",
  "rolloutPercentage": 100,
  "package": {
    "fileName": "$package_file",
    "sizeBytes": $size_bytes,
    "sha256": "$sha256"
  },
  "contents": [
$contents_json
  ],
  "requiresRestart": true,
  "installMode": "zip"
}
EOF_MANIFEST
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

build_target() {
  target="$1"
  rustup target add --toolchain "$MACOS_RUSTUP_TOOLCHAIN" "$target"
  ensure_macos_libvpx "$target"
  export VPX_LIB_DIR="$VPX_PREFIX/lib"
  export VPX_INCLUDE_DIR="$VPX_PREFIX/include"
  export VPX_VERSION="$MACOS_LIBVPX_VERSION"
  export VPX_STATIC=1
  echo "Cleaning talos_permissions_helper artifacts for $target so rebuilt frontend assets are embedded..." >&2
  (cd "$APPS_ROOT" && run_cargo clean -p talos_permissions_helper --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR")
  if [ "$MACOS_CARGO_PROFILE" = "release" ]; then
    (cd "$APPS_ROOT" && run_cargo build -p talos_supervisor --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR" --release)
    (cd "$APPS_ROOT" && run_cargo build -p talos_worker --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR" --release)
    (cd "$APPS_ROOT" && run_cargo build -p talos_worker_helper --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR" --release)
    (cd "$APPS_ROOT" && run_cargo build -p talos_worker_chat --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR" --release)
    (cd "$APPS_ROOT" && run_cargo build -p talos_permissions_helper --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR" --release)
  else
    (cd "$APPS_ROOT" && run_cargo build -p talos_supervisor --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR")
    (cd "$APPS_ROOT" && run_cargo build -p talos_worker --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR")
    (cd "$APPS_ROOT" && run_cargo build -p talos_worker_helper --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR")
    (cd "$APPS_ROOT" && run_cargo build -p talos_worker_chat --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR")
    (cd "$APPS_ROOT" && run_cargo build -p talos_permissions_helper --target "$target" --target-dir "$MACOS_CARGO_TARGET_DIR")
  fi
}

codesign_binary() {
  binary_path="$1"
  if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
    codesign --force --sign "$SIGNING_IDENTITY" --keychain "$MACOS_SIGNING_KEYCHAIN" --timestamp=none "$binary_path"
  else
    codesign --force --sign "$SIGNING_IDENTITY" --timestamp=none "$binary_path"
  fi
}

codesign_app_bundle() {
  bundle_path="$1"
  if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
    codesign --force --deep --sign "$SIGNING_IDENTITY" --keychain "$MACOS_SIGNING_KEYCHAIN" --timestamp=none "$bundle_path"
  else
    codesign --force --deep --sign "$SIGNING_IDENTITY" --timestamp=none "$bundle_path"
  fi
}

write_app_info_plist() {
  plist_path="$1"
  bundle_identifier="$2"
  product_name="$3"
  executable_name="$4"
  version="$5"
  presentation_mode="$6"
  screen_capture_usage_description="${7:-}"
  accessibility_usage_description="${8:-}"
  mkdir -p "$(dirname "$plist_path")"
  cat > "$plist_path" <<EOF_PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>$executable_name</string>
  <key>CFBundleIdentifier</key>
  <string>$bundle_identifier</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleIconFile</key>
  <string>talos-server-icon</string>
  <key>CFBundleName</key>
  <string>$product_name</string>
  <key>CFBundleDisplayName</key>
  <string>$product_name</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSMinimumSystemVersion</key>
  <string>$MACOSX_DEPLOYMENT_TARGET</string>
EOF_PLIST
  if [ -n "$screen_capture_usage_description" ]; then
    cat >> "$plist_path" <<EOF_SCREEN_CAPTURE
  <key>NSScreenCaptureUsageDescription</key>
  <string>$screen_capture_usage_description</string>
EOF_SCREEN_CAPTURE
  fi
  if [ -n "$accessibility_usage_description" ]; then
    cat >> "$plist_path" <<EOF_ACCESSIBILITY
  <key>NSAccessibilityUsageDescription</key>
  <string>$accessibility_usage_description</string>
EOF_ACCESSIBILITY
  fi
  if [ "$presentation_mode" = "background" ]; then
    cat >> "$plist_path" <<'EOF_BACKGROUND'
  <key>LSBackgroundOnly</key>
  <true/>
EOF_BACKGROUND
  elif [ "$presentation_mode" = "agent" ]; then
    cat >> "$plist_path" <<'EOF_AGENT'
  <key>LSUIElement</key>
  <true/>
EOF_AGENT
  fi
  cat >> "$plist_path" <<'EOF_PLIST_END'
</dict>
</plist>
EOF_PLIST_END
}

install_app_icon() {
  app_path="$1"
  if [ ! -f "$APP_ICON_SOURCE" ]; then
    echo "Talos app icon not found: $APP_ICON_SOURCE" >&2
    echo "Generate the shared macOS icon assets before building." >&2
    exit 1
  fi
  install -m 0644 "$APP_ICON_SOURCE" "$app_path/Contents/Resources/$APP_ICON_FILE_NAME"
}

find_permission_flow_resource_bundle() {
  profile_dir="$1"
  for root in \
    "$MACOS_CARGO_TARGET_DIR/aarch64-apple-darwin/$profile_dir/build" \
    "$MACOS_CARGO_TARGET_DIR/x86_64-apple-darwin/$profile_dir/build" \
    "$MACOS_CARGO_TARGET_DIR"
  do
    if [ ! -d "$root" ]; then
      continue
    fi
    bundle_path="$(find "$root" -path "*/$PERMISSION_FLOW_RESOURCE_BUNDLE_NAME" -type d -print 2>/dev/null | sort | tail -n 1)"
    if [ -n "$bundle_path" ]; then
      printf '%s\n' "$bundle_path"
      return 0
    fi
  done

  echo "PermissionFlow resource bundle not found under $MACOS_CARGO_TARGET_DIR" >&2
  echo "Expected SwiftPM to emit $PERMISSION_FLOW_RESOURCE_BUNDLE_NAME while building talos_permissions_helper." >&2
  exit 1
}

install_permission_flow_resources() {
  app_path="$1"
  if [ -z "$PERMISSION_FLOW_RESOURCE_BUNDLE_SOURCE" ] || [ ! -d "$PERMISSION_FLOW_RESOURCE_BUNDLE_SOURCE" ]; then
    echo "PermissionFlow resource bundle source is missing: $PERMISSION_FLOW_RESOURCE_BUNDLE_SOURCE" >&2
    exit 1
  fi

  rm -rf "$app_path/Contents/Resources/$PERMISSION_FLOW_RESOURCE_BUNDLE_NAME"
  copy_bundle_without_metadata \
    "$PERMISSION_FLOW_RESOURCE_BUNDLE_SOURCE" \
    "$app_path/Contents/Resources/$PERMISSION_FLOW_RESOURCE_BUNDLE_NAME"
}

make_supervisor_app_bundle() {
  app_path="$1"
  supervisor_binary="$2"
  version="$3"
  rm -rf "$app_path"
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
  install -m 0755 "$supervisor_binary" "$app_path/Contents/MacOS/talos_supervisor"
  install_app_icon "$app_path"
  write_app_info_plist \
    "$app_path/Contents/Info.plist" \
    "$SUPERVISOR_BUNDLE_IDENTIFIER" \
    "Talos Supervisor" \
    "talos_supervisor" \
    "$version" \
    "background"
  printf 'APPL????' > "$app_path/Contents/PkgInfo"
  codesign_app_bundle "$app_path"
}

make_worker_app_bundle() {
  app_path="$1"
  worker_binary="$2"
  version="$3"
  rm -rf "$app_path"
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
  install -m 0755 "$worker_binary" "$app_path/Contents/MacOS/talos_worker"
  install_app_icon "$app_path"
  write_app_info_plist \
    "$app_path/Contents/Info.plist" \
    "$WORKER_BUNDLE_IDENTIFIER" \
    "Talos Worker" \
    "talos_worker" \
    "$version" \
    "background"
  printf 'APPL????' > "$app_path/Contents/PkgInfo"
  codesign_app_bundle "$app_path"
}

make_worker_helper_app_bundle() {
  app_path="$1"
  helper_binary="$2"
  version="$3"
  rm -rf "$app_path"
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
  install -m 0755 "$helper_binary" "$app_path/Contents/MacOS/talos_worker_helper"
  install_app_icon "$app_path"
  write_app_info_plist \
    "$app_path/Contents/Info.plist" \
    "$WORKER_HELPER_BUNDLE_IDENTIFIER" \
    "Talos Worker Helper" \
    "talos_worker_helper" \
    "$version" \
    "agent" \
    "Allow Talos to capture this Mac screen during approved remote desktop sessions." \
    "Allow Talos to send mouse and keyboard input during approved remote desktop sessions."
  printf 'APPL????' > "$app_path/Contents/PkgInfo"
  codesign_app_bundle "$app_path"
}

make_worker_chat_app_bundle() {
  app_path="$1"
  chat_binary="$2"
  version="$3"
  rm -rf "$app_path"
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
  install -m 0755 "$chat_binary" "$app_path/Contents/MacOS/talos_worker_chat"
  install_app_icon "$app_path"
  write_app_info_plist \
    "$app_path/Contents/Info.plist" \
    "$WORKER_CHAT_BUNDLE_IDENTIFIER" \
    "Talos Worker Chat" \
    "talos_worker_chat" \
    "$version" \
    "regular"
  printf 'APPL????' > "$app_path/Contents/PkgInfo"
  codesign_app_bundle "$app_path"
}

make_permissions_helper_app_bundle() {
  app_path="$1"
  helper_binary="$2"
  version="$3"
  rm -rf "$app_path"
  mkdir -p "$app_path/Contents/MacOS" "$app_path/Contents/Resources"
  install -m 0755 "$helper_binary" "$app_path/Contents/MacOS/talos_permissions_helper"
  install_app_icon "$app_path"
  install_permission_flow_resources "$app_path"
  write_app_info_plist \
    "$app_path/Contents/Info.plist" \
    "$PERMISSIONS_HELPER_BUNDLE_IDENTIFIER" \
    "Talos Permissions Helper" \
    "talos_permissions_helper" \
    "$version" \
    "regular"
  printf 'APPL????' > "$app_path/Contents/PkgInfo"
  codesign_app_bundle "$app_path"
}

make_supervisor_update_package() {
  arch="$1"
  version="$2"
  supervisor_app="$3"

  package_file="Talos.Supervisor.$arch.Update.zip"
  package_path="$ARTIFACT_DIR/$package_file"
  manifest_path="$ARTIFACT_DIR/Talos.Supervisor.$arch.Update.manifest.json"
  signature_path="$ARTIFACT_DIR/Talos.Supervisor.$arch.Update.manifest.sig"
  tmp_dir="$(mktemp -d)"
  rm -f "$package_path"
  copy_bundle_without_metadata "$supervisor_app" "$tmp_dir/$SUPERVISOR_APP_NAME"
  xattr -cr "$tmp_dir" >/dev/null 2>&1 || true
  find "$tmp_dir" -name '._*' -delete
  (cd "$tmp_dir" && zip -qry "$package_path" "$SUPERVISOR_APP_NAME")
  rm -rf "$tmp_dir"
  write_manifest "supervisor" "$arch" "$version" "$package_file" "$package_path" "$manifest_path" "$SUPERVISOR_APP_NAME"
  sign_manifest "$manifest_path" "$signature_path"
}

make_worker_update_package() {
  arch="$1"
  version="$2"
  worker_app="$3"
  worker_helper_app="$4"
  worker_chat_app="$5"
  permissions_helper_app="$6"

  package_file="Talos.Worker.$arch.Update.zip"
  package_path="$ARTIFACT_DIR/$package_file"
  manifest_path="$ARTIFACT_DIR/Talos.Worker.$arch.Update.manifest.json"
  signature_path="$ARTIFACT_DIR/Talos.Worker.$arch.Update.manifest.sig"
  tmp_dir="$(mktemp -d)"
  rm -f "$package_path"
  copy_bundle_without_metadata "$worker_app" "$tmp_dir/$WORKER_APP_NAME"
  copy_bundle_without_metadata "$worker_helper_app" "$tmp_dir/$WORKER_HELPER_APP_NAME"
  copy_bundle_without_metadata "$worker_chat_app" "$tmp_dir/$WORKER_CHAT_APP_NAME"
  copy_bundle_without_metadata "$permissions_helper_app" "$tmp_dir/$PERMISSIONS_HELPER_APP_NAME"
  xattr -cr "$tmp_dir" >/dev/null 2>&1 || true
  find "$tmp_dir" -name '._*' -delete
  (cd "$tmp_dir" && zip -qry "$package_path" "$WORKER_APP_NAME" "$WORKER_HELPER_APP_NAME" "$WORKER_CHAT_APP_NAME" "$PERMISSIONS_HELPER_APP_NAME")
  rm -rf "$tmp_dir"
  write_manifest "worker" "$arch" "$version" "$package_file" "$package_path" "$manifest_path" "$WORKER_APP_NAME" "$WORKER_HELPER_APP_NAME" "$WORKER_CHAT_APP_NAME" "$PERMISSIONS_HELPER_APP_NAME"
  sign_manifest "$manifest_path" "$signature_path"
}

update_artifact_manifest() {
  manifest_path="$ARTIFACT_DIR/manifest.json"
  tmp_manifest="$manifest_path.tmp"
  generated_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

  bun -e '
const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const [manifestPath, tmpPath, profile, generatedAtUtc] = process.argv.slice(1);
let manifest = {};
try {
  manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
} catch {
  manifest = {};
}

manifest.profile = manifest.profile || profile;
manifest.generatedAtUtc = generatedAtUtc;
manifest.updates = manifest.updates || {};

for (const entry of [
  ["workerMacosArm64", "Worker", "macos-arm64"],
  ["workerMacosX64", "Worker", "macos-x64"],
  ["supervisorMacosArm64", "Supervisor", "macos-arm64"],
  ["supervisorMacosX64", "Supervisor", "macos-x64"]
]) {
  const [key, productTitle, arch] = entry;
  const prefix = `Talos.${productTitle}.${arch}.Update`;
  manifest.updates[key] = {
    package: artifact(`${prefix}.zip`),
    manifest: artifact(`${prefix}.manifest.json`),
    signature: artifact(`${prefix}.manifest.sig`)
  };
}

fs.writeFileSync(tmpPath, `${JSON.stringify(manifest, null, 2)}\n`);

function artifact(name) {
  const artifactPath = path.join(path.dirname(manifestPath), name);
  const bytes = fs.readFileSync(artifactPath);
  return {
    fileName: name,
    sizeBytes: fs.statSync(artifactPath).size,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex")
  };
}
' "$manifest_path" "$tmp_manifest" "$BUILD_PROFILE" "$generated_at"
  mv "$tmp_manifest" "$manifest_path"
}

write_pkg_file() {
  path="$1"
  mode="$2"
  shift 2
  mkdir -p "$(dirname "$path")"
  cat > "$path"
  chmod "$mode" "$path"
}

build_pkg_root() {
  pkg_root="$1"
  pkg_scripts="$2"
  rm -rf "$pkg_root" "$pkg_scripts"
  mkdir -p "$pkg_root/Library/Talos/Supervisor" \
    "$pkg_root/Library/Talos/Worker" \
    "$pkg_root/Library/LaunchDaemons" \
    "$pkg_root/Library/LaunchAgents" \
    "$pkg_root/Applications" \
    "$pkg_scripts"

  copy_bundle_without_metadata "$UNIVERSAL_DIR/$SUPERVISOR_APP_NAME" "$pkg_root/Library/Talos/Supervisor/$SUPERVISOR_APP_NAME"
  copy_bundle_without_metadata "$UNIVERSAL_DIR/$WORKER_APP_NAME" "$pkg_root/Library/Talos/Worker/$WORKER_APP_NAME"
  copy_bundle_without_metadata "$UNIVERSAL_DIR/$WORKER_HELPER_APP_NAME" "$pkg_root/Library/Talos/Worker/$WORKER_HELPER_APP_NAME"
  copy_bundle_without_metadata "$UNIVERSAL_DIR/$WORKER_CHAT_APP_NAME" "$pkg_root/Library/Talos/Worker/$WORKER_CHAT_APP_NAME"
  copy_bundle_without_metadata "$UNIVERSAL_DIR/$PERMISSIONS_HELPER_APP_NAME" "$pkg_root/Applications/$PERMISSIONS_HELPER_APP_NAME"
  find "$pkg_root" -name '._*' -delete

  write_pkg_file "$pkg_root/Library/Talos/Supervisor/run-talos-supervisor.sh" 0755 <<'EOF_SUPERVISOR_RUNNER'
#!/bin/sh
set -eu

ENV_FILE="/Library/Preferences/Talos/talos-supervisor.env"
if [ -f "$ENV_FILE" ]; then
  set -a
  . "$ENV_FILE"
  set +a
fi

exec "/Library/Talos/Supervisor/Talos Supervisor.app/Contents/MacOS/talos_supervisor"
EOF_SUPERVISOR_RUNNER

  write_pkg_file "$pkg_root/Library/LaunchAgents/com.talos.permissions-helper.plist" 0644 <<'EOF_PERMISSIONS_HELPER_PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.talos.permissions-helper</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Applications/Talos Permissions Helper.app/Contents/MacOS/talos_permissions_helper</string>
    <string>--login-check</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  <string>/tmp/talos_permissions_helper.launchd.out.log</string>
  <key>StandardErrorPath</key>
  <string>/tmp/talos_permissions_helper.launchd.err.log</string>
</dict>
</plist>
EOF_PERMISSIONS_HELPER_PLIST

  write_pkg_file "$pkg_root/Library/LaunchDaemons/com.talos.talos-supervisor.plist" 0644 <<'EOF_SUPERVISOR_PLIST'
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
  <string>/Library/Logs/Talos/talos_supervisor.out.log</string>
  <key>StandardErrorPath</key>
  <string>/Library/Logs/Talos/talos_supervisor.err.log</string>
  <key>UserName</key>
  <string>root</string>
  <key>GroupName</key>
  <string>wheel</string>
</dict>
</plist>
EOF_SUPERVISOR_PLIST

  write_pkg_file "$pkg_root/Library/LaunchDaemons/com.talos.talos-worker.plist" 0644 <<'EOF_WORKER_PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.talos.talos-worker</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Library/Talos/Worker/Talos Worker.app/Contents/MacOS/talos_worker</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>RMM_AGENT_ID_PATH</key>
    <string>/Library/Application Support/Talos/talos_worker_id.txt</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>WorkingDirectory</key>
  <string>/Library/Talos/Worker</string>
  <key>StandardOutPath</key>
  <string>/Library/Logs/Talos/talos_worker.out.log</string>
  <key>StandardErrorPath</key>
  <string>/Library/Logs/Talos/talos_worker.err.log</string>
</dict>
</plist>
EOF_WORKER_PLIST

  write_pkg_file "$pkg_scripts/postinstall" 0755 <<'EOF_POSTINSTALL'
#!/bin/sh
set -eu

SUPERVISOR_SERVICE_LABEL="com.talos.talos-supervisor"
WORKER_SERVICE_LABEL="com.talos.talos-worker"
SUPERVISOR_PLIST_PATH="/Library/LaunchDaemons/$SUPERVISOR_SERVICE_LABEL.plist"
WORKER_PLIST_PATH="/Library/LaunchDaemons/$WORKER_SERVICE_LABEL.plist"
PERMISSIONS_HELPER_PLIST_PATH="/Library/LaunchAgents/com.talos.permissions-helper.plist"
ENV_DIR="/Library/Preferences/Talos"
STATE_DIR="/Library/Application Support/Talos"
LOG_DIR="/Library/Logs/Talos"
AGENT_ENV_PATH="$ENV_DIR/rmm-agent.env"
SUPERVISOR_ENV_PATH="$ENV_DIR/talos-supervisor.env"
POSTINSTALL_LOG="$LOG_DIR/talos_agent_postinstall.log"

log_postinstall() {
  printf '%s %s\n' "$(/bin/date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$POSTINSTALL_LOG" 2>/dev/null || true
}

start_talos_launchdaemon() {
  label="$1"
  plist_path="$2"
  launchctl bootstrap system "$plist_path" >/dev/null 2>&1 || log_postinstall "failed to bootstrap $label"
  launchctl enable "system/$label" >/dev/null 2>&1 || log_postinstall "failed to enable $label"
  launchctl kickstart -k "system/$label" >/dev/null 2>&1 || log_postinstall "failed to kickstart $label"
}

install -d -m 0755 "/Library/Talos/Supervisor" "/Library/Talos/Worker" "$ENV_DIR" "$LOG_DIR"
install -d -m 0700 "$STATE_DIR" "$STATE_DIR/updates"
rm -f "/Library/Talos/Supervisor/talos_supervisor" "/Library/Talos/Supervisor/updater" "/Library/Talos/Supervisor/talos_supervisor.next" "/Library/Talos/Supervisor/updater.next" "/Library/Talos/Supervisor/talos_supervisor.previous" "/Library/Talos/Supervisor/updater.previous"
rm -f "/Library/Talos/Worker/talos_worker" "/Library/Talos/Worker/talos_worker_helper" "/Library/Talos/Worker/talos_worker_chat" "/Library/Talos/Worker/talos-rmm-agent"
rm -f "/Library/Talos/Worker/run-talos-worker.sh"
find "/Library/Talos/Supervisor" -maxdepth 1 -type d \( \
  -name "Talos Supervisor [0-9]*.app" -o \
  -name "Talos Supervisor.app.previous" \
\) -exec rm -rf {} + >/dev/null 2>&1 || true
find "/Library/Talos/Worker" -maxdepth 1 -type d \( \
  -name "Talos Worker [0-9]*.app" -o \
  -name "Talos Worker Helper [0-9]*.app" -o \
  -name "Talos Worker Chat [0-9]*.app" -o \
  -name "Talos Worker.app.previous" -o \
  -name "Talos Worker Helper.app.previous" -o \
  -name "Talos Worker Chat.app.previous" \
\) -exec rm -rf {} + >/dev/null 2>&1 || true
find "/Applications" -maxdepth 1 -type d -name "Talos Permissions Helper [0-9]*.app" -exec rm -rf {} + >/dev/null 2>&1 || true
chmod 0755 "/Library/Talos/Supervisor/Talos Supervisor.app/Contents/MacOS/talos_supervisor" "/Library/Talos/Worker/Talos Worker.app/Contents/MacOS/talos_worker" "/Library/Talos/Worker/Talos Worker Helper.app/Contents/MacOS/talos_worker_helper" "/Library/Talos/Worker/Talos Worker Chat.app/Contents/MacOS/talos_worker_chat" "/Applications/Talos Permissions Helper.app/Contents/MacOS/talos_permissions_helper"
chmod 0755 "/Library/Talos/Supervisor/run-talos-supervisor.sh"
chown -R root:wheel "/Library/Talos/Supervisor/Talos Supervisor.app" "/Library/Talos/Worker/Talos Worker.app" "/Library/Talos/Worker/Talos Worker Helper.app" "/Library/Talos/Worker/Talos Worker Chat.app" "/Applications/Talos Permissions Helper.app"
chown root:wheel "$SUPERVISOR_PLIST_PATH" "$WORKER_PLIST_PATH" "$PERMISSIONS_HELPER_PLIST_PATH"
chmod 0644 "$SUPERVISOR_PLIST_PATH" "$WORKER_PLIST_PATH" "$PERMISSIONS_HELPER_PLIST_PATH"

if [ ! -f "$AGENT_ENV_PATH" ]; then
  cat > "$AGENT_ENV_PATH" <<'EOF_AGENT_ENV'
RMM_SERVER_URL='ws://127.0.0.1:3002/agent/ws'
RMM_AGENT_TOKEN='replace-with-enrollment-token'
RMM_AGENT_ID_PATH='/Library/Application Support/Talos/talos_worker_id.txt'
RMM_INVENTORY_INTERVAL_SECS=30
RMM_RECONNECT_MAX_SECS=30
RMM_COMMAND_TIMEOUT_SECS=120
RUST_LOG=info
EOF_AGENT_ENV
  chmod 0600 "$AGENT_ENV_PATH"
fi

if [ ! -f "$SUPERVISOR_ENV_PATH" ]; then
  cat > "$SUPERVISOR_ENV_PATH" <<'EOF_SUPERVISOR_ENV'
# Automatic updates are disabled until RMM_UPDATE_BASE_URL points to your self-hosted API.
RMM_UPDATE_CHANNEL=stable
RMM_WORKER_INSTALL_DIR='/Library/Talos/Worker'
RMM_WORKER_ENV_FILE='/Library/Preferences/Talos/rmm-agent.env'
RMM_WORKER_VERSION_PATH='/Library/Application Support/Talos/worker.version'
RMM_WORKER_SERVICE_NAME=com.talos.talos-worker
RMM_SUPERVISOR_SERVICE_NAME=com.talos.talos-supervisor
RMM_SUPERVISOR_UPDATE_INTERVAL_SECS=86400
RMM_SUPERVISOR_MONITOR_INTERVAL_SECS=60
RUST_LOG=info
EOF_SUPERVISOR_ENV
  chmod 0600 "$SUPERVISOR_ENV_PATH"
fi

log_postinstall "reloading Talos LaunchDaemons"
launchctl bootout "system/$WORKER_SERVICE_LABEL" >/dev/null 2>&1 || true
launchctl bootout "system/$SUPERVISOR_SERVICE_LABEL" >/dev/null 2>&1 || true
start_talos_launchdaemon "$SUPERVISOR_SERVICE_LABEL" "$SUPERVISOR_PLIST_PATH"
start_talos_launchdaemon "$WORKER_SERVICE_LABEL" "$WORKER_PLIST_PATH"
log_postinstall "Talos Worker will surface Talos Permissions Helper if startup approvals are missing"

exit 0
EOF_POSTINSTALL
}

build_signed_pkg() {
  pkg_root="$ARTIFACT_DIR/macos-pkg-root"
  pkg_scripts="$ARTIFACT_DIR/macos-pkg-scripts"
  build_pkg_root "$pkg_root" "$pkg_scripts"
  xattr -cr "$pkg_root" "$pkg_scripts" >/dev/null 2>&1 || true
  find "$pkg_root" "$pkg_scripts" -name '._*' -delete
  rm -f "$PKG_PATH"
  if [ -n "$PKG_SIGNING_IDENTITY" ]; then
    if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
      pkgbuild \
        --root "$pkg_root" \
        --scripts "$pkg_scripts" \
        --identifier "$PKG_IDENTIFIER" \
        --version "$worker_version" \
        --install-location "/" \
        --filter '(^|/)\._[^/]*$' \
        --filter '(^|/)\.DS_Store$' \
        --filter '(^|/)CVS($|/)' \
        --filter '(^|/)\.svn($|/)' \
        --sign "$PKG_SIGNING_IDENTITY" \
        --keychain "$MACOS_SIGNING_KEYCHAIN" \
        "$PKG_PATH"
    else
      pkgbuild \
        --root "$pkg_root" \
        --scripts "$pkg_scripts" \
        --identifier "$PKG_IDENTIFIER" \
        --version "$worker_version" \
        --install-location "/" \
        --filter '(^|/)\._[^/]*$' \
        --filter '(^|/)\.DS_Store$' \
        --filter '(^|/)CVS($|/)' \
        --filter '(^|/)\.svn($|/)' \
        --sign "$PKG_SIGNING_IDENTITY" \
        "$PKG_PATH"
    fi
  else
    pkgbuild \
      --root "$pkg_root" \
      --scripts "$pkg_scripts" \
      --identifier "$PKG_IDENTIFIER" \
      --version "$worker_version" \
      --install-location "/" \
      --filter '(^|/)\._[^/]*$' \
      --filter '(^|/)\.DS_Store$' \
      --filter '(^|/)CVS($|/)' \
      --filter '(^|/)\.svn($|/)' \
      "$PKG_PATH"
  fi
  pkgutil --check-signature "$PKG_PATH" || true
}

parse_args "$@"
echo "Using macOS Cargo profile: $MACOS_CARGO_PROFILE"

require_macos
require_command rustup
require_command lipo
require_command openssl
require_command security
require_command codesign
require_command pkgbuild
require_command pkgutil
require_command shasum
require_command stat
require_command zip
require_command ditto
require_command curl
require_command make
require_command tar
require_command xcrun
require_command bun
require_rustup_toolchain
configure_swift_runtime_link_path

if [ -n "$MACOS_SIGNING_KEYCHAIN" ]; then
  MACOS_SIGNING_KEYCHAIN="$(normalize_keychain_path "$MACOS_SIGNING_KEYCHAIN")"
  if [ ! -f "$MACOS_SIGNING_KEYCHAIN" ]; then
    echo "macOS signing keychain not found: $MACOS_SIGNING_KEYCHAIN" >&2
    echo "Set MACOS_SIGNING_KEYCHAIN to a real keychain path, for example: $HOME/Library/Keychains/login.keychain-db" >&2
    exit 1
  fi
fi

mkdir -p "$ARTIFACT_DIR" "$UNIVERSAL_DIR" "$UNIVERSAL_BIN_DIR"
clean_loose_macos_artifact_binaries
mkdir -p "$UNIVERSAL_BIN_DIR"
resolve_codesign_identity
build_permissions_helper_frontend
build_worker_chat_frontend
export RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH="$(signing_public_key_path)"

build_target aarch64-apple-darwin
build_target x86_64-apple-darwin

profile_dir="$(target_profile_dir)"
PERMISSION_FLOW_RESOURCE_BUNDLE_SOURCE="$(find_permission_flow_resource_bundle "$profile_dir")"
worker_arm64="$TARGET_DIR/aarch64-apple-darwin/$profile_dir/talos_worker"
worker_x64="$TARGET_DIR/x86_64-apple-darwin/$profile_dir/talos_worker"
helper_arm64="$TARGET_DIR/aarch64-apple-darwin/$profile_dir/talos_worker_helper"
helper_x64="$TARGET_DIR/x86_64-apple-darwin/$profile_dir/talos_worker_helper"
worker_chat_arm64="$TARGET_DIR/aarch64-apple-darwin/$profile_dir/talos_worker_chat"
worker_chat_x64="$TARGET_DIR/x86_64-apple-darwin/$profile_dir/talos_worker_chat"
supervisor_arm64="$TARGET_DIR/aarch64-apple-darwin/$profile_dir/talos_supervisor"
supervisor_x64="$TARGET_DIR/x86_64-apple-darwin/$profile_dir/talos_supervisor"
permissions_helper_arm64="$TARGET_DIR/aarch64-apple-darwin/$profile_dir/talos_permissions_helper"
permissions_helper_x64="$TARGET_DIR/x86_64-apple-darwin/$profile_dir/talos_permissions_helper"

codesign_binary "$worker_arm64"
codesign_binary "$worker_x64"
codesign_binary "$helper_arm64"
codesign_binary "$helper_x64"
codesign_binary "$worker_chat_arm64"
codesign_binary "$worker_chat_x64"
codesign_binary "$supervisor_arm64"
codesign_binary "$supervisor_x64"
codesign_binary "$permissions_helper_arm64"
codesign_binary "$permissions_helper_x64"

lipo -create "$worker_arm64" "$worker_x64" -output "$UNIVERSAL_BIN_DIR/talos_worker"
lipo -create "$helper_arm64" "$helper_x64" -output "$UNIVERSAL_BIN_DIR/talos_worker_helper"
lipo -create "$worker_chat_arm64" "$worker_chat_x64" -output "$UNIVERSAL_BIN_DIR/talos_worker_chat"
lipo -create "$supervisor_arm64" "$supervisor_x64" -output "$UNIVERSAL_BIN_DIR/talos_supervisor"
lipo -create "$permissions_helper_arm64" "$permissions_helper_x64" -output "$UNIVERSAL_BIN_DIR/talos_permissions_helper"
chmod 0755 "$UNIVERSAL_BIN_DIR/talos_worker" "$UNIVERSAL_BIN_DIR/talos_worker_helper" "$UNIVERSAL_BIN_DIR/talos_worker_chat" "$UNIVERSAL_BIN_DIR/talos_supervisor" "$UNIVERSAL_BIN_DIR/talos_permissions_helper"
codesign_binary "$UNIVERSAL_BIN_DIR/talos_worker"
codesign_binary "$UNIVERSAL_BIN_DIR/talos_worker_helper"
codesign_binary "$UNIVERSAL_BIN_DIR/talos_worker_chat"
codesign_binary "$UNIVERSAL_BIN_DIR/talos_supervisor"
codesign_binary "$UNIVERSAL_BIN_DIR/talos_permissions_helper"

worker_version="$(package_version "$APPS_ROOT/talos_worker/Cargo.toml")"
worker_helper_version="$(package_version "$APPS_ROOT/talos_worker_helper/Cargo.toml")"
supervisor_version="$(package_version "$APPS_ROOT/talos_supervisor/Cargo.toml")"
worker_chat_version="$(package_version "$APPS_ROOT/talos_worker_chat/src-tauri/Cargo.toml")"
permissions_helper_version="$(package_version "$APPS_ROOT/talos_permissions_helper/src-tauri/Cargo.toml")"

clean_agent_app_bundles
make_supervisor_app_bundle "$ARTIFACT_DIR/macos-arm64/$SUPERVISOR_APP_NAME" "$supervisor_arm64" "$supervisor_version"
make_supervisor_app_bundle "$ARTIFACT_DIR/macos-x64/$SUPERVISOR_APP_NAME" "$supervisor_x64" "$supervisor_version"
make_supervisor_app_bundle "$UNIVERSAL_DIR/$SUPERVISOR_APP_NAME" "$UNIVERSAL_BIN_DIR/talos_supervisor" "$supervisor_version"
make_worker_app_bundle "$ARTIFACT_DIR/macos-arm64/$WORKER_APP_NAME" "$worker_arm64" "$worker_version"
make_worker_app_bundle "$ARTIFACT_DIR/macos-x64/$WORKER_APP_NAME" "$worker_x64" "$worker_version"
make_worker_app_bundle "$UNIVERSAL_DIR/$WORKER_APP_NAME" "$UNIVERSAL_BIN_DIR/talos_worker" "$worker_version"
make_worker_helper_app_bundle "$ARTIFACT_DIR/macos-arm64/$WORKER_HELPER_APP_NAME" "$helper_arm64" "$worker_helper_version"
make_worker_helper_app_bundle "$ARTIFACT_DIR/macos-x64/$WORKER_HELPER_APP_NAME" "$helper_x64" "$worker_helper_version"
make_worker_helper_app_bundle "$UNIVERSAL_DIR/$WORKER_HELPER_APP_NAME" "$UNIVERSAL_BIN_DIR/talos_worker_helper" "$worker_helper_version"
make_worker_chat_app_bundle "$ARTIFACT_DIR/macos-arm64/$WORKER_CHAT_APP_NAME" "$worker_chat_arm64" "$worker_chat_version"
make_worker_chat_app_bundle "$ARTIFACT_DIR/macos-x64/$WORKER_CHAT_APP_NAME" "$worker_chat_x64" "$worker_chat_version"
make_worker_chat_app_bundle "$UNIVERSAL_DIR/$WORKER_CHAT_APP_NAME" "$UNIVERSAL_BIN_DIR/talos_worker_chat" "$worker_chat_version"
make_permissions_helper_app_bundle "$UNIVERSAL_DIR/$PERMISSIONS_HELPER_APP_NAME" "$UNIVERSAL_BIN_DIR/talos_permissions_helper" "$permissions_helper_version"
clean_numbered_agent_app_bundles

make_worker_update_package macos-arm64 "$worker_version" "$ARTIFACT_DIR/macos-arm64/$WORKER_APP_NAME" "$ARTIFACT_DIR/macos-arm64/$WORKER_HELPER_APP_NAME" "$ARTIFACT_DIR/macos-arm64/$WORKER_CHAT_APP_NAME" "$UNIVERSAL_DIR/$PERMISSIONS_HELPER_APP_NAME"
make_worker_update_package macos-x64 "$worker_version" "$ARTIFACT_DIR/macos-x64/$WORKER_APP_NAME" "$ARTIFACT_DIR/macos-x64/$WORKER_HELPER_APP_NAME" "$ARTIFACT_DIR/macos-x64/$WORKER_CHAT_APP_NAME" "$UNIVERSAL_DIR/$PERMISSIONS_HELPER_APP_NAME"
make_supervisor_update_package macos-arm64 "$supervisor_version" "$ARTIFACT_DIR/macos-arm64/$SUPERVISOR_APP_NAME"
make_supervisor_update_package macos-x64 "$supervisor_version" "$ARTIFACT_DIR/macos-x64/$SUPERVISOR_APP_NAME"
build_signed_pkg
update_artifact_manifest

lipo -info "$UNIVERSAL_DIR/$SUPERVISOR_APP_NAME/Contents/MacOS/talos_supervisor"
lipo -info "$UNIVERSAL_DIR/$WORKER_APP_NAME/Contents/MacOS/talos_worker"
lipo -info "$UNIVERSAL_DIR/$WORKER_HELPER_APP_NAME/Contents/MacOS/talos_worker_helper"
lipo -info "$UNIVERSAL_DIR/$WORKER_CHAT_APP_NAME/Contents/MacOS/talos_worker_chat"
lipo -info "$UNIVERSAL_DIR/$PERMISSIONS_HELPER_APP_NAME/Contents/MacOS/talos_permissions_helper"
rm -rf "$UNIVERSAL_BIN_DIR"

echo "macOS universal app bundles: $UNIVERSAL_DIR"
echo "macOS update artifacts: $ARTIFACT_DIR"
echo "macOS package: $PKG_PATH"
