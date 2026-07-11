# syntax=docker/dockerfile:1.7
ARG RUST_IMAGE=rust:1.94-bookworm
FROM ${RUST_IMAGE} AS builder
WORKDIR /build
COPY services/runtime/Cargo.toml services/runtime/Cargo.lock ./
COPY services/runtime/target/docker-vendor /vendor
COPY services/runtime/src ./src
COPY services/runtime/scripts ./scripts
RUN mkdir -p .cargo \
    && printf '[source.crates-io]\nreplace-with = "vendored-sources"\n[source.vendored-sources]\ndirectory = "/vendor"\n' > .cargo/config.toml \
    && cargo build --release --locked --offline

FROM debian:bookworm-slim
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    sed -i 's|http://deb.debian.org/debian-security|https://mirrors.tuna.tsinghua.edu.cn/debian-security|g; s|http://deb.debian.org/debian|https://mirrors.tuna.tsinghua.edu.cn/debian|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=5 update \
    && apt-get -o Acquire::Retries=5 install -y --no-install-recommends ca-certificates chromium kubernetes-client \
    && useradd --system --uid 10001 --create-home --home-dir /var/lib/anydesign-runtime runtime
ARG REPOSITORY_COMMIT=unknown
ARG REPOSITORY_DIRTY=false
ARG RUNTIME_IMAGE_REF
LABEL org.opencontainers.image.source="https://github.com/carlosfengv/zeronDesign" \
      org.opencontainers.image.revision="${REPOSITORY_COMMIT}" \
      io.zerondesign.repository-dirty="${REPOSITORY_DIRTY}"
COPY --from=builder /build/target/release/anydesign-runtime /usr/local/bin/anydesign-runtime
ENV RUNTIME_REPOSITORY_COMMIT=${REPOSITORY_COMMIT} \
    RUNTIME_REPOSITORY_DIRTY=${REPOSITORY_DIRTY} \
    RUNTIME_IMAGE_REF=${RUNTIME_IMAGE_REF} \
    RUNTIME_BROWSER_EXECUTABLE=/usr/bin/chromium \
    RUNTIME_HOST=0.0.0.0 \
    RUNTIME_PORT=8080
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/anydesign-runtime"]
