#!/bin/sh
# Runs sync-server as the unprivileged `noura` user. A platform that mounts a
# root-owned volume starts the container as root, and this hands the data
# directory to `noura` and drops root before the server starts. `serve` runs
# `migrate` first, so an empty data directory or database starts on its own.
set -eu

data="${NOURA_DATA_DIR:-/data}"

if [ "$(id -u)" = 0 ]; then
	mkdir -p "$data"
	if [ "$(stat -c %U "$data")" != noura ]; then
		chown -R noura:noura "$data"
	fi
	chmod 0700 "$data"
	exec su-exec noura "$0" "$@"
fi

if [ "${1:-}" = serve ]; then
	sync-server migrate
fi
exec sync-server "$@"
