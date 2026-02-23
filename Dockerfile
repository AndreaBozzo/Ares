FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin ares-api

FROM debian:bookworm-slim AS runner
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 chromium \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ares-api /usr/local/bin/ares-api
EXPOSE 3000
CMD ["ares-api"]
