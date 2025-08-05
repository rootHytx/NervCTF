FROM rust:slim AS build

# create a new empty shell project
RUN USER=root cargo new --bin local-configurator
WORKDIR /local-configurator

# copy over your manifests
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# this build step will cache your dependencies
RUN cargo build --release
RUN rm src/*.rs

# copy your source tree
COPY ./src ./src

# build for release
RUN rm ./target/release/deps/local_configurator*
RUN cargo build --release

# our final base
FROM rust:slim

# Set working directory
WORKDIR /app

# copy the build artifact from the build stage
COPY --from=build /local-configurator/target/release/local-configurator .

# set the startup command to run your binary
CMD ["./local-configurator"]
