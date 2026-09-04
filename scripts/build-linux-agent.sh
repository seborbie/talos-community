#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
APPS_ROOT="$REPO_ROOT/apps"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
LINUX_ARCH="${LINUX_ARCH:-linux-x64}"
if [ -n "${ARTIFACT_DIR+x}" ]; then
  ARTIFACT_DIR_EXPLICIT=1
else
  ARTIFACT_DIR_EXPLICIT=0
  ARTIFACT_DIR="$APPS_ROOT/installer/artifacts/$BUILD_PROFILE"
fi
PAYLOAD_DIR="$APPS_ROOT/installer/payload/linux/$LINUX_ARCH"
CERTS_DIR="${TALOS_CERTS_DIR:-$APPS_ROOT/certs}"
MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH="${RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH:-$CERTS_DIR/talos-manifest-signing.key.pem}"
MANIFEST_SIGNING_PUBLIC_KEY_DER_SOURCE="${RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH:-$CERTS_DIR/talos-manifest-signing-public.der}"
MANIFEST_SIGNING_PFX_PATH="${RMM_MANIFEST_SIGNING_PFX_PATH:-$CERTS_DIR/Talos Manifest Signing.pfx}"
MANIFEST_PUBLIC_KEY_DER_PATH="$APPS_ROOT/installer/tmp/manifest_public_key.der"
LINUX_RUST_TOOLCHAIN="${LINUX_RUST_TOOLCHAIN:-1.95.0}"
# Version and digest from Rust's official rustup archive/checksum pair.
LINUX_RUSTUP_INIT_VERSION="1.28.2"
LINUX_RUSTUP_INIT_SHA256="20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c"
LINUX_RUSTUP_INIT_URL="https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init"
LINUX_BUILDER_IMAGE="${LINUX_BUILDER_IMAGE:-talos-linux-builder:rust-1.95-rustup1.28.2-rockylinux8-glibc2.28}"
LINUX_DOCKER_PLATFORM="${LINUX_DOCKER_PLATFORM:-linux/amd64}"
LINUX_DOCKER_BUILD_JOBS="${LINUX_DOCKER_BUILD_JOBS:-2}"
HOST_UNAME="$(uname -s)"

case "$HOST_UNAME" in
  Linux*)
    USE_DOCKER="${USE_DOCKER:-0}"
    ;;
  Darwin*|MINGW*|MSYS*|CYGWIN*)
    USE_DOCKER="${USE_DOCKER:-1}"
    ;;
  *)
    USE_DOCKER="${USE_DOCKER:-1}"
    ;;
esac

SIGNING_PRIVATE_KEY_PATH=""
SIGNING_PRIVATE_KEY_IS_TEMP=0
SANITIZED_BUILD_CONTEXT=""
DOCKER_BUILD_OUTPUT=""

cleanup() {
  if [ "$SIGNING_PRIVATE_KEY_IS_TEMP" = "1" ] && [ -n "$SIGNING_PRIVATE_KEY_PATH" ]; then
    rm -f "$SIGNING_PRIVATE_KEY_PATH"
  fi
  if [ -n "$SANITIZED_BUILD_CONTEXT" ] && [ -d "$SANITIZED_BUILD_CONTEXT" ]; then
    rm -rf -- "$SANITIZED_BUILD_CONTEXT"
  fi
  if [ -n "$DOCKER_BUILD_OUTPUT" ] && [ -d "$DOCKER_BUILD_OUTPUT" ]; then
    rm -rf -- "$DOCKER_BUILD_OUTPUT"
  fi
}
trap cleanup EXIT

print_usage() {
  cat <<EOF_USAGE
Usage: $0 [--debug|--release] [--arch linux-x64] [--docker|--native]

Builds the Linux Talos agent artifacts:
  - talos-rmm-agent-linux-x64
  - Talos.Worker.<arch>.Update.zip
  - Talos.Worker.<arch>.Update.manifest.json/.sig
  - Talos.Supervisor.<arch>.Update.zip
  - Talos.Supervisor.<arch>.Update.manifest.json/.sig
  - manifest.json with Linux artifact metadata

macOS and Windows hosts build in Docker by default. Linux hosts build natively by
default, or in Docker with --docker.

Environment:
  BUILD_PROFILE                              dev or release
  ARTIFACT_DIR                               output directory
  LINUX_ARCH                                 linux-x64 (default)
  LINUX_BUILDER_IMAGE                        Docker image tag
  LINUX_DOCKER_PLATFORM                      Docker platform, default linux/amd64
  RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH  PEM private key for manifest signing
  RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH   PKCS#1 RSA public key DER to embed
  RMM_MANIFEST_SIGNING_PFX_PATH              optional PFX fallback
  RMM_MANIFEST_SIGNING_PFX_PASSWORD          required when using the PFX fallback
EOF_USAGE
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required" >&2
    exit 1
  fi
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --debug|--dev)
        BUILD_PROFILE="dev"
        if [ "$ARTIFACT_DIR_EXPLICIT" = "0" ]; then
          ARTIFACT_DIR="$APPS_ROOT/installer/artifacts/$BUILD_PROFILE"
        fi
        ;;
      --release)
        BUILD_PROFILE="release"
        if [ "$ARTIFACT_DIR_EXPLICIT" = "0" ]; then
          ARTIFACT_DIR="$APPS_ROOT/installer/artifacts/$BUILD_PROFILE"
        fi
        ;;
      --arch)
        shift
        if [ "$#" -eq 0 ]; then
          echo "--arch requires a value" >&2
          exit 1
        fi
        LINUX_ARCH="$1"
        PAYLOAD_DIR="$APPS_ROOT/installer/payload/linux/$LINUX_ARCH"
        ;;
      --artifact-dir)
        shift
        if [ "$#" -eq 0 ]; then
          echo "--artifact-dir requires a value" >&2
          exit 1
        fi
        ARTIFACT_DIR="$1"
        ARTIFACT_DIR_EXPLICIT=1
        ;;
      --docker)
        USE_DOCKER=1
        ;;
      --native)
        USE_DOCKER=0
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
}

target_triple_for_arch() {
  case "$1" in
    linux-x64)
      printf '%s\n' "x86_64-unknown-linux-gnu"
      ;;
    *)
      echo "Unsupported Linux architecture: $1" >&2
      echo "This script currently matches the Docker path from build-installers.ps1, which supports linux-x64." >&2
      exit 1
      ;;
  esac
}

cargo_profile_dir() {
  case "$BUILD_PROFILE" in
    dev)
      printf '%s\n' "debug"
      ;;
    release)
      printf '%s\n' "release"
      ;;
    *)
      echo "BUILD_PROFILE must be dev or release, got '$BUILD_PROFILE'." >&2
      exit 1
      ;;
  esac
}

cargo_build_args() {
  if [ "$BUILD_PROFILE" = "release" ]; then
    printf '%s\n' "build --locked --release --target $TARGET_TRIPLE -p talos_worker -p talos_supervisor"
  else
    printf '%s\n' "build --locked --target $TARGET_TRIPLE -p talos_worker -p talos_supervisor"
  fi
}

export_linux_target_toolchain_env() {
  case "$TARGET_TRIPLE" in
    x86_64-unknown-linux-gnu)
      export CC_x86_64_unknown_linux_gnu="${CC_x86_64_unknown_linux_gnu:-gcc}"
      export CXX_x86_64_unknown_linux_gnu="${CXX_x86_64_unknown_linux_gnu:-g++}"
      export AR_x86_64_unknown_linux_gnu="${AR_x86_64_unknown_linux_gnu:-ar}"
      export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-gcc}"
      ;;
    *)
      ;;
  esac
}

package_version() {
  version="$(sed -n 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -n 1)"
  if [ -z "$version" ]; then
    echo "Unable to find package version in $1" >&2
    exit 1
  fi
  printf '%s\n' "$version"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

file_size() {
  case "$HOST_UNAME" in
    Darwin*)
      stat -f%z "$1"
      ;;
    *)
      stat -c%s "$1"
      ;;
  esac
}

docker_mount_path() {
  path="$1"
  case "$HOST_UNAME" in
    MINGW*|MSYS*|CYGWIN*)
      if command -v cygpath >/dev/null 2>&1; then
        cygpath -w "$path" | sed 's#\\#/#g'
      else
        (cd "$path" && pwd -W)
      fi
      ;;
    *)
      printf '%s\n' "$path"
      ;;
  esac
}

ensure_manifest_signing_key() {
  if [ -f "$MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH" ]; then
    SIGNING_PRIVATE_KEY_PATH="$MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH"
    return
  fi

  if [ -f "$MANIFEST_SIGNING_PFX_PATH" ] && [ -n "${RMM_MANIFEST_SIGNING_PFX_PASSWORD:-}" ]; then
    SIGNING_PRIVATE_KEY_PATH="$(mktemp "${TMPDIR:-/tmp}/talos-linux-manifest-key.XXXXXX")"
    SIGNING_PRIVATE_KEY_IS_TEMP=1
    openssl pkcs12 \
      -in "$MANIFEST_SIGNING_PFX_PATH" \
      -nocerts \
      -nodes \
      -passin env:RMM_MANIFEST_SIGNING_PFX_PASSWORD 2>/dev/null |
      openssl pkey -out "$SIGNING_PRIVATE_KEY_PATH" 2>/dev/null
    return
  fi

  echo "Manifest signing private key not found: $MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH" >&2
  echo "Set RMM_MANIFEST_SIGNING_PRIVATE_KEY_PEM_PATH, or set RMM_MANIFEST_SIGNING_PFX_PASSWORD for: $MANIFEST_SIGNING_PFX_PATH" >&2
  exit 1
}

prepare_manifest_public_key() {
  mkdir -p "$(dirname "$MANIFEST_PUBLIC_KEY_DER_PATH")"
  if [ -f "$MANIFEST_SIGNING_PUBLIC_KEY_DER_SOURCE" ]; then
    cp -f "$MANIFEST_SIGNING_PUBLIC_KEY_DER_SOURCE" "$MANIFEST_PUBLIC_KEY_DER_PATH"
    return
  fi

  openssl rsa \
    -in "$SIGNING_PRIVATE_KEY_PATH" \
    -RSAPublicKey_out \
    -outform DER \
    -out "$MANIFEST_PUBLIC_KEY_DER_PATH" >/dev/null 2>&1
}

prepare_sanitized_build_context() {
  require_command git
  SANITIZED_BUILD_CONTEXT="$(mktemp -d "${TMPDIR:-/tmp}/talos-linux-source.XXXXXX")"

  while IFS= read -r -d '' relative_path; do
    case "$relative_path" in
      apps/certs/*|*.pem|*.pfx|*.p12|*.key)
        continue
        ;;
      .env|*/.env|.env.*|*/.env.*)
        case "$relative_path" in
          *.example|*.sample|*.template)
            ;;
          *)
            continue
            ;;
        esac
        ;;
    esac

    source_path="$REPO_ROOT/$relative_path"
    if [ ! -f "$source_path" ] && [ ! -L "$source_path" ]; then
      continue
    fi
    destination_path="$SANITIZED_BUILD_CONTEXT/$relative_path"
    mkdir -p "$(dirname "$destination_path")"
    cp -pP "$source_path" "$destination_path"
  done < <(git -C "$REPO_ROOT" ls-files --cached --others --exclude-standard -z)

  # The clients need this public trust anchor at compile time. It is the only ignored build input
  # copied into the context; the corresponding private key/PFX remains outside the container.
  sanitized_public_key="$SANITIZED_BUILD_CONTEXT/apps/installer/tmp/manifest_public_key.der"
  mkdir -p "$(dirname "$sanitized_public_key")"
  cp -p "$MANIFEST_PUBLIC_KEY_DER_PATH" "$sanitized_public_key"
}

ensure_docker_builder_image() {
  require_command docker
  if [ "$LINUX_DOCKER_PLATFORM" != "linux/amd64" ]; then
    echo "The pinned rustup-init bootstrap is for x86_64 Linux; LINUX_DOCKER_PLATFORM must be linux/amd64." >&2
    exit 1
  fi
  image_platform="$(docker image inspect "$LINUX_BUILDER_IMAGE" --format '{{.Os}}/{{.Architecture}}' 2>/dev/null || true)"
  if [ "$image_platform" = "$LINUX_DOCKER_PLATFORM" ]; then
    echo "Using cached Linux Docker builder image: $LINUX_BUILDER_IMAGE"
    return
  fi
  if [ -n "$image_platform" ]; then
    echo "Rebuilding Linux Docker builder image for $LINUX_DOCKER_PLATFORM (cached image is $image_platform)"
  fi

  echo "Building Linux Docker builder image: $LINUX_BUILDER_IMAGE ($LINUX_DOCKER_PLATFORM)"
  docker build --platform "$LINUX_DOCKER_PLATFORM" -t "$LINUX_BUILDER_IMAGE" - <<EOF_DOCKER
FROM rockylinux:8@sha256:9794037624aaa6212aeada1d28861ef5e0a935adaf93e4ef79837119f2a2d04c
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:\$PATH
RUN dnf install -y \
        ca-certificates \
        clang \
        clang-devel \
        cmake \
        curl \
        gcc \
        gcc-c++ \
        git \
        make \
        openssl-devel \
        perl \
        pkgconf-pkg-config \
        tar \
        xz \
        zip \
    && dnf clean all \
    && rm -rf /var/cache/dnf
RUN curl --proto '=https' --tlsv1.2 -sSfL '$LINUX_RUSTUP_INIT_URL' -o /tmp/rustup-init \
    && echo '$LINUX_RUSTUP_INIT_SHA256  /tmp/rustup-init' | sha256sum -c - \
    && chmod 0755 /tmp/rustup-init \
    && /tmp/rustup-init -y --profile minimal --default-toolchain $LINUX_RUST_TOOLCHAIN --target x86_64-unknown-linux-gnu \
    && rm -f /tmp/rustup-init \
    && chmod -R a+w \$RUSTUP_HOME \$CARGO_HOME
EOF_DOCKER
}

build_in_docker() {
  ensure_docker_builder_image
  prepare_sanitized_build_context
  source_mount="$(docker_mount_path "$SANITIZED_BUILD_CONTEXT")"
  DOCKER_BUILD_OUTPUT="$(mktemp -d "${TMPDIR:-/tmp}/talos-linux-output.XXXXXX")"
  output_mount="$(docker_mount_path "$DOCKER_BUILD_OUTPUT")"
  target_cache_volume="talos-linux-target-$TARGET_TRIPLE"
  cargo_args="$(cargo_build_args)"
container_script="
set -euo pipefail
export PATH=\"/usr/local/cargo/bin:\$PATH\"
export CC_x86_64_unknown_linux_gnu=\"\${CC_x86_64_unknown_linux_gnu:-gcc}\"
export CXX_x86_64_unknown_linux_gnu=\"\${CXX_x86_64_unknown_linux_gnu:-g++}\"
export AR_x86_64_unknown_linux_gnu=\"\${AR_x86_64_unknown_linux_gnu:-ar}\"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=\"\${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-gcc}\"
rustup target add '$TARGET_TRIPLE'
cargo $cargo_args
cp '/cargo-target/$TARGET_TRIPLE/$CARGO_PROFILE_DIR/talos_worker' '/talos-build-output/talos_worker'
cp '/cargo-target/$TARGET_TRIPLE/$CARGO_PROFILE_DIR/talos_supervisor' '/talos-build-output/talos_supervisor'
"

  echo "Building Linux worker and supervisor in Docker..."
  MSYS_NO_PATHCONV=1 docker run --rm \
    --platform "$LINUX_DOCKER_PLATFORM" \
    -v "$source_mount:/workspace:ro" \
    -v "$output_mount:/talos-build-output" \
    -v talos-linux-cargo-registry:/usr/local/cargo/registry \
    -v talos-linux-cargo-git:/usr/local/cargo/git \
    -v "$target_cache_volume:/cargo-target" \
    -w /workspace/apps \
    -e CARGO_TARGET_DIR=/cargo-target \
    -e CARGO_BUILD_JOBS="$LINUX_DOCKER_BUILD_JOBS" \
    -e RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH=/workspace/apps/installer/tmp/manifest_public_key.der \
    "$LINUX_BUILDER_IMAGE" \
    bash -lc "$container_script"

  host_output_dir="$APPS_ROOT/target/$TARGET_TRIPLE/$CARGO_PROFILE_DIR"
  mkdir -p "$host_output_dir"
  cp -f "$DOCKER_BUILD_OUTPUT/talos_worker" "$host_output_dir/talos_worker"
  cp -f "$DOCKER_BUILD_OUTPUT/talos_supervisor" "$host_output_dir/talos_supervisor"
  rm -rf -- "$SANITIZED_BUILD_CONTEXT" "$DOCKER_BUILD_OUTPUT"
  SANITIZED_BUILD_CONTEXT=""
  DOCKER_BUILD_OUTPUT=""
}

build_natively() {
  if [ "$HOST_UNAME" != "Linux" ]; then
    echo "--native is only supported on Linux. Use --docker on $HOST_UNAME." >&2
    exit 1
  fi
  require_command rustup
  require_command cargo

  rustup target add --toolchain "$LINUX_RUST_TOOLCHAIN" "$TARGET_TRIPLE"
  echo "Building Linux worker and supervisor natively..."
  (
    cd "$APPS_ROOT"
    export RMM_MANIFEST_SIGNING_PUBLIC_KEY_DER_PATH="$MANIFEST_PUBLIC_KEY_DER_PATH"
    export_linux_target_toolchain_env
    if [ "$BUILD_PROFILE" = "release" ]; then
      RUSTC="$(rustup which --toolchain "$LINUX_RUST_TOOLCHAIN" rustc)" \
        rustup run "$LINUX_RUST_TOOLCHAIN" cargo build --locked --release --target "$TARGET_TRIPLE" -p talos_worker -p talos_supervisor
    else
      RUSTC="$(rustup which --toolchain "$LINUX_RUST_TOOLCHAIN" rustc)" \
        rustup run "$LINUX_RUST_TOOLCHAIN" cargo build --locked --target "$TARGET_TRIPLE" -p talos_worker -p talos_supervisor
    fi
  )
}

copy_binary() {
  source_path="$1"
  destination_path="$2"
  if [ ! -f "$source_path" ]; then
    echo "Expected build output not found: $source_path" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$destination_path")"
  cp -f "$source_path" "$destination_path"
  chmod 0755 "$destination_path"
}

make_zip_host() {
  package_path="$1"
  payload_dir="$2"
  shift 2
  require_command zip
  rm -f "$package_path"
  (
    cd "$payload_dir"
    zip -q -9 -X "$package_path" "$@"
  )
}

make_zip_docker() {
  package_file="$1"
  shift
  payload_mount="$(docker_mount_path "$PAYLOAD_DIR")"
  artifact_mount="$(docker_mount_path "$ARTIFACT_DIR")"
  entries=""
  for entry in "$@"; do
    entries="$entries '$entry'"
  done
  MSYS_NO_PATHCONV=1 docker run --rm \
    --platform "$LINUX_DOCKER_PLATFORM" \
    -v "$payload_mount:/talos-payload:ro" \
    -v "$artifact_mount:/talos-artifacts" \
    -w "/talos-payload" \
    "$LINUX_BUILDER_IMAGE" \
    bash -lc "rm -f '/talos-artifacts/$package_file' && zip -q -9 -X '/talos-artifacts/$package_file' $entries"
}

make_update_zip() {
  package_file="$1"
  shift
  package_path="$ARTIFACT_DIR/$package_file"
  if [ "$USE_DOCKER" = "1" ]; then
    make_zip_docker "$package_file" "$@"
  else
    make_zip_host "$package_path" "$PAYLOAD_DIR" "$@"
  fi
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

write_update_manifest() {
  product="$1"
  version="$2"
  package_file="$3"
  manifest_path="$4"
  shift 4

  package_path="$ARTIFACT_DIR/$package_file"
  sha256="$(sha256_file "$package_path")"
  size_bytes="$(file_size "$package_path")"
  contents_json="$(manifest_contents_json "$@")"

  cat > "$manifest_path" <<EOF_MANIFEST
{
  "product": "$product",
  "platform": "linux",
  "arch": "$LINUX_ARCH",
  "channel": "stable",
  "version": "$version",
  "minimumSupportedVersion": "$version",
  "severity": "normal",
  "publishedAtUtc": "$GENERATED_AT_UTC",
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
  "installMode": "silent"
}
EOF_MANIFEST
}

sign_manifest() {
  manifest_path="$1"
  signature_path="$2"
  tmp_sig="$(mktemp "${TMPDIR:-/tmp}/talos-linux-manifest-sig.XXXXXX")"
  openssl dgst -sha256 -sign "$SIGNING_PRIVATE_KEY_PATH" -out "$tmp_sig" "$manifest_path"
  openssl base64 -A -in "$tmp_sig" -out "$signature_path"
  rm -f "$tmp_sig"
}

artifact_json() {
  file_name="$1"
  artifact_path="$ARTIFACT_DIR/$file_name"
  printf '{ "fileName": "%s", "sizeBytes": %s, "sha256": "%s" }' \
    "$file_name" \
    "$(file_size "$artifact_path")" \
    "$(sha256_file "$artifact_path")"
}

manifest_suffix_for_arch() {
  case "$LINUX_ARCH" in
    linux-x64)
      printf '%s\n' "LinuxX64"
      ;;
    *)
      echo "Unsupported Linux architecture: $LINUX_ARCH" >&2
      exit 1
      ;;
  esac
}

write_artifact_manifest() {
  manifest_path="$ARTIFACT_DIR/manifest.json"
  worker_key="worker$(manifest_suffix_for_arch)"
  supervisor_key="supervisor$(manifest_suffix_for_arch)"
  linux_agent_json="$(artifact_json "$LINUX_AGENT_BINARY_NAME")"
  worker_manifest_json="$(artifact_json "$WORKER_MANIFEST_FILE")"
  worker_signature_json="$(artifact_json "$WORKER_SIGNATURE_FILE")"
  worker_package_json="$(artifact_json "$WORKER_PACKAGE_FILE")"
  supervisor_manifest_json="$(artifact_json "$SUPERVISOR_MANIFEST_FILE")"
  supervisor_signature_json="$(artifact_json "$SUPERVISOR_SIGNATURE_FILE")"
  supervisor_package_json="$(artifact_json "$SUPERVISOR_PACKAGE_FILE")"

  cat > "$manifest_path" <<EOF_ARTIFACT_MANIFEST
{
  "profile": "$BUILD_PROFILE",
  "generatedAtUtc": "$GENERATED_AT_UTC",
  "linux": {
    "agentBinary": $linux_agent_json
  },
  "updates": {
    "$worker_key": {
      "manifest": $worker_manifest_json,
      "signature": $worker_signature_json,
      "package": $worker_package_json
    },
    "$supervisor_key": {
      "manifest": $supervisor_manifest_json,
      "signature": $supervisor_signature_json,
      "package": $supervisor_package_json
    }
  }
}
EOF_ARTIFACT_MANIFEST
}

if [ "${BASH_SOURCE[0]}" != "$0" ]; then
  # Focused contract tests source the context-copying functions without loading a signing key or
  # invoking Docker. Direct script execution always continues into the build.
  return 0
fi

parse_args "$@"
TARGET_TRIPLE="$(target_triple_for_arch "$LINUX_ARCH")"
CARGO_PROFILE_DIR="$(cargo_profile_dir)"
WORKER_VERSION="$(package_version "$APPS_ROOT/talos_worker/Cargo.toml")"
SUPERVISOR_VERSION="$(package_version "$APPS_ROOT/talos_supervisor/Cargo.toml")"
GENERATED_AT_UTC="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
LINUX_AGENT_BINARY_NAME="talos-rmm-agent-linux-x64"
WORKER_PACKAGE_FILE="Talos.Worker.$LINUX_ARCH.Update.zip"
WORKER_MANIFEST_FILE="Talos.Worker.$LINUX_ARCH.Update.manifest.json"
WORKER_SIGNATURE_FILE="Talos.Worker.$LINUX_ARCH.Update.manifest.sig"
SUPERVISOR_PACKAGE_FILE="Talos.Supervisor.$LINUX_ARCH.Update.zip"
SUPERVISOR_MANIFEST_FILE="Talos.Supervisor.$LINUX_ARCH.Update.manifest.json"
SUPERVISOR_SIGNATURE_FILE="Talos.Supervisor.$LINUX_ARCH.Update.manifest.sig"

require_command openssl
ensure_manifest_signing_key
prepare_manifest_public_key

mkdir -p "$ARTIFACT_DIR" "$PAYLOAD_DIR"

echo "Using apps root: $APPS_ROOT"
echo "Using build profile: $BUILD_PROFILE"
echo "Using Linux architecture: $LINUX_ARCH ($TARGET_TRIPLE)"
echo "Using artifact directory: $ARTIFACT_DIR"
if [ "$USE_DOCKER" = "1" ]; then
  build_in_docker
else
  build_natively
fi

LINUX_BUILD_DIR="$APPS_ROOT/target/$TARGET_TRIPLE/$CARGO_PROFILE_DIR"
copy_binary "$LINUX_BUILD_DIR/talos_worker" "$PAYLOAD_DIR/talos_worker"
copy_binary "$LINUX_BUILD_DIR/talos_supervisor" "$PAYLOAD_DIR/talos_supervisor"
copy_binary "$LINUX_BUILD_DIR/talos_supervisor" "$ARTIFACT_DIR/$LINUX_AGENT_BINARY_NAME"

echo "Building Linux update packages..."
make_update_zip "$WORKER_PACKAGE_FILE" "talos_worker"
make_update_zip "$SUPERVISOR_PACKAGE_FILE" "talos_supervisor"

echo "Writing signed Linux update manifests..."
write_update_manifest "worker" "$WORKER_VERSION" "$WORKER_PACKAGE_FILE" "$ARTIFACT_DIR/$WORKER_MANIFEST_FILE" "talos_worker"
sign_manifest "$ARTIFACT_DIR/$WORKER_MANIFEST_FILE" "$ARTIFACT_DIR/$WORKER_SIGNATURE_FILE"
write_update_manifest "supervisor" "$SUPERVISOR_VERSION" "$SUPERVISOR_PACKAGE_FILE" "$ARTIFACT_DIR/$SUPERVISOR_MANIFEST_FILE" "talos_supervisor"
sign_manifest "$ARTIFACT_DIR/$SUPERVISOR_MANIFEST_FILE" "$ARTIFACT_DIR/$SUPERVISOR_SIGNATURE_FILE"

echo "Writing Linux artifact manifest..."
write_artifact_manifest

echo ""
echo "Linux agent build complete."
echo "Linux installer agent binary:"
echo "$ARTIFACT_DIR/$LINUX_AGENT_BINARY_NAME"
echo "Linux worker update package:"
echo "$ARTIFACT_DIR/$WORKER_PACKAGE_FILE"
echo "Linux worker update manifest:"
echo "$ARTIFACT_DIR/$WORKER_MANIFEST_FILE"
echo "Linux supervisor update package:"
echo "$ARTIFACT_DIR/$SUPERVISOR_PACKAGE_FILE"
echo "Linux supervisor update manifest:"
echo "$ARTIFACT_DIR/$SUPERVISOR_MANIFEST_FILE"
echo "Installer artifact manifest:"
echo "$ARTIFACT_DIR/manifest.json"
