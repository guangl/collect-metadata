ARG PRIVATE_METADATA_BASE_IMAGE=homelab/private-metadata:13d1b6a-reviewed
FROM ${PRIVATE_METADATA_BASE_IMAGE}
USER root
RUN sed -i 's|URIs: http://deb.debian.org|URIs: https://deb.debian.org|' /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=2 update \
    && apt-get -o Acquire::Retries=2 install -y --no-install-recommends ffmpeg \
    && rm -rf /var/lib/apt/lists/*
COPY --chown=1026:100 overrides/ /opt/private-metadata/javlibrarian/
USER 1026:100
