# syntax=docker/dockerfile:1

# Cross-compiles expense-bot to a fully static x86_64 musl binary.
#
# The target server is Ubuntu 16.04 (glibc 2.23); a x86_64-unknown-linux-gnu
# binary built on any modern distro dies there with "GLIBC_2.28 not found".
# musl links libc statically, so the binary has no runtime dependency at all.
#
# The final stage is scratch and holds nothing but the binary, so:
#   docker buildx build --output type=local,dest=./dist .
# drops exactly ./dist/expense-bot on the host.

# rust:alpine is musl-hosted, so x86_64-unknown-linux-musl is the default
# host target and needs no rustup target add. The real MSRV is 1.95: ureq 3.3
# asks for 1.85, but libsqlite3-sys 0.38 (pulled in by rusqlite's `bundled`
# feature) uses cfg_select! in its build script, which only stabilized in 1.95.
# libsqlite3-sys declares no rust-version, so the resolver cannot route around
# it and 1.85 fails outright. Pin with
#   --build-arg RUST_IMAGE=rust:1.95-alpine
# if a future stable ever breaks the build; never below 1.95.
ARG RUST_IMAGE=rust:alpine

FROM ${RUST_IMAGE} AS builder

# rust:alpine already carries gcc and ca-certificates but deliberately omits
# the libc headers. rusqlite's `bundled` feature compiles SQLite from C and
# ring assembles native code, so both need musl-dev to find <stdio.h> et al.
RUN apk add --no-cache musl-dev

ENV CARGO_TERM_COLOR=never
WORKDIR /build

COPY . .

# target/ and the registry live in cache mounts, which COPY --from cannot read,
# so the binary is copied out to / inside the same layer.
RUN --mount=type=cache,target=/build/target,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --release --target x86_64-unknown-linux-musl \
    && cp target/x86_64-unknown-linux-musl/release/expense-bot /expense-bot

FROM scratch
COPY --from=builder /expense-bot /expense-bot
