# syntax=docker/dockerfile:1
FROM debian:bookworm-slim

ARG TARGETARCH
ARG TARGETVARIANT
ARG VERSION
ARG GITHUB_REPOSITORY

LABEL org.opencontainers.image.title="openrepo-sync" \
      org.opencontainers.image.description="Sync packages from upstream sources into an OpenRepo repository" \
      org.opencontainers.image.url="https://github.com/${GITHUB_REPOSITORY}" \
      org.opencontainers.image.source="https://github.com/${GITHUB_REPOSITORY}" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.licenses="MIT"

# Install runtime dependencies:
#   ca-certificates  — HTTPS downloads
#   dpkg             — dpkg-deb for direct_url_latest .deb version detection
#   rpm              — rpm -qp  for direct_url_latest .rpm version detection
RUN apt-get update -qq \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      dpkg \
      rpm \
 && rm -rf /var/lib/apt/lists/*

# Map Docker's TARGETARCH (arm for linux/arm/v7) to the Debian package arch name.
# Docker sets TARGETARCH=arm + TARGETVARIANT=v7 for linux/arm/v7, but the .deb
# is named with the Debian convention: armhf.
COPY --chmod=755 <<'EOF' /usr/local/bin/install-pkg.sh
#!/bin/sh
set -e
ARCH="$1"
VARIANT="$2"
VERSION="$3"
if [ "$ARCH" = "arm" ] && [ "$VARIANT" = "v7" ]; then
  DEB_ARCH="armhf"
else
  DEB_ARCH="$ARCH"
fi
dpkg -i "/tmp/openrepo-sync_${VERSION}_${DEB_ARCH}.deb"
rm -f "/tmp/openrepo-sync_${VERSION}_${DEB_ARCH}.deb"
EOF

COPY openrepo-sync_${VERSION}_amd64.deb \
     openrepo-sync_${VERSION}_arm64.deb \
     openrepo-sync_${VERSION}_armhf.deb \
     /tmp/

RUN /usr/local/bin/install-pkg.sh "$TARGETARCH" "$TARGETVARIANT" "$VERSION" \
 && rm /usr/local/bin/install-pkg.sh

RUN useradd --system --no-create-home --shell /usr/sbin/nologin --uid 1000 openrepo

USER openrepo

ENTRYPOINT ["/usr/bin/openrepo-sync"]
CMD ["--help"]
