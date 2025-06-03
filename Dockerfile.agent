# syntax=docker/dockerfile:1

# use a specific rust version matching your project
FROM rust:bullseye

# install system dependencies required for building (e.g., for postgresql client - pq-sys).
RUN apt-get update && \
    apt-get install -y libpq-dev pkg-config build-essential && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/app

# set environment variables for the build process
ENV SQLX_OFFLINE=true

# copy manifests (cargo.toml, cargo.lock) to leverage docker cache
COPY Cargo.toml Cargo.lock ./

# create a dummy main.rs to build and cache dependencies.
# this layer will be rebuilt only if cargo.toml or cargo.lock changes.
# adjust 'run_tg_bot' if your crate/binary name in cargo.toml is different.
RUN mkdir -p src/bin && \
    echo "fn main() {println!(\\"building_agent_dependencies_bin_for_caching...\\");}" > src/bin/run_tg_bot.rs

# build dependencies (release mode, similar to the main dockerfile's builder stage for effective caching).
# adjust 'run_tg_bot' if your_crate_name is different.
RUN cargo build --release --locked --bin run_tg_bot && \
    rm -f target/release/run_tg_bot # remove the dummy executable, retain dependencies.

# copy the rest of the application source code.
# remove the dummy src directory first to prevent conflicts.
RUN rm -rf src
COPY src ./src
COPY migrations ./migrations
COPY .sqlx ./.sqlx
# note: if there are other files or directories needed for the build, copy them here. e.g. static assets, config files for build.

# set the entrypoint to 'cargo'.
# this allows running 'cargo build', 'cargo test', 'cargo clippy', etc. by appending to 'docker run <image_name>'.
# example usage:
#   docker build -t my-rust-agent -f dockerfile.agent .
#   docker run --rm -v $(pwd):/usr/src/app my-rust-agent build --release
#   docker run --rm -v $(pwd):/usr/src/app my-rust-agent test
ENTRYPOINT ["cargo"]

# default command to execute if no command is provided to 'docker run'.
# this will execute 'cargo build' (which is a debug build by default).
CMD ["build"]
