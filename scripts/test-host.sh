#!/usr/bin/env bash
set -Eeuo pipefail

# Host-side gate.  `make test` is the supported entrypoint and is responsible
# for selecting the OrbStack Docker toolchain; this wrapper only verifies that
# a daemon is available and preserves the output for CI diagnostics.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

log_file="${MICROSYSTEM_HOST_TEST_LOG:-$repo_root/target/test-host.log}"
mkdir -p "$(dirname -- "$log_file")"

if ! command -v docker >/dev/null 2>&1; then
  echo "test-host: docker is required (run through OrbStack)" >&2
  exit 2
fi
if ! docker info >/dev/null 2>&1; then
  echo "test-host: Docker daemon is unavailable; start OrbStack first" >&2
  exit 2
fi
if ! command -v make >/dev/null 2>&1; then
  echo "test-host: make is required" >&2
  exit 2
fi

set +e
make test >"$log_file" 2>&1
status=$?
set -e
cat "$log_file"

if [[ "$status" -ne 0 ]]; then
  echo "test-host: FAIL (make test status=$status; log=$log_file)" >&2
  exit "$status"
fi
echo "test-host: PASS (log=$log_file)"
