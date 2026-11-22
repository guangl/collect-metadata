FROM node:24-bookworm-slim AS frontend
WORKDIR /build
COPY package.json package-lock.json ./
RUN npm ci
COPY *.config.ts tsconfig.json ./
COPY ui ui
COPY docs/TROUBLESHOOTING.md docs/TROUBLESHOOTING.md
RUN npm run build

FROM rust:1.100-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src src
COPY --from=frontend /build/dist dist
RUN cargo build --locked --release --no-default-features --features web --bin collect-metadata-web

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -m 0700 -o 65532 -g 65532 /var/lib/collect-metadata /output \
    && install -m 0600 -o 65532 -g 65532 /dev/null /output/.initialized \
    && install -m 0600 -o 65532 -g 65532 /dev/null /var/lib/collect-metadata/.initialized
COPY --from=build /build/target/release/collect-metadata-web /usr/local/bin/
COPY LICENSE /usr/share/doc/collect-metadata/LICENSE
ENV APP_BIND=0.0.0.0:8080
ENV TMDB_CONFIG_FILE=/var/lib/collect-metadata/tmdb-token
ENV SCRAPE_OUTPUT_ROOT=/output
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=4s --start-period=20s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--max-time", "3", "http://127.0.0.1:8080/readyz"]
USER 65532:65532
ENTRYPOINT ["collect-metadata-web"]
