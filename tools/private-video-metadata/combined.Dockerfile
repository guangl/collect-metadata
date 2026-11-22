ARG COLLECT_WEB_IMAGE=ghcr.io/guangl/collect-metadata:latest
ARG PRIVATE_METADATA_IMAGE=homelab/private-metadata:20261122-synopsis-deepseek
FROM ${COLLECT_WEB_IMAGE} AS web
FROM ${PRIVATE_METADATA_IMAGE}

USER root
RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=web /usr/local/bin/collect-metadata-web /usr/local/bin/
COPY --from=web /usr/share/doc/collect-metadata/LICENSE /usr/share/doc/collect-metadata/LICENSE
COPY supervise.py /opt/private-metadata/supervise.py
RUN chmod 0644 /opt/private-metadata/supervise.py \
    && install -d -m 0700 -o 1026 -g 100 /var/lib/collect-metadata /output

ENV APP_BIND=0.0.0.0:8080 \
    TMDB_CONFIG_FILE=/var/lib/collect-metadata/tmdb-token \
    SCRAPE_OUTPUT_ROOT=/output \
    PYTHONDONTWRITEBYTECODE=1
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=4s --start-period=20s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--max-time", "3", "http://127.0.0.1:8080/readyz"]
USER 1026:100
ENTRYPOINT ["python", "-u", "/opt/private-metadata/supervise.py"]
