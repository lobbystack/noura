# The sync server's image: the static binary, the account-web module its pages
# load, and a small entrypoint. Build it from the repository root:
#
#   docker build -f crates/sync-server/Dockerfile -t noura-sync-server .
FROM rust:1.99.1-alpine AS build
WORKDIR /src
RUN apk add --no-cache bash make musl-dev perl \
	&& rustup target add wasm32-unknown-unknown \
	&& cargo install wasm-bindgen-cli --version 0.2.127 --locked
COPY . .
RUN cargo build -p sync-server --release --locked \
	&& scripts/build-account-web.sh /out/account-web \
	&& cp target/release/sync-server /out/sync-server

FROM alpine:3.22
RUN apk add --no-cache ca-certificates su-exec \
	&& addgroup -S -g 10001 noura \
	&& adduser -S -D -H -u 10001 -G noura noura \
	&& mkdir -p /data \
	&& chown noura:noura /data
COPY --from=build /out/sync-server /usr/local/bin/sync-server
COPY --from=build /out/account-web /usr/share/noura/account-web
COPY crates/sync-server/docker/entrypoint.sh /usr/local/bin/noura-entrypoint
COPY crates/sync-server/THIRD_PARTY_NOTICES.txt /usr/share/noura/THIRD_PARTY_NOTICES.txt
USER noura
ENV NOURA_DATA_DIR=/data \
	NOURA_LISTEN=0.0.0.0:8080 \
	NOURA_ACCOUNT_WEB_DIR=/usr/share/noura/account-web
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=6s --start-period=30s \
	CMD ["sync-server", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/noura-entrypoint"]
CMD ["serve"]
