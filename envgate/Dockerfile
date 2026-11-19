# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS build
# cmake is needed by libz-ng-sys, a dependency of Pingora.
RUN apt-get update && apt-get install -y --no-install-recommends cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && cp target/release/envgate /envgate && mkdir /data

# glibc, libgcc and CA roots, nothing else: no shell, no package manager.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /envgate /envgate
# Created here so a named volume mounted at /data starts out writable by the
# unprivileged user. A bind mount has to be made writable for uid 65532.
COPY --from=build --chown=nonroot:nonroot /data /data
VOLUME /data
EXPOSE 80 443
ENTRYPOINT ["/envgate"]
