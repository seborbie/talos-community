#!/usr/bin/env bash
set -euo pipefail

# This helper is limited to the Community image job's Ubuntu x86_64 runner.
[[ "$(uname -s)" = Linux && "$(uname -m)" = x86_64 ]] || {
  echo 'Community Buildx acquisition requires Linux x86_64.' >&2
  exit 1
}
: "${DOCKER_CONFIG:?Community image builds require an isolated Docker configuration}"
: "${RUNNER_TEMP:?Community Buildx acquisition requires runner temporary storage}"

version=v0.37.2
sha256=982ca20490b45ed1ec8d99795974d3d874a358f75938c9c237305010e6b7e548
url="https://github.com/docker/buildx/releases/download/${version}/buildx-${version}.linux-amd64"
download="$(mktemp "${RUNNER_TEMP}/talos-buildx.XXXXXX")"
trap 'rm -f "${download}"' EXIT

curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
  --retry 2 --connect-timeout 20 --max-time 180 --output "${download}" "${url}"
printf '%s  %s\n' "${sha256}" "${download}" | sha256sum --check --status

plugin="${DOCKER_CONFIG}/cli-plugins/docker-buildx"
mkdir -p "${DOCKER_CONFIG}/cli-plugins"
install -m 0755 "${download}" "${plugin}"
printf '%s  %s\n' "${sha256}" "${plugin}" | sha256sum --check --status

# Match the pinned setup action's availability probe before it can consider a download.
# An empty version input then retains this already verified plugin.
docker buildx >/dev/null
observed="$(docker buildx version)"
[[ "${observed}" = "github.com/docker/buildx ${version} "* ]] || {
  echo 'Docker did not select the reviewed Community Buildx version.' >&2
  exit 1
}
