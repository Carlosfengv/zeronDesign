# syntax=docker/dockerfile:1.7@sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e
ARG RUST_IMAGE=docker.io/library/rust:1.94-bookworm@sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55
ARG DEBIAN_IMAGE=docker.io/library/debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
ARG NODE_IMAGE=docker.io/library/node:22-bookworm@sha256:5647be709086c696ff32edaaf1c70cd26d1da6ab2b39c32f3c7b4c4a31957e37
FROM ${RUST_IMAGE} AS builder
WORKDIR /build
COPY services/runtime/Cargo.toml services/runtime/Cargo.lock ./
# Cargo resolves the path-based provider-gateway dev dependency while reading
# Cargo.lock, even though release builds do not compile the dev dependency.
COPY services/provider-gateway/Cargo.toml /provider-gateway/Cargo.toml
COPY services/runtime/target/docker-vendor /vendor
COPY services/runtime/src ./src
COPY services/runtime/migrations ./migrations
COPY services/runtime/scripts ./scripts
RUN mkdir -p .cargo \
    && printf '[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "/vendor"\n' > .cargo/config.toml \
    && cargo build --release --locked --offline

FROM ${NODE_IMAGE} AS browser-collector-runtime

FROM ${DEBIAN_IMAGE}
ARG DEBIAN_APT_MIRROR=https://deb.debian.org/debian
ARG DEBIAN_SECURITY_APT_MIRROR=https://security.debian.org/debian-security
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    sed -i "s|http://deb.debian.org/debian-security|${DEBIAN_SECURITY_APT_MIRROR}|g; s|http://deb.debian.org/debian|${DEBIAN_APT_MIRROR}|g" /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=5 update \
    && apt-get -o Acquire::Retries=5 install -y --no-install-recommends \
        ca-certificates \
        chromium \
        fontconfig \
        fonts-noto-cjk \
        fonts-noto-color-emoji \
        fonts-noto-core \
        kubernetes-client \
    && fc-cache -f \
    && useradd --system --uid 10001 --create-home --home-dir /var/lib/anydesign-runtime runtime
ARG REPOSITORY_COMMIT=unknown
ARG REPOSITORY_DIRTY=false
ARG RUNTIME_IMAGE_REF
ARG IMAGE_LOCK_HASH=unknown
ARG BUILD_TIMESTAMP=unknown
LABEL org.opencontainers.image.source="https://github.com/carlosfengv/zeronDesign" \
      org.opencontainers.image.revision="${REPOSITORY_COMMIT}" \
      org.opencontainers.image.created="${BUILD_TIMESTAMP}" \
      io.zerondesign.repository-dirty="${REPOSITORY_DIRTY}" \
      io.zerondesign.image-lock-hash="${IMAGE_LOCK_HASH}"
COPY --from=builder /build/target/release/anydesign-runtime /usr/local/bin/anydesign-runtime
COPY --from=browser-collector-runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=browser-collector-runtime /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
COPY --from=builder /build/scripts/check-browser-fonts.mjs /usr/local/lib/anydesign/check-browser-fonts.mjs
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
    && /usr/local/bin/node --version \
    && /usr/local/bin/npm --version \
    && /usr/local/bin/node /usr/local/lib/anydesign/check-browser-fonts.mjs
ENV RUNTIME_REPOSITORY_COMMIT=${REPOSITORY_COMMIT} \
    RUNTIME_REPOSITORY_DIRTY=${REPOSITORY_DIRTY} \
    RUNTIME_IMAGE_REF=${RUNTIME_IMAGE_REF} \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    RUNTIME_BROWSER_EXECUTABLE=/usr/bin/chromium \
    RUNTIME_BROWSER_COLLECTOR_EXECUTABLE=/usr/local/bin/node \
    RUNTIME_HOST=0.0.0.0 \
    RUNTIME_PORT=8080
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/anydesign-runtime"]
