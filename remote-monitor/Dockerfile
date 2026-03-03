# Use official Rust image for build
FROM rust:1.72 as builder

WORKDIR /app
COPY . .

RUN cargo build --release

# Use minimal base for runtime
FROM debian:bullseye-slim
WORKDIR /app

COPY --from=builder /app/target/release/remote-monitor /app/remote-monitor

# Set up environment variable for network IP
ENV REMOTE_MONITOR_NETWORK=127.0.0.1
ENV REMOTE_MONITOR_PORT=33133

EXPOSE 33133

CMD ["./remote-monitor"]
