FROM rust:1.90-slim AS builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
# charradissa-core has a path dependency on amassada-core (../../Amassada). Provide the
# Amassada repo as a named build context so the relative path resolves to /Amassada:
#   docker build --build-context amassada=../Amassada -t ... .
WORKDIR /Amassada
COPY --from=amassada . .
WORKDIR /app
COPY . .
RUN cargo build --release --bin charradissa-daemon

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/charradissa-daemon /usr/local/bin/charradissa-daemon
EXPOSE 8448
ENTRYPOINT ["charradissa-daemon"]
