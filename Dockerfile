# syntax=docker/dockerfile:1

FROM rustlang/rust:nightly-slim as builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    libssl-dev \
    pkg-config \
    build-essential \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

COPY . .
RUN cargo build --release

# -- NEW RUNTIME STAGE HERE --

# Use Debian 12 (Bookworm) slim as runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/transfer-tracker-service ./app

# ✅ Copy wait-for-it.sh into the final container
COPY wait-for-it.sh /wait-for-it.sh

# Make sure it's executable
RUN chmod +x /wait-for-it.sh

ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=8000

CMD ["./app"]
