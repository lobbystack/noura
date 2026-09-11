FROM oven/bun:1.3.14 AS build
WORKDIR /app
COPY . .
RUN bun install --frozen-lockfile
RUN bun run --filter @noura/server build:release

FROM oven/bun:1.3.14-slim
WORKDIR /app
COPY --from=build --chown=bun:bun /app/apps/server/dist/ ./
COPY apps/server/entrypoint.sh /usr/local/bin/noura-entrypoint
# The entrypoint starts as root only long enough to fix volume ownership, then
# drops to `bun`. The image default stays unprivileged for compose and local use.
RUN chmod 0755 /usr/local/bin/noura-entrypoint \
	&& mkdir -p /data/blobs \
	&& chown -R bun:bun /data
USER bun
ENV NODE_ENV=production HOST=0.0.0.0 PORT=1900 BLOB_ROOT=/data/blobs
EXPOSE 1900
ENTRYPOINT ["/usr/local/bin/noura-entrypoint"]
CMD ["bun", "--no-install", "main.js"]
