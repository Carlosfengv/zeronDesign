FROM rust:1.94-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp /src/target/release/provider-gateway /tmp/provider-gateway

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --create-home gateway
COPY --from=build /tmp/provider-gateway /usr/local/bin/provider-gateway
USER 10001
EXPOSE 9000
ENTRYPOINT ["/usr/local/bin/provider-gateway"]
