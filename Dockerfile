FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY assets ./assets
COPY web ./web
RUN cargo build --release --locked --bin music-link-finder-web

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/music-link-finder-web /usr/local/bin/music-link-finder-web
ENV PORT=10000
EXPOSE 10000
USER 10001:10001
CMD ["/usr/local/bin/music-link-finder-web"]
