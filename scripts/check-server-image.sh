#!/usr/bin/env bash
# Builds the sync server's image, starts it on an empty data directory, and
# waits for its `healthcheck` to pass. It runs twice: as the image's own user,
# and started as root on a root-owned mount, as Railway mounts a volume. Both
# times the server must run as a user other than root.
#
#   scripts/check-server-image.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
image=noura-sync-server:check
name="noura-sync-check-$$"

cleanup() {
  docker rm -f "$name" > /dev/null 2>&1 || true
}
trap cleanup EXIT

docker build -f "$root/crates/sync-server/Dockerfile" -t "$image" "$root"

check() {
  local status
  for _ in $(seq 1 60); do
    status="$(docker inspect --format '{{.State.Health.Status}}' "$name")"
    if [ "$status" = healthy ]; then
      break
    fi
    if [ "$status" = unhealthy ]; then
      docker logs "$name"
      echo "error: the image's health check failed." >&2
      exit 1
    fi
    sleep 1
  done
  if [ "$status" != healthy ]; then
    docker logs "$name"
    echo "error: the image never passed its health check." >&2
    exit 1
  fi
  local uid
  uid="$(docker exec "$name" awk '/^Uid:/ { print $2 }' /proc/1/status)"
  if [ "$uid" = 0 ]; then
    echo "error: the server runs as root." >&2
    exit 1
  fi
  docker exec "$name" sync-server healthcheck
  docker rm -f "$name" > /dev/null
}

origin=NOURA_PUBLIC_ORIGIN=https://sync.example.com
docker run -d --name "$name" -e "$origin" "$image" > /dev/null
check

docker run -d --name "$name" -e "$origin" --user 0 \
  --mount type=tmpfs,destination=/data "$image" > /dev/null
check

echo "The image starts on an empty data directory, as a user other than root."
