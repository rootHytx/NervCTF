FROM rust:slim AS build

# create a new empty shell project
RUN USER=root cargo new --bin remote-monitor
WORKDIR /remote-monitor

# copy over your manifests
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# this build step will cache your dependencies
RUN cargo build --release
RUN rm src/*.rs

# copy your source tree
COPY ./src ./src

# build for release
RUN rm ./target/release/deps/remote_monitor*
RUN cargo build --release

# Runtime stage
FROM python:3.11-slim
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir ctfcli
COPY --from=build /remote-monitor/target/release/remote-monitor /usr/local/bin/
CMD ["remote-monitor"]
