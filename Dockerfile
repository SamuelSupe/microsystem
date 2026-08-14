# syntax=docker/dockerfile:1.7
FROM rust:1.97.1-bookworm

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        cpio \
        curl \
        ipxe-qemu \
        make \
        qemu-efi-aarch64 \
        qemu-system-arm \
    && rm -rf /var/lib/apt/lists/*

ARG HOST_CA_DIGEST=none
RUN --mount=type=secret,id=host_ca,required=false \
    cp /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/microsystem-ca-bundle.pem \
    && if [ -s /run/secrets/host_ca ]; then \
        cat /run/secrets/host_ca >> /etc/ssl/certs/microsystem-ca-bundle.pem; \
    fi \
    && test -n "$HOST_CA_DIGEST"

ENV SSL_CERT_FILE=/etc/ssl/certs/microsystem-ca-bundle.pem \
    CURL_CA_BUNDLE=/etc/ssl/certs/microsystem-ca-bundle.pem

RUN RUSTUP_USE_CURL=1 rustup target add aarch64-unknown-none-softfloat --toolchain 1.97.1

WORKDIR /workspace
