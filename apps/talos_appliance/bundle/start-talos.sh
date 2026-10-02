#!/bin/sh
set -eu
bundle_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
launcher="$bundle_dir/bin/linux-x86_64/talos-server"
if [ "$#" -gt 0 ]; then
  exec "$launcher" "$@"
fi
echo "Talos local evaluation. Docker Engine with Compose must be running."
echo "Data is retained in /var/lib/talos-server and the Docker PostgreSQL volume."
chmod 600 "$bundle_dir/community-install.local.json"
exec "$launcher" quickstart --config "$bundle_dir/community-install.local.json"
