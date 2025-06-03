# syntax=docker/dockerfile:1

# ---- builder stage ----
# use a specific rust version matching your project if possible, e.g., rust:1.7x-bullseye
# using bullseye for newer system libraries.
FROM rust:bullseye AS builder
# alternative: from rust:latest as builder if you always want the newest rust.

# install system dependencies required for building.
RUN apt-get update && \
    apt-get install -y pkg-config build-essential libssl-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/app

# copy manifests (cargo.toml, cargo.lock)
COPY Cargo.toml Cargo.lock ./

# create a dummy main.rs to build and cache dependencies.
# this layer will be rebuilt only if cargo.toml or cargo.lock changes.
RUN mkdir -p src/bin && \
    echo "fn main() {println!(\"building_dependencies_bin...\");}" > src/bin/run_tg_bot.rs

# build dependencies.
# this step assumes the package name in cargo.toml (and thus the default binary name
# for this dummy build) is 'run_tg_bot'. if it's different, adjust 'run_tg_bot' in the rm command below.
# your_crate_name should be replaced with the actual crate name if not 'run_tg_bot'.
ENV SQLX_OFFLINE=true
RUN cargo build --release --locked --bin run_tg_bot && \
    rm -f target/release/run_tg_bot # remove the dummy executable (adjust 'run_tg_bot' if your_crate_name is different), retain dependencies.

# copy the rest of the application source code.
# remove the dummy src directory first to prevent conflicts.
RUN rm -rf src
COPY src ./src
COPY migrations ./migrations
COPY .sqlx ./.sqlx

# build the application binary.
# sqlx_offline=true is inherited, ensuring build uses checked-in sqlx-data.json.
# adjust 'run_tg_bot' if your final binary name is different. the actual binary name will be
# determined by your cargo.toml ([package].name or [[bin]].name).
# your_crate_name should be replaced with the actual crate name if not 'run_tg_bot'.
RUN cargo build --release --locked --bin run_tg_bot

# ---- runtime stage ----
# use a slim base image for a smaller footprint.
FROM debian:bullseye-slim AS runtime

WORKDIR /app

# install runtime dependencies:
# - ca-certificates: for making https requests (e.g., to discord/telegram apis).
# - libpq5: postgresql client library, required by sqlx if connecting to postgresql.
RUN apt-get update && \
    apt-get install -y ca-certificates libpq5 && \
    rm -rf /var/lib/apt/lists/*

# copy the compiled binary from the builder stage.
# important: adjust 'run_tg_bot' to your actual binary name (your_crate_name).
# this is typically the package name from cargo.toml, unless overridden.
COPY --from=builder /usr/src/app/target/release/run_tg_bot .

# set default environment variables.
# rust_log controls logging verbosity (e.g., info, debug, trace).
ENV RUST_LOG="debug,reqwest=info,hyper_util=info,sqlx=warn"
# database_url should be provided by the deployment environment (e.g., docker run, k8s secret).
# env database_url="postgres://user:password@host:port/database" # example

# create a non-root user and group for security.
# running as non-root is a best practice.
RUN groupadd --system --gid 1001 appgroup && \
    useradd --system --uid 1001 --gid appgroup --create-home appuser && \
    chown -R appuser:appgroup /app
USER appuser

# define the command to run the application.
# important: adjust 'run_tg_bot' to your actual binary name (your_crate_name).
CMD ["./run_tg_bot"]

# expose any ports the application listens on.
# for many bots, this is not needed as they connect outwards.
# expose 8080 