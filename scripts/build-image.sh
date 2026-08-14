#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
docker_bin="${DOCKER:-docker}"
image="${IMAGE:-microsystem-dev:rust-1.97.1}"
trust_bundle="$(mktemp)"
trap 'rm -f "$trust_bundle"' EXIT INT TERM

if command -v security >/dev/null 2>&1; then
  security find-certificate -a -p >"$trust_bundle"
elif [[ -r /etc/ssl/certs/ca-certificates.crt ]]; then
  cp /etc/ssl/certs/ca-certificates.crt "$trust_bundle"
fi

if command -v shasum >/dev/null 2>&1; then
  trust_digest="$(shasum -a 256 "$trust_bundle" | awk '{print $1}')"
else
  trust_digest="$(sha256sum "$trust_bundle" | awk '{print $1}')"
fi

"$docker_bin" build \
  --secret "id=host_ca,src=$trust_bundle" \
  --build-arg "HOST_CA_DIGEST=$trust_digest" \
  -t "$image" \
  "$repo_root"
