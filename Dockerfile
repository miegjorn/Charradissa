FROM rust:1.90-slim AS builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
# charradissa-core → amassada-core → fondament-core (transitive path deps).
# Provide both upstream repos as named build contexts so the relative paths resolve:
#   /Fondament → fondament-core (amassada-core's dep: ../../../Fondament/fondament-core)
#   /Amassada  → amassada-core  (charradissa-core's dep: ../../Amassada/crates/amassada-core)
WORKDIR /Fondament
COPY --from=fondament . .
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
