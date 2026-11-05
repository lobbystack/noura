#!/usr/bin/env bash
# Builds the sync server's image through the Dokploy compose file, then
# starts it three ways and waits for its `healthcheck` to pass each time: as
# the image's own user, started as root on a root-owned mount, and through the
# compose file on its named volume. The server must run as a user other than
# root, stop cleanly on a terminate signal, and keep its data across a restart
# of the compose project.
#
#   scripts/check-server-image.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
compose_file="$root/.dokploy/docker-compose.sync-v2.yml"
image=noura-sync-server
name="noura-sync-check-$$"
project="noura-sync-check-$$"
export NOURA_PUBLIC_ORIGIN=https://sync.example.com

compose() {
  docker compose -p "$project" -f "$compose_file" "$@"
}

cleanup() {
  docker rm -f "$name" > /dev/null 2>&1 || true
  compose down -v > /dev/null 2>&1 || true
}
trap cleanup EXIT

compose config --quiet
compose build

# Waits for the container $1 to pass its health check, as a user other than
# root.
wait_healthy() {
  local container="$1" status
  for _ in $(seq 1 60); do
    status="$(docker inspect --format '{{.State.Health.Status}}' "$container")"
    if [ "$status" = healthy ]; then
      break
    fi
    if [ "$status" = unhealthy ]; then
      docker logs "$container"
      echo "error: the image's health check failed." >&2
      exit 1
    fi
    sleep 1
  done
  if [ "$status" != healthy ]; then
    docker logs "$container"
    echo "error: the image never passed its health check." >&2
    exit 1
  fi
  local uid
  uid="$(docker exec "$container" awk '/^Uid:/ { print $2 }' /proc/1/status)"
  if [ "$uid" = 0 ]; then
    echo "error: the server runs as root." >&2
    exit 1
  fi
  docker exec "$container" sync-server healthcheck
}

origin="NOURA_PUBLIC_ORIGIN=$NOURA_PUBLIC_ORIGIN"
docker run -d --name "$name" -e "$origin" "$image" > /dev/null
wait_healthy "$name"
if ! docker exec "$name" grep -q 'webauthn-rs 0.5.5' /usr/share/noura/THIRD_PARTY_NOTICES.txt; then
  echo "error: the image lacks the server's notice file." >&2
  exit 1
fi
docker rm -f "$name" > /dev/null

docker run -d --name "$name" -e "$origin" --user 0 \
  --mount type=tmpfs,destination=/data "$image" > /dev/null
wait_healthy "$name"
docker rm -f "$name" > /dev/null

compose up -d --no-build
container="$(compose ps -q sync-server)"
wait_healthy "$container"
before="$(docker exec "$container" sha256sum /data/server-secret)"
compose stop
exit_code="$(docker inspect --format '{{.State.ExitCode}}' "$container")"
if [ "$exit_code" != 0 ]; then
  docker logs "$container"
  echo "error: the server exited with $exit_code on a terminate signal." >&2
  exit 1
fi
compose down
compose up -d --no-build
container="$(compose ps -q sync-server)"
wait_healthy "$container"
if [ "$(docker exec "$container" sha256sum /data/server-secret)" != "$before" ]; then
  echo "error: the data volume lost the server secret across a restart." >&2
  exit 1
fi

echo "The image starts on an empty data directory and on the compose volume, as a user other than root."
