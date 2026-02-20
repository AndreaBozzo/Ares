FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin ares-server

FROM debian:bookworm-slim AS runner
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 chromium \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ares-server /usr/local/bin/ares-server
EXPOSE 3000
CMD ["ares-server"]
