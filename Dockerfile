# Build context must contain the verified output of scripts/package-release.py
# as release-bundle/. The image runs the controller only; agents remain separate.
FROM alpine:3.22 AS release
ARG TARGETARCH
ARG TARGETVARIANT
COPY release-bundle/ /bundle/
RUN set -eu; \
    case "${TARGETARCH}/${TARGETVARIANT}" in \
      amd64/*) platform=linux-amd64 ;; \
      arm64/*) platform=linux-arm64 ;; \
      arm/v7) platform=linux-armv7 ;; \
      *) echo "Unsupported image platform: ${TARGETARCH}/${TARGETVARIANT}" >&2; exit 1 ;; \
    esac; \
    cp "/bundle/hproxy-server-${platform}" /hproxy-server; \
    rm /bundle/hproxy-server-linux-* /bundle/SHA256SUMS-server-linux-*; \
    chmod 0555 /hproxy-server

FROM alpine:3.22
RUN apk add --no-cache ca-certificates && \
    addgroup -S -g 10001 hproxy && \
    adduser -S -D -H -u 10001 -G hproxy hproxy && \
    mkdir -p /var/lib/hproxy/server && \
    chown hproxy:hproxy /var/lib/hproxy/server
COPY --from=release /hproxy-server /usr/local/bin/hproxy-server
COPY --from=release /bundle/ /opt/hproxy/release/
COPY scripts/oci-server-init.sh /usr/local/bin/hproxy-server-init
RUN chmod 0555 /usr/local/bin/hproxy-server-init
USER 10001:10001
WORKDIR /var/lib/hproxy/server
ENV HPROXY_PORT=19080
EXPOSE 19080
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD wget -q -O /dev/null "http://127.0.0.1:${HPROXY_PORT}/healthz" || exit 1
ENTRYPOINT ["/usr/local/bin/hproxy-server"]
CMD ["run", "--data", "/var/lib/hproxy/server", "--downloads", "/opt/hproxy/release", "--listen", "0.0.0.0:19080"]
