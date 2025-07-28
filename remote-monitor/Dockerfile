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

# our final base
FROM rust:slim

# copy the build artifact from the build stage
COPY --from=build /remote-monitor/target/release/remote-monitor .

# set the startup command to run your binary
CMD ["./remote-monitor"]
