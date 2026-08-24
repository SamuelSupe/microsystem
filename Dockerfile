# syntax=docker/dockerfile:1.7
FROM rust:1.97.1-bookworm

RUN echo 'deb http://deb.debian.org/debian bookworm-backports main' \
        > /etc/apt/sources.list.d/bookworm-backports.list \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        cpio \
        curl \
        grub-common \
        ipxe-qemu \
        make \
        qemu-efi-aarch64 \
        opensbi \
        xorriso \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        -t bookworm-backports \
        qemu-system-misc \
        qemu-system-arm \
        qemu-system-x86 \
    && dpkg --add-architecture amd64 \
    && apt-get update \
    && cd /tmp \
    && apt-get download grub-pc-bin:amd64 \
    && dpkg-deb --extract grub-pc-bin_*_amd64.deb / \
    && rm -f grub-pc-bin_*_amd64.deb \
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

RUN RUSTUP_USE_CURL=1 rustup target add \
        aarch64-unknown-none-softfloat \
        riscv64gc-unknown-none-elf \
        x86_64-unknown-none \
        --toolchain 1.97.1

WORKDIR /workspace
