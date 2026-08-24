# A distroless image: no shell, no package manager, one static binary.
#
# The builder base is deliberately the rolling `rust:1-alpine`; the compiler
# itself is pinned by rust-toolchain.toml, which the build honours, and
# Cargo.lock makes the result reproducible. Alpine gives the musl target,
# so the daemon links statically and the final stage can be `scratch` —
# there is nothing in the running image to patch, update, or exploit.
FROM docker.io/library/rust:1-alpine AS build
WORKDIR /src
COPY . .
RUN cargo build --release --bin cairnd

FROM scratch
LABEL org.opencontainers.image.title="cairn" \
      org.opencontainers.image.description="ZIM archive server over a local HTTP API" \
      org.opencontainers.image.source="https://github.com/muhnschein/cairn" \
      org.opencontainers.image.licenses="ISC"
COPY --from=build /src/target/release/cairnd /cairnd

# Numeric ids: a scratch image has no /etc/passwd to look names up in.
# 65532 is the conventional nonroot uid; rootless podman maps it into the
# user namespace regardless.
USER 65532:65532

# The socket path comes from cairn.conf (bind-mounted read-only); see
# systemd/cairnd.container for the intended layout.
ENTRYPOINT ["/cairnd"]
