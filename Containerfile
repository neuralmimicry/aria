FROM docker.io/library/rust:1.92-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
COPY config ./config
RUN cargo build --release --locked

FROM docker.io/library/debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 aria && useradd --uid 10001 --gid aria --no-create-home aria \
    && mkdir /data && chown aria:aria /data
COPY --from=build /build/target/release/aria /usr/local/bin/aria
USER 10001:10001
WORKDIR /data
ENV ARIA_BIND=0.0.0.0:8091 ARIA_DATABASE_URL=sqlite:///data/aria.db?mode=rwc
EXPOSE 8091
HEALTHCHECK --interval=30s --timeout=3s CMD curl --fail --silent http://127.0.0.1:8091/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/aria"]
