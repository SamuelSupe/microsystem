#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
source "$repo_root/scripts/qemu-arch.sh"

timeout_seconds="${MICROSYSTEM_MICA_QEMU_TIMEOUT:-180}"
input_settle="${MICROSYSTEM_MICA_INPUT_SETTLE:-0.15}"
stage_timeout="${MICROSYSTEM_MICA_STAGE_TIMEOUT:-60}"
combo_timeout="${MICROSYSTEM_MICA_COMBO_TIMEOUT:-90}"
log_file="${MICROSYSTEM_MICA_QEMU_LOG:-$repo_root/target/mica-qemu.log}"
fixture_http_port="${MICROSYSTEM_MICA_HTTP_PORT:-18080}"
fixture_tcp_port="${MICROSYSTEM_MICA_TCP_PORT:-18082}"
fixture_udp_port="${MICROSYSTEM_MICA_UDP_PORT:-18081}"
fixture_tls_port="${MICROSYSTEM_MICA_TLS_PORT:-18443}"
fixture_tls_unknown_port="${MICROSYSTEM_MICA_TLS_UNKNOWN_PORT:-18444}"
fixture_tls_expired_port="${MICROSYSTEM_MICA_TLS_EXPIRED_PORT:-18445}"
fixture_tls_future_port="${MICROSYSTEM_MICA_TLS_FUTURE_PORT:-18446}"
fixture_tls_hostname_port="${MICROSYSTEM_MICA_TLS_HOSTNAME_PORT:-18447}"
fixture_dns_port=53
tls_fixture="${MICROSYSTEM_MICA_TLS_FIXTURE:-0}"

if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]] || (( timeout_seconds == 0 )); then
  echo "mica-qemu: MICROSYSTEM_MICA_QEMU_TIMEOUT must be a positive integer" >&2
  exit 2
fi
if ! [[ "$stage_timeout" =~ ^[0-9]+$ ]] || (( stage_timeout == 0 )) ||
   ! [[ "$combo_timeout" =~ ^[0-9]+$ ]] || (( combo_timeout == 0 )); then
  echo "mica-qemu: stage/combo timeout must be positive integers" >&2
  exit 2
fi

if [[ ! -f /.dockerenv && "${MICROSYSTEM_IN_CONTAINER:-0}" != "1" ]]; then
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "mica-qemu: OrbStack Docker is required" >&2
    exit 2
  fi
  image="${IMAGE:-microsystem-dev:rust-1.97.1}"
  inner_log="/workspace/target/$(basename -- "$log_file")"
  exec docker run --rm -i --init \
    -e ARCH="$MICROSYSTEM_ARCH" \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -e MICROSYSTEM_IN_CONTAINER=1 \
    -e MICROSYSTEM_MICA_QEMU_TIMEOUT="$timeout_seconds" \
    -e MICROSYSTEM_MICA_INPUT_SETTLE="$input_settle" \
    -e MICROSYSTEM_MICA_STAGE_TIMEOUT="$stage_timeout" \
    -e MICROSYSTEM_MICA_COMBO_TIMEOUT="$combo_timeout" \
    -e MICROSYSTEM_MICA_TLS_FIXTURE="$tls_fixture" \
    -e MICROSYSTEM_MICA_QEMU_LOG="$inner_log" \
    -v "$repo_root:/workspace" -w /workspace "$image" bash scripts/mica-qemu.sh
fi

for command_name in cargo "$MICROSYSTEM_QEMU_BINARY" timeout python3; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "mica-qemu: $command_name is required inside the OrbStack container" >&2
    exit 2
  fi
done
if [[ "$tls_fixture" == 1 ]]; then
  for command_name in openssl sha256sum; do
    if ! command -v "$command_name" >/dev/null 2>&1; then
      echo "mica-qemu: $command_name is required for the TLS fixture" >&2
      exit 2
    fi
  done
fi
if [[ ! -f "target/$MICROSYSTEM_TARGET/release/microsystem-kernel" ||
      ! -f build/microsystem.img || ! -x target/release/mfsctl ]]; then
  echo "mica-qemu: build artifacts are missing; run make build first" >&2
  exit 2
fi

mkdir -p "$(dirname -- "$log_file")"
: >"$log_file"

fixture_dir="$repo_root/target/mica-fixture.$$"
fixture_log="$fixture_dir/server.log"
fixture_saved_log="$repo_root/target/mica-dns-fixture.log"
image_backup="$repo_root/target/mica-image-backup.$$"
test_image="$repo_root/target/mica-test-image.$$"
resolv_backup="$repo_root/target/mica-resolv-backup.$$"
artifact_backup_dir="$repo_root/target/mica-artifacts-backup.$$"
mfsctl="$repo_root/target/release/mfsctl"
fixture_pid=""
image_backed_up=0
resolv_configured=0
artifact_backed_up=0
test_bundle_hash=""
production_bundle_hash=""
restore_verify_failed=0

cleanup() {
  if [[ -n "$fixture_pid" ]] && kill -0 "$fixture_pid" 2>/dev/null; then
    kill "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  if [[ -f "$fixture_log" ]]; then
    cp "$fixture_log" "$fixture_saved_log" || true
  fi
  if [[ "$resolv_configured" == 1 && -f "$resolv_backup" ]]; then
    cp "$resolv_backup" /etc/resolv.conf || true
  fi
  rm -f "$resolv_backup"
  rm -f "$test_image"
  if [[ "$image_backed_up" == 1 && -f "$image_backup" ]]; then
    cp "$image_backup" build/microsystem.img || true
    if ! cmp -s "$image_backup" build/microsystem.img; then
      echo "mica-qemu: restored disk differs from backup" >&2
      restore_verify_failed=1
    fi
    rm -f "$image_backup"
  fi
  if [[ "$artifact_backed_up" == 1 && -d "$artifact_backup_dir" ]]; then
    for artifact in \
      "target/$MICROSYSTEM_TARGET/release/microsystem-mica-service" \
      "target/$MICROSYSTEM_TARGET/release/microsystem-kernel" \
      build/bootfs.cpio \
      build/ca-bundle.derpack; do
      backup="$artifact_backup_dir/$(basename -- "$artifact")"
      if [[ -f "$backup" ]]; then
        cp "$backup" "$artifact" || true
        if ! cmp -s "$backup" "$artifact"; then
          echo "mica-qemu: restored artifact differs: $artifact" >&2
          restore_verify_failed=1
        fi
      fi
    done
    rm -rf "$artifact_backup_dir"
  fi
  rm -rf "$fixture_dir"
}
trap cleanup EXIT

mkdir -p "$fixture_dir"
if [[ -e /etc/resolv.conf ]]; then
  cp -L /etc/resolv.conf "$resolv_backup"
else
  : >"$resolv_backup"
fi
if ! printf '%s\n' 'nameserver 127.0.0.1' >/etc/resolv.conf; then
  echo "mica-qemu: cannot configure temporary local DNS resolver" >&2
  exit 1
fi
resolv_configured=1
cp build/microsystem.img "$image_backup"
image_backed_up=1

if [[ "$tls_fixture" == 1 ]]; then
  production_bundle_hash="$(sha256sum build/ca-bundle.derpack | awk '{print $1}')"
  if [[ "${#production_bundle_hash}" -ne 64 ]]; then
    echo "mica-qemu: production CA bundle hash generation failed" >&2
    exit 1
  fi
  mkdir -p "$artifact_backup_dir"
  for artifact in \
    "target/$MICROSYSTEM_TARGET/release/microsystem-mica-service" \
    "target/$MICROSYSTEM_TARGET/release/microsystem-kernel" \
    build/bootfs.cpio \
    build/ca-bundle.derpack; do
    cp "$artifact" "$artifact_backup_dir/$(basename -- "$artifact")"
  done
  artifact_backed_up=1
  cp "$image_backup" "$test_image"

  make_ca() {
    local prefix="$1"
    local common_name="$2"
    local dns_name="${3:-}"
    local -a san_extension=()
    if [[ -n "$dns_name" ]]; then
      san_extension=(-addext "subjectAltName=DNS:$dns_name")
    fi
    openssl req -x509 -newkey rsa:2048 -nodes \
      -keyout "$fixture_dir/$prefix.key" -out "$fixture_dir/$prefix.pem" \
      -sha256 -days 3650 -set_serial 0x1001 \
      -subj "/CN=$common_name" \
      -addext 'basicConstraints=critical,CA:TRUE,pathlen:1' \
      -addext 'keyUsage=critical,keyCertSign,cRLSign' \
      -addext 'subjectKeyIdentifier=hash' "${san_extension[@]}" >/dev/null 2>&1
  }

  make_leaf() {
    local name="$1"
    local ca_prefix="$2"
    local common_name="$3"
    local dns_name="$4"
    local start_date="$5"
    local end_date="$6"
    local serial="$7"
    local key_type="${8:-rsa}"
    local config="$fixture_dir/$name.cnf"
    mkdir -p "$fixture_dir/$name-newcerts"
    : >"$fixture_dir/$name.index"
    printf '%s\n' "$serial" >"$fixture_dir/$name.serial"
    if [[ "$key_type" == "p384" ]]; then
      openssl ecparam -name secp384r1 -genkey -noout \
        -out "$fixture_dir/$name.key" >/dev/null 2>&1
      openssl req -new -key "$fixture_dir/$name.key" \
        -out "$fixture_dir/$name.csr" -sha384 -subj "/CN=$common_name" >/dev/null 2>&1
    else
      openssl req -newkey rsa:2048 -nodes \
        -keyout "$fixture_dir/$name.key" -out "$fixture_dir/$name.csr" \
        -sha256 -subj "/CN=$common_name" >/dev/null 2>&1
    fi
    cat >"$config" <<EOF
[ca]
default_ca = CA_default
[CA_default]
database = $fixture_dir/$name.index
new_certs_dir = $fixture_dir/$name-newcerts
serial = $fixture_dir/$name.serial
certificate = $fixture_dir/$ca_prefix.pem
private_key = $fixture_dir/$ca_prefix.key
default_md = sha256
default_days = 3650
policy = policy_any
[policy_any]
commonName = supplied
[server_ext]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names
[alt_names]
DNS.1 = $dns_name
EOF
    openssl ca -batch -config "$config" -in "$fixture_dir/$name.csr" \
      -out "$fixture_dir/$name.pem" -startdate "$start_date" -enddate "$end_date" \
      -extensions server_ext >/dev/null 2>&1
  }

  trusted_ca_dns_name='root-long-san-for-microsystem-tls-parser-0123456789abcd.example.test'
  make_ca test-ca 'MicroSystem TLS fixture CA' "$trusted_ca_dns_name"
  make_ca unknown-ca 'MicroSystem unknown TLS CA'
  make_leaf trusted test-ca mica.test mica.test 200101000000Z 400101000000Z 2001 p384
  make_leaf unknown unknown-ca mica.test mica.test 200101000000Z 400101000000Z 2002
  make_leaf expired test-ca mica.test mica.test 100101000000Z 200101000000Z 2003
  make_leaf future test-ca mica.test mica.test 400101000000Z 500101000000Z 2004
  make_leaf hostname test-ca wrong.test wrong.test 200101000000Z 400101000000Z 2005
  cat "$fixture_dir/trusted.pem" "$fixture_dir/test-ca.pem" >"$fixture_dir/trusted-chain.pem"
  cp "$fixture_dir/trusted.key" "$fixture_dir/trusted-chain.key"
  if ! openssl x509 -in "$fixture_dir/trusted.pem" -noout -text |
    grep -Fq -- 'ASN1 OID: secp384r1'; then
    echo "mica-qemu: TLS fixture trusted leaf is not secp384r1" >&2
    exit 1
  fi
  if [[ "$(grep -c -- 'BEGIN CERTIFICATE' "$fixture_dir/trusted-chain.pem")" -ne 2 ]] ||
     ! openssl verify -CAfile "$fixture_dir/test-ca.pem" "$fixture_dir/trusted.pem" >/dev/null 2>&1; then
    echo "mica-qemu: TLS fixture trusted leaf/root chain is invalid" >&2
    exit 1
  fi
  openssl x509 -in "$fixture_dir/test-ca.pem" -outform DER -out "$fixture_dir/test-ca.der"
  test_bundle="$fixture_dir/test-ca.derpack"
  python3 - "$fixture_dir/test-ca.der" "$test_bundle" <<'PY'
import struct
import sys

der = open(sys.argv[1], "rb").read()
entries = 121
payload = b"".join(struct.pack("<I", len(der)) + der for _ in range(entries))
bundle = b"MCAB" + struct.pack("<III", 1, entries, len(payload)) + payload
open(sys.argv[2], "wb").write(bundle)
PY
  test_ca_subject_alt_name="$(openssl x509 -in "$fixture_dir/test-ca.pem" -noout -ext subjectAltName)"
  test_ca_dns_name="$(printf '%s\n' "$test_ca_subject_alt_name" |
    sed -n 's/.*DNS:\([^,[:space:]]*\).*/\1/p')"
  test_ca_name_bytes="$(printf '%s' "$test_ca_dns_name" | wc -c | tr -d '[:space:]')"
  if [[ "$test_ca_dns_name" != "$trusted_ca_dns_name" ]] || (( test_ca_name_bytes <= 64 )); then
    echo "mica-qemu: TLS fixture trusted CA DNS SAN is not longer than 64 bytes" >&2
    exit 1
  fi
  test_bundle_hash="$(sha256sum "$test_bundle" | awk '{print $1}')"
  if [[ "${#test_bundle_hash}" -ne 64 ]]; then
    echo "mica-qemu: TLS fixture bundle hash generation failed" >&2
    exit 1
  fi
  if ! python3 - \
    "target/$MICROSYSTEM_TARGET/release/microsystem-kernel" \
    "$production_bundle_hash" "$test_bundle_hash" <<'PY'
from pathlib import Path
import sys

kernel_path = Path(sys.argv[1])
old_hash = bytes.fromhex(sys.argv[2])
new_hash = bytes.fromhex(sys.argv[3])
data = kernel_path.read_bytes()
matches = data.count(old_hash)
if matches != 1:
    print(f"kernel CA bundle hash occurrences={matches}, expected exactly 1", file=sys.stderr)
    raise SystemExit(1)
patched = data.replace(old_hash, new_hash, 1)
if patched.count(old_hash) != 0:
    print("kernel CA bundle hash replacement left the production hash", file=sys.stderr)
    raise SystemExit(1)
kernel_path.write_bytes(patched)
PY
  then
    echo "mica-qemu: kernel CA bundle hash replacement failed" >&2
    exit 1
  fi
  if ! "$mfsctl" put "$test_image" "$test_bundle" \
    /.system/certs/ca-bundle.derpack >"$fixture_dir/test-bundle-put.log" 2>&1; then
    echo "mica-qemu: TLS fixture bundle injection failed" >&2
    cat "$fixture_dir/test-bundle-put.log" >&2 || true
    exit 1
  fi
  cp "$test_image" build/microsystem.img
fi

tls_policy_lines=""
tls_request_args=""
tls_probe=""
tls_combo_marker=""
if [[ "$tls_fixture" == 1 ]]; then
  tls_policy_lines=$(cat <<EOF
--!allow net.connect:mica.test:$fixture_tls_port
--!allow net.connect:mica.test:$fixture_tls_unknown_port
--!allow net.connect:mica.test:$fixture_tls_expired_port
--!allow net.connect:mica.test:$fixture_tls_future_port
--!allow net.connect:mica.test:$fixture_tls_hostname_port
EOF
)
  tls_request_args=" --allow net.connect:mica.test:$fixture_tls_port --allow net.connect:mica.test:$fixture_tls_unknown_port --allow net.connect:mica.test:$fixture_tls_expired_port --allow net.connect:mica.test:$fixture_tls_future_port --allow net.connect:mica.test:$fixture_tls_hostname_port"
  tls_combo_marker=" https=true tls-unknown-ca=true tls-expired=true tls-not-yet-valid=true tls-hostname-mismatch=true"
  tls_probe=$(cat <<EOF
local secure_response, secure_error = http.get("https://mica.test:$fixture_tls_port/index.txt")
if secure_error ~= nil or secure_response == nil then
  return network_failure("https-trusted", secure_error)
end
local secure_body, secure_read_error = secure_response.read_all(secure_response)
if secure_read_error ~= nil or secure_body == nil then
  return network_failure("https-read", secure_read_error)
end
local secure_text, secure_text_error = bytes.to_string(secure_body)
if secure_text_error ~= nil or secure_text ~= "mica-https-ok" then
  return network_failure("https-body", secure_text_error)
end
function expect_tls_failure(label, url)
  local response, error = http.get(url)
  if response ~= nil or error == nil then
    return network_failure("https-" + label, error)
  end
  print("mica-https " + label + "=rejected")
end
expect_tls_failure("unknown-ca", "https://mica.test:$fixture_tls_unknown_port/index.txt")
expect_tls_failure("expired", "https://mica.test:$fixture_tls_expired_port/index.txt")
expect_tls_failure("not-yet-valid", "https://mica.test:$fixture_tls_future_port/index.txt")
expect_tls_failure("hostname-mismatch", "https://mica.test:$fixture_tls_hostname_port/index.txt")
print("mica-https trusted=true body=mica-https-ok tls-trusted-root-chain=true tls-p384=true tls-ca-name-long=true")
EOF
)
fi

printf '%s\n' 'return {value = "helper-cache"}' >"$fixture_dir/helper.mica"
cat >"$fixture_dir/main.mica" <<MICA
--!mica 1
--!allow fs.read:/mica
--!allow fs.write:/mica
--!allow net.connect:mica.test:53
--!allow net.browse
--!allow net.connect:10.0.2.2:$fixture_tcp_port
--!allow net.connect:10.0.2.2:$fixture_udp_port
$tls_policy_lines

local args = require("args")
local bytes = require("bytes")
local fs = require("fs")
local net = require("net")
local http = require("http")
local helper_a = require("helper")
local helper_b = require("helper")
assert(helper_a.value == helper_b.value, "same-directory module cache")
assert(args.get(1) == "alpha" and args.get(2) == "beta", "argument forwarding")
local all = args.all()
assert(all[1] == "alpha" and all[2] == "beta", "argument table")

local denied, denial = fs.read_file("/outside")
assert(denied == nil and denial ~= nil, "path permission denial")
local written, write_error = fs.write_file("/mica/atomic.txt", "mica-atomic-ok", {atomic = true, fsync = true})
assert(written == true and write_error == nil, "atomic fsync write")
local saved = fs.read_file("/mica/atomic.txt")
assert(bytes.to_string(saved) == "mica-atomic-ok", "atomic readback")

function network_failure(stage, error)
  local message = "invalid result"
  local kind = "none"
  local operation = "none"
  local code = "none"
  if error ~= nil then
    message = error.message
    kind = error.kind
    operation = error.operation
    code = tostring(error.code)
  end
  print("mica network error stage=" + stage + " message=" + message + " kind=" + kind + " operation=" + operation + " code=" + code)
  return 1
end

local tcp, tcp_error = net.tcp_connect("10.0.2.2", $fixture_tcp_port)
if tcp_error ~= nil or tcp == nil then
  return network_failure("tcp-connect", tcp_error)
end
local tcp_written, tcp_write_error = net.tcp_write(tcp, bytes.from_string("mica-tcp-ok"))
if tcp_write_error ~= nil or tcp_written == nil then
  return network_failure("tcp-write", tcp_write_error)
end
local tcp_reply, tcp_read_error = net.tcp_read(tcp, 64)
if tcp_read_error ~= nil or tcp_reply == nil then
  return network_failure("tcp-read", tcp_read_error)
end
local tcp_closed, tcp_close_error = net.tcp_close(tcp)
if tcp_close_error ~= nil or tcp_closed ~= true then
  return network_failure("tcp-close", tcp_close_error)
end
local tcp_text, tcp_text_error = bytes.to_string(tcp_reply)
if tcp_text_error ~= nil or tcp_text ~= "mica-tcp-ok" then
  return network_failure("tcp-echo", tcp_text_error)
end

local address, address_error = net.resolve("mica.test")
if address_error ~= nil or type(address) ~= "string" then
  return network_failure("dns-resolve", address_error)
end

local udp, udp_error = net.udp_open()
if udp_error ~= nil or udp == nil then
  return network_failure("udp-open", udp_error)
end
local udp_sent, udp_send_error = net.udp_send_to(udp, "10.0.2.2", $fixture_udp_port, bytes.from_string("mica-udp-ok"))
if udp_send_error ~= nil or udp_sent == nil then
  return network_failure("udp-send", udp_send_error)
end
local udp_reply, udp_read_error = net.udp_recv_from(udp, 64)
if udp_read_error ~= nil or udp_reply == nil then
  return network_failure("udp-recv", udp_read_error)
end
local browse_udp_sent, browse_udp_error = net.udp_send_to(udp, "browse.test", $fixture_http_port, bytes.from_string("browse-udp"))
if browse_udp_sent ~= nil or browse_udp_error == nil or browse_udp_error.kind ~= "access" then
  return network_failure("browse-raw-udp", browse_udp_error)
end
local udp_closed, udp_close_error = net.udp_close(udp)
if udp_close_error ~= nil or udp_closed ~= true then
  return network_failure("udp-close", udp_close_error)
end
local udp_text, udp_text_error = bytes.to_string(udp_reply.data)
if udp_text_error ~= nil or udp_text ~= "mica-udp-ok" then
  return network_failure("udp-echo", udp_text_error)
end

local response, http_error = http.get("http://10.0.2.2:$fixture_http_port/index.txt?browse=1#fragment")
if http_error ~= nil or response == nil then
  return network_failure("http-get", http_error)
end
local body, http_read_error = response.read_all(response)
if http_read_error ~= nil or body == nil then
  return network_failure("http-read", http_read_error)
end
local body_text, http_text_error = bytes.to_string(body)
if http_text_error ~= nil or body_text ~= "mica-http-ok" then
  return network_failure("http-body", http_text_error)
end
local browse_address, browse_resolve_error = net.resolve("browse.test")
if browse_address ~= nil or browse_resolve_error == nil or browse_resolve_error.kind ~= "access" then
  return network_failure("browse-raw-resolve", browse_resolve_error)
end
local browse_tcp, browse_tcp_error = net.tcp_connect("browse.test", $fixture_http_port)
if browse_tcp ~= nil or browse_tcp_error == nil or browse_tcp_error.kind ~= "access" then
  return network_failure("browse-raw-tcp", browse_tcp_error)
end
local browse_post, browse_post_error = http.post("http://10.0.2.2:$fixture_http_port/index.txt", bytes.from_string("browse-post"))
if browse_post ~= nil or browse_post_error == nil or browse_post_error.kind ~= "access" then
  return network_failure("browse-post", browse_post_error)
end
$tls_probe
print("mica-combo args=alpha,beta module-cache=true fs-permission-denied=true fs-atomic=true dns=true tcp=true udp=true http=true browse=true browse-resolve-denied=true browse-raw-denied=true browse-udp-denied=true browse-post-denied=true$tls_combo_marker")
MICA

"$mfsctl" mkdir build/microsystem.img /mica >"$fixture_dir/mkdir.log" 2>&1
"$mfsctl" put build/microsystem.img "$fixture_dir/helper.mica" /mica/helper.mica >"$fixture_dir/helper-put.log" 2>&1
"$mfsctl" put build/microsystem.img "$fixture_dir/main.mica" /mica/main.mica >"$fixture_dir/main-put.log" 2>&1

python3 -u - "$fixture_http_port" "$fixture_tcp_port" "$fixture_udp_port" "$fixture_dns_port" \
  "$tls_fixture" "$fixture_dir" "$fixture_tls_port" "$fixture_tls_unknown_port" \
  "$fixture_tls_expired_port" "$fixture_tls_future_port" "$fixture_tls_hostname_port" \
  >"$fixture_log" 2>&1 <<'PY' &
import http.server
import signal
import socketserver
import ssl
import sys
import threading

http_port, tcp_port, udp_port, dns_port = (int(value) for value in sys.argv[1:5])
tls_enabled = sys.argv[5] == "1"
tls_dir = sys.argv[6]
tls_ports = [int(value) for value in sys.argv[7:12]]

class HttpHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        print(f"http GET {self.path}", flush=True)
        body = b"mica-http-ok"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        print(f"http POST {self.path}", flush=True)
        self.send_error(405)
    def log_message(self, *_args):
        pass

class TlsHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        print(f"tls request path={self.path}", flush=True)
        body = b"mica-https-ok"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args):
        pass

class TcpHandler(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.sendall(self.request.recv(32768))

class UdpHandler(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        sock.sendto(data, self.client_address)

class DnsHandler(socketserver.BaseRequestHandler):
    def handle(self):
        data, sock = self.request
        if len(data) < 12:
            return
        offset = 12
        labels = []
        while offset < len(data):
            length = data[offset]
            offset += 1
            if length == 0:
                break
            if length & 0xc0 or offset + length > len(data):
                return
            labels.append(data[offset:offset + length].decode("ascii", "ignore"))
            offset += length
        if offset + 4 > len(data):
            return
        question = data[12:offset + 4]
        qtype = int.from_bytes(data[offset:offset + 2], "big")
        qclass = int.from_bytes(data[offset + 2:offset + 4], "big")
        name = ".".join(labels).lower()
        target = name == "mica.test" and qtype == 1 and qclass == 1
        answer = (
            b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\x1e\x00\x04\x0a\x00\x02\x02"
            if target
            else b""
        )
        header = (
            data[:2]
            + b"\x81\x80"
            + b"\x00\x01"
            + (b"\x00\x01" if target else b"\x00\x00")
            + b"\x00\x00\x00\x00"
        )
        print(
            f"dns query name={name} type={qtype} answer={'10.0.2.2' if target else 'none'}",
            flush=True,
        )
        sock.sendto(header + question + answer, self.client_address)

class ReusableTcp(socketserver.ThreadingTCPServer):
    allow_reuse_address = True

def tls_server(port, certificate):
    server = ReusableTcp(("0.0.0.0", port), TlsHandler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.maximum_version = ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(
        certfile=f"{tls_dir}/{certificate}.pem",
        keyfile=f"{tls_dir}/{certificate}.key",
    )
    print(f"tls server port={port} certificate={certificate}", flush=True)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    return server

servers = [
    http.server.ThreadingHTTPServer(("0.0.0.0", http_port), HttpHandler),
    ReusableTcp(("0.0.0.0", tcp_port), TcpHandler),
    socketserver.ThreadingUDPServer(("0.0.0.0", udp_port), UdpHandler),
    socketserver.ThreadingUDPServer(("127.0.0.1", dns_port), DnsHandler),
]
if tls_enabled:
    for port, certificate in zip(
        tls_ports,
        ("trusted-chain", "unknown", "expired", "future", "hostname"),
    ):
        servers.append(tls_server(port, certificate))
for server in servers:
    threading.Thread(target=server.serve_forever, daemon=True).start()
signal.pause()
PY
fixture_pid=$!
sleep 0.2
if ! kill -0 "$fixture_pid" 2>/dev/null; then
  echo "mica-qemu: local DNS/TCP/UDP/HTTP fixture failed to start" >&2
  cat "$fixture_log" >&2 || true
  exit 1
fi

send_input() {
  log_count() {
    local pattern="$1"
    { grep -Eo -- "$pattern" "$log_file" 2>/dev/null || true; } | wc -l | tr -d ' '
  }
  standalone_42_count() {
    tr -d '\r' <"$log_file" | awk '$0 == "42" { count += 1 } END { print count + 0 }'
  }
  wait_for_count() {
    local pattern="$1"
    local expected="$2"
    local label="$3"
    local limit="$stage_timeout"
    if [[ "$label" == combo-* ]]; then
      limit="$combo_timeout"
    fi
    local deadline=$((SECONDS + limit))
    while (( SECONDS < deadline )); do
      local count
      count="$(log_count "$pattern")"
      if [[ "$count" =~ ^[0-9]+$ ]] && (( count >= expected )); then
        return 0
      fi
      sleep 0.05
    done
    echo "mica-qemu: input stage timeout label=$label expected=$expected pattern=$pattern" >&2
    return 1
  }
  wait_for_42() {
    local expected="$1"
    local label="$2"
    local deadline=$((SECONDS + stage_timeout))
    while (( SECONDS < deadline )); do
      local count
      count="$(standalone_42_count)"
      if [[ "$count" =~ ^[0-9]+$ ]] && (( count >= expected )); then
        return 0
      fi
      sleep 0.05
    done
    echo "mica-qemu: input stage timeout label=$label expected=$expected standalone=42" >&2
    return 1
  }
  send_line() {
    printf '%s\n' "$1"
    sleep "$input_settle"
  }

  wait_for_count 'micro>[[:space:]]' 1 initial-prompt || return 1
  send_line "mica -e 'print(40 + 2)'"
  wait_for_count 'mica:[[:space:]]+pid=[0-9]+[[:space:]]+status=0' 1 eval-status || return 1
  wait_for_count 'micro>[[:space:]]' 2 eval-prompt || return 1

  send_line 'mica'
  wait_for_count 'mica>[[:space:]]' 1 repl-prompt || return 1
  send_line 'print(6 * 7)'
  wait_for_42 2 repl-output || return 1
  wait_for_count 'mica>[[:space:]]' 2 repl-output-prompt || return 1
  send_line 'exit'
  wait_for_count 'mica:[[:space:]]+pid=[0-9]+[[:space:]]+status=0' 2 repl-status || return 1
  wait_for_count 'micro>[[:space:]]' 3 repl-exit-prompt || return 1

  send_line "mica --allow fs.read:/mica --allow fs.write:/mica --allow net.connect:mica.test:53 --allow net.browse --allow net.connect:10.0.2.2:$fixture_tcp_port --allow net.connect:10.0.2.2:$fixture_udp_port$tls_request_args /mica/main.mica -- alpha beta"
  wait_for_count 'mica-combo[[:space:]]+args=alpha,beta' 1 combo-marker || return 1
  wait_for_count 'mica:[[:space:]]+pid=[0-9]+[[:space:]]+status=0' 3 combo-status || return 1
  wait_for_count 'micro>[[:space:]]' 4 combo-prompt || return 1

  send_line "curl -i http://10.0.2.2:$fixture_http_port/index.txt?curl=stdout"
  wait_for_count 'HTTP status=200' 1 curl-status || return 1
  wait_for_count 'mica-http-ok' 1 curl-body || return 1
  wait_for_count 'micro>[[:space:]]' 5 curl-prompt || return 1

  send_line "curl -s -o /mica/curl.txt http://10.0.2.2:$fixture_http_port/index.txt?curl=file"
  wait_for_count 'curl:[[:space:]]+saved[[:space:]]+/mica/curl.txt' 1 curl-save || return 1
  wait_for_count 'micro>[[:space:]]' 6 curl-save-prompt || return 1
  send_line 'cat /mica/curl.txt'
  wait_for_count 'mica-http-ok' 2 curl-file-body || return 1
  wait_for_count 'micro>[[:space:]]' 7 curl-file-prompt || return 1

  send_line 'run mica'
  wait_for_count 'run:[[:space:]]+mica:[[:space:]]+not[[:space:]]+found' 1 run-rejection || return 1
  wait_for_count 'micro>[[:space:]]' 8 run-prompt || return 1
  send_line 'shutdown'
  wait_for_count '\[system\][[:space:]]+shutdown' 1 shutdown || return 1
}

set +e
if [[ -x target/debug/xtask ]]; then
  qemu_command=(target/debug/xtask qemu)
else
  qemu_command=(cargo run -p xtask -- qemu)
fi
send_input | timeout --signal=TERM --kill-after=5s "${timeout_seconds}s" \
  "${qemu_command[@]}" >"$log_file" 2>&1
status=$?
set -e

if [[ "$status" -eq 124 || "$status" -eq 137 ]]; then
  echo "mica-qemu: guest did not reach shutdown within ${timeout_seconds}s" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

required_markers=(
  "\\[bootfs\\][[:space:]]+valid=true[[:space:]]+entries=25[[:space:]]+static-elfs=24"
  "\\[service\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+address-spaces=13[[:space:]]+asids=\\[0x20\\.\\.0x2c\\]"
  "\\[proc\\][[:space:]]+dynamic[[:space:]]+application[[:space:]]+capacity=8[[:space:]]+first-pid=14[[:space:]]+independent-slots=true"
  "\\[mm\\][[:space:]]+TaskMemory/page[[:space:]]+tables[[:space:]]+allocated[[:space:]]+from[[:space:]]+kernel[[:space:]]+heap[[:space:]]+slot-bytes=0x[[:xdigit:]]+[[:space:]]+allocated=0x[[:xdigit:]]+"
  "\\[service\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+ready=8/8[[:space:]]+online=8/8[[:space:]]+switches=[0-9]+"
  "\\[service\\][[:space:]]+shell[[:space:]]+ready"
  "\\[mica\\][[:space:]]+session[[:space:]]+isolated=true[[:space:]]+brokers=fs,network,process[[:space:]]+time=true"
  "\\[mica\\][[:space:]]+vm[[:space:]]+verified=true[[:space:]]+gc=mark-sweep[[:space:]]+pcall=true[[:space:]]+modules=true"
  "\\[net\\][[:space:]]+netd[[:space:]]+ready[[:space:]]+ipv4=10.0.2.15[[:space:]]+outbound=true[[:space:]]+raw-device-isolated=true[[:space:]]+tcp-owner=true[[:space:]]+dns=true[[:space:]]+tcp=64[[:space:]]+udp=8"
  "mica:[[:space:]]+pid=[0-9]+[[:space:]]+status=0"
  "micro>[[:space:]]+mica"
  "mica>[[:space:]]+"
  "print\(6[[:space:]]+\*[[:space:]]+7\)"
  "run:[[:space:]]+mica:[[:space:]]+not[[:space:]]+found"
  "HTTP[[:space:]]+status=200"
  "curl:[[:space:]]+saved[[:space:]]+/mica/curl.txt"
  "mica-combo[[:space:]]+args=alpha,beta[[:space:]]+module-cache=true[[:space:]]+fs-permission-denied=true[[:space:]]+fs-atomic=true[[:space:]]+dns=true[[:space:]]+tcp=true[[:space:]]+udp=true[[:space:]]+http=true[[:space:]]+browse=true[[:space:]]+browse-resolve-denied=true[[:space:]]+browse-raw-denied=true[[:space:]]+browse-udp-denied=true[[:space:]]+browse-post-denied=true"
)
if [[ "$tls_fixture" == 1 ]]; then
  required_markers+=(
    "mica-https[[:space:]]+trusted=true[[:space:]]+body=mica-https-ok[[:space:]]+tls-trusted-root-chain=true[[:space:]]+tls-p384=true[[:space:]]+tls-ca-name-long=true"
    "mica-https[[:space:]]+unknown-ca=rejected"
    "mica-https[[:space:]]+expired=rejected"
    "mica-https[[:space:]]+not-yet-valid=rejected"
    "mica-https[[:space:]]+hostname-mismatch=rejected"
    "mica-combo[[:space:]]+.*https=true[[:space:]]+tls-unknown-ca=true[[:space:]]+tls-expired=true[[:space:]]+tls-not-yet-valid=true[[:space:]]+tls-hostname-mismatch=true"
  )
  if ! grep -Fq -- "tls server port=$fixture_tls_port certificate=trusted-chain" "$fixture_log"; then
    echo "mica-qemu: TLS fixture did not serve the trusted leaf/root chain" >&2
    tail -n 80 "$fixture_log" >&2 || true
    exit 1
  fi
fi
missing=()
for marker in "${required_markers[@]}"; do
  if ! grep -Eqi -- "$marker" "$log_file"; then
    missing+=("$marker")
  fi
done
if (( $(grep -E 'mica:[[:space:]]+pid=[0-9]+[[:space:]]+status=0' "$log_file" | wc -l | tr -d ' ') < 2 )); then
  missing+=("two Mica status=0 completions (eval and REPL)")
fi
if (( $(tr -d '\r' <"$log_file" | awk '$0 == "42" { found += 1 } END { print found + 0 }') < 2 )); then
  missing+=("two standalone Mica output lines 42 (eval and REPL)")
fi
if ((${#missing[@]} != 0)); then
  echo "mica-qemu: missing serial Mica milestones: ${missing[*]}" >&2
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi
if ! grep -Fq -- "http GET /index.txt?browse=1" "$fixture_log"; then
  echo "mica-qemu: browse GET query/fragment contract missing from HTTP fixture log" >&2
  tail -n 80 "$fixture_log" >&2 || true
  exit 1
fi
if ! grep -Fq -- "http GET /index.txt?curl=stdout" "$fixture_log" \
  || ! grep -Fq -- "http GET /index.txt?curl=file" "$fixture_log"; then
  echo "mica-qemu: curl GET fixture requests missing" >&2
  tail -n 80 "$fixture_log" >&2 || true
  exit 1
fi
if grep -Fq -- "http POST /index.txt" "$fixture_log"; then
  echo "mica-qemu: browse-only permission unexpectedly reached HTTP POST fixture" >&2
  tail -n 80 "$fixture_log" >&2 || true
  exit 1
fi
if grep -Eiq '\[panic\]|\[service\][[:space:]]+critical[[:space:]]+service|translation fault|DMA fault' "$log_file"; then
  echo "mica-qemu: panic/service/DMA fault appeared in serial log" >&2
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi

echo "mica-qemu: PASS bootfs=25/24 first-pid=14 mica-output=42 status=0 run-mica=not-found log=$log_file"
