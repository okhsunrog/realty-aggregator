FROM rust:1.98-trixie AS builder
WORKDIR /source/backend
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/.cargo/ ./.cargo/
COPY backend/.sqlx/ ./.sqlx/
COPY backend/migrations/ ./migrations/
COPY backend/src/ ./src/
RUN cargo build --locked --release --bin backend --bin maintenance --bin sync_actuality

FROM debian:trixie
RUN apt-get update -qq && apt-get install -y --no-install-recommends ca-certificates libssl3t64 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /source/backend/target/release/backend /app/bin/backend
COPY --from=builder /source/backend/target/release/maintenance /app/bin/maintenance
COPY --from=builder /source/backend/target/release/sync_actuality /app/bin/sync_actuality
CMD ["/app/bin/backend"]
