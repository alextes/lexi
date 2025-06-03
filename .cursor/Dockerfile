# syntax=docker/dockerfile:1

# use a specific rust version matching your project
FROM rust:bullseye

# install system dependencies required for building (e.g., for postgresql client - pq-sys).
RUN apt-get update && \
    apt-get install -y pkg-config build-essential libssl-dev

RUN cargo build