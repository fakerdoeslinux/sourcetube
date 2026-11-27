FROM rust:1.100-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates mpv \
  && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/sourcetube /usr/local/bin/sourcetube
COPY --from=builder /app/target/release/sourcetube-player /usr/local/bin/sourcetube-player
COPY public /usr/share/sourcetube/public
ENV PORT=8787 \
    APP_ORIGIN=http://localhost:8787 \
    SOURCETUBE_PUBLIC=/usr/share/sourcetube/public \
    SOURCETUBE_PLAYER=/usr/local/bin/sourcetube-player
EXPOSE 8787
CMD ["sourcetube"]
