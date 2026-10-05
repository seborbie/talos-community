#!/usr/bin/env bash
# Release-size integration test. Run only on a disposable GitHub-hosted Linux runner.
set -euo pipefail
umask 077
test "${TALOS_DISPOSABLE_SMOKE:-}" = yes
test "${GITHUB_ACTIONS:-}" = true
test "${RUNNER_ENVIRONMENT:-}" = github-hosted
test "$(uname -s)/$(uname -m)" = Linux/x86_64
test "$#" -eq 2
archive=$1
evidence=$2
case "${evidence}" in "${RUNNER_TEMP:?}"/*) ;; *) exit 1 ;; esac
state=/var/lib/talos-server
sudo test ! -e "${state}" || { echo "Smoke runner already has Talos state" >&2; exit 1; }
test -z "$(sudo docker ps --all --quiet --filter label=com.docker.compose.project=talos-community)"
test -z "$(sudo docker volume ls --quiet --filter label=com.docker.compose.project=talos-community)"
work=$(mktemp -d "${RUNNER_TEMP}/talos-smoke.XXXXXX")
trap 'rm -rf -- "${work}"' EXIT
mkdir -p "${evidence}" "${work}/unpacked"
tar -xzf "${archive}" -C "${work}/unpacked"
mapfile -t bundles < <(find "${work}/unpacked" -mindepth 1 -maxdepth 1 -type d -name 'talos-community-*')
test "${#bundles[@]}" -eq 1
cd "${bundles[0]}"
sha256sum --check SHA256SUMS > "${evidence}/inner-checksums.txt"
launcher="$PWD/bin/linux-x86_64/talos-server"
test -x "${launcher}"
test -x ./start-talos.sh

# Real executable failure: a prerequisite error must not create state or secrets.
if sudo "${launcher}" --docker "${work}/missing-docker" quickstart \
  --config "$PWD/community-install.local.json" > "${evidence}/missing-docker.txt" 2>&1; then
  echo "Missing Docker was incorrectly accepted" >&2
  exit 1
fi
grep -q 'Install/start Docker' "${evidence}/missing-docker.txt"
sudo test ! -e "${state}"

# Use only the extracted wrapper/binary and anonymous published digests. No build/runtime tooling.
sudo ./start-talos.sh
sudo ./start-talos.sh status > "${evidence}/initial-status.json"
jq -e '.lifecycle == "running" and .operation == null' "${evidence}/initial-status.json" >/dev/null
installation_id=$(jq -r .installation_id "${evidence}/initial-status.json")
[[ "${installation_id}" =~ ^[0-9a-f]{32}$ ]]
sudo cat "${state}/local-tls/certificate.pem" > "${work}/certificate.pem"
sudo sha256sum "${state}/secrets.json" > "${work}/secrets-before.sha256"
traefik_id=$(sudo docker ps --quiet --filter label=com.docker.compose.project=talos-community \
  --filter label=com.docker.compose.service=traefik)
test -n "${traefik_id}"
sudo docker inspect --format '{{json .NetworkSettings.Ports}}' "${traefik_id}" \
  > "${evidence}/edge-ports.json"
jq -e '.["80/tcp"] == [{"HostIp":"127.0.0.1","HostPort":"8080"}] and
  .["443/tcp"] == [{"HostIp":"127.0.0.1","HostPort":"8443"}]' \
  "${evidence}/edge-ports.json" >/dev/null

request() {
  local host=$1 path=$2 output=$3
  shift 3
  curl --silent --show-error --fail --connect-timeout 5 --max-time 30 \
    --noproxy '*' --cacert "${work}/certificate.pem" \
    --resolve "${host}:8443:127.0.0.1" "https://${host}:8443${path}" \
    --output "${output}" "$@"
}
request talos.localhost / "${work}/frontend.html"
request api.talos.localhost /auth/registration-status "${work}/registration-status.json"
jq -e '.registrationOpen == true' "${work}/registration-status.json" >/dev/null
openssl rand -base64 32 > "${work}/password.txt"
jq -n --rawfile password "${work}/password.txt" \
  '{email:"release-smoke@example.invalid",password:($password | rtrimstr("\n"))}' \
  > "${work}/registration.json"
request api.talos.localhost /auth/register "${work}/registration-response.json" \
  --header 'Content-Type: application/json' --data-binary "@${work}/registration.json"
jq -e '.user.id and .token' "${work}/registration-response.json" >/dev/null
user_id=$(jq -r .user.id "${work}/registration-response.json")

sudo ./start-talos.sh stop
sudo ./start-talos.sh
sudo ./start-talos.sh uninstall
sudo test -f "${state}/secrets.json"
sudo ./start-talos.sh
sudo sha256sum "${state}/secrets.json" > "${work}/secrets-after.sha256"
cmp "${work}/secrets-before.sha256" "${work}/secrets-after.sha256"
request api.talos.localhost /auth/registration-status "${evidence}/persisted-registration-status.json"
jq -e '.registrationOpen == false and .mode == "closed"' \
  "${evidence}/persisted-registration-status.json" >/dev/null
request api.talos.localhost /auth/login "${work}/login-response.json" \
  --header 'Content-Type: application/json' --data-binary "@${work}/registration.json"
test "$(jq -r .user.id "${work}/login-response.json")" = "${user_id}"
sudo ./start-talos.sh status > "${evidence}/restarted-status.json"
test "$(jq -r .installation_id "${evidence}/restarted-status.json")" = "${installation_id}"
sudo ./start-talos.sh uninstall --remove-data --confirm "${installation_id}"
sudo test ! -e "${state}"
test -z "$(sudo docker volume ls --quiet --filter label=com.docker.compose.project=talos-community)"
printf '%s\n' 'Linux released-bundle startup, account persistence, loopback ports, and data removal passed.' \
  > "${evidence}/RESULT.txt"
(cd "${evidence}" && sha256sum ./* > SHA256SUMS)
