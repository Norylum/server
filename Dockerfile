# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --locked -p norylum-server --bins

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 ffmpeg mkvtoolnix sqlite3 curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/norylum-server /usr/local/bin/norylum-server
COPY --from=builder /app/target/release/norylum-metadata-tags /usr/local/bin/norylum-metadata-tags
COPY scripts/docker-entrypoint.sh /usr/local/bin/norylum-entrypoint
RUN chmod 755 /usr/local/bin/norylum-entrypoint
WORKDIR /data
EXPOSE 8120
ENTRYPOINT ["/usr/local/bin/norylum-entrypoint"]
CMD ["norylum-server"]
