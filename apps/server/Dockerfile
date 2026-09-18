# syntax=docker/dockerfile:1.7
FROM oven/bun:1.3.14 AS build
WORKDIR /app
ENV PATH=/root/.cargo/bin:$PATH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl build-essential \
	&& rm -rf /var/lib/apt/lists/* \
	&& curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.91 --profile minimal \
	&& rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,target=/root/.cargo/registry \
	--mount=type=cache,target=/root/.cargo/git \
	cargo install wasm-bindgen-cli --version 0.2.127 --locked
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
