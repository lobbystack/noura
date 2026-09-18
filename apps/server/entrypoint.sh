#!/bin/sh
# Ensure the persistent blob volume is writable by the unprivileged `bun` user,
# then drop privileges before starting the server. Railway mounts volumes as
# root, so the container must start as root to chown the mount; the long-running
# process must not stay root.
set -eu

if [ "$(id -u)" = "0" ]; then
	blob_root="${BLOB_ROOT:-/data/blobs}"
	mkdir -p "$blob_root"
	# Only the blob root is owned by the app; restrict it and hand it to `bun`.
	chown -R bun:bun "$blob_root"
	chmod 0700 "$blob_root"
	exec setpriv --reuid=bun --regid=bun --init-groups -- "$@"
fi

exec "$@"
