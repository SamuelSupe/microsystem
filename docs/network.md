# Network and netd

`netd` is the sole EL0 owner of the raw VirtIO-net device. Its boot capability
is `NETWORK_DEVICE` (slot 58, generation 1, `READ|WRITE`), and the kernel only
allows task 11 (`netd`) to invoke `NetReceive=27` and `NetSend=28`. Every other
service, including Mica and SSHD, uses endpoint protocol 9 (`NETWORK`). This
is the boundary represented by the final marker:

```text
[net] netd ready ipv4=10.0.2.15 outbound=true raw-device-isolated=true tcp-owner=true dns=true tcp=64 udp=8
```

The VirtIO-net RX path is deliberately bounded; the final GUI/SSH acceptance
marker reports `rx-buffers=2`.

## Guest network

netd configures smoltcp with:

```text
MAC       52:54:00:12:34:56
address   10.0.2.15/24
gateway   10.0.2.2
DNS       10.0.2.3:53 (local socket 53053)
SSH       TCP/22
```

The service owns 64 outbound TCP sockets, 8 UDP sockets and 2 SSH listener
sockets. There are at most 8 script network sessions, each with 8 logical
connections. Requests and payloads are capped at 32 KiB by the ABI/session
broker. DNS is an A-record query path with per-session transaction IDs and
response-name checking; the Mica fixture resolves `mica.test` to `10.0.2.2`.

## Endpoint operations

Protocol 9 operations are:

```text
OpenSession=1 CloseSession=2 Resolve=3
TcpConnect=4 TcpRead=5 TcpWrite=6 TcpClose=7
UdpOpen=8 UdpSendTo=9 UdpRecvFrom=10 UdpClose=11
TlsConnect=12 Cancel=13
SshListen=14 SshAccept=15 SshSharedFrame=16
SshReceive=17 SshSend=18 SshClose=19 SshStatus=20
```

Mica's `net.resolve`, TCP and UDP wrappers authenticate a per-process token,
map that process's session region only while servicing a request, and yield on
`Busy` until a 10 s operation deadline. Raw network operations are gated by
case-insensitive exact `net.connect:host:port` rules; no wildcard or arbitrary
raw socket is exposed. The unscoped `net.browse` permission is narrower than
it sounds: only the Mica HTTP GET path may use it to resolve/connect a dynamic
host, and the private broker marker is not available to script-level resolve,
TCP or UDP calls. POST/PUT/PATCH/DELETE likewise still require exact
`net.connect` rules. DNS resolve may use a host rule without pinning a specific
port, but TCP/UDP connect/send still require the exact destination port.

## TLS boundary

The broker's `TlsConnect=12` operation returns `NotSupported`. Trusted HTTPS is
implemented one layer up by `services/mica`: it resolves/connects with netd
TCP, then runs a TLS 1.3 client with PL031 realtime, Mozilla trust roots,
hostname checking and RSA-PSS plus P-384 ECDSA CertificateVerify verification.
If the server's final presented certificate equals the selected trust anchor,
the verifier omits that redundant tail before chain validation. `net.browse`
uses exactly this HTTPS path; it changes destination authorization for GET but
does not weaken roots, time, hostname or body/content limits. See
[docs/mica.md](mica.md) for the bundle identity, CA read path and HTTPS limits.
The unified Mica stage sets `MICROSYSTEM_MICA_TLS_FIXTURE=1`; its trusted body is
`mica-https-ok`, while unknown-CA, expired, not-yet-valid and hostname-mismatch
certificates are rejected.

The guest MCAB trust bundle keeps the pinned Mozilla-derived 121-root snapshot,
then appends deduplicated certificates from the build container's
`SSL_CERT_FILE`. `xtask` creates it before compiling the Mica service and
injects its full SHA-256; the runtime verifies the complete bundle before the
trusted read. Each host certificate is limited to 16 KiB and the complete
bundle to 256 KiB.

## Remaining boundaries

There is no low-level TLS socket, HTTP/2, proxy, cookie or streaming response
API. HTTP returns the first `Content-Type` and `Location` values as bounded
response-table fields (not an arbitrary header map); it never follows a
redirect automatically. The Mica Reader example uses those fields to render
HTML/text and expose a redirect as an explicit link. Its page links are
same-origin, while an explicit address-bar URL may select another HTTP(S)
origin through `net.browse`; fragments are stripped before the request. netd
remains a bounded single-owner dataplane: no arbitrary scatter/gather,
multiple network domains, hotplug or generic device drivers are promised. The
SSH protocol is a separate endpoint consumer of netd's two listener sockets;
its command and key contract is in [docs/ssh.md](ssh.md).

The final OrbStack fixtures recorded `GET /index.txt?browse=1` after dropping a
URL fragment, plus DNS resolution for `mica.test`. The Mica combo marker
confirmed `browse=true`, raw resolve/TCP/UDP and POST denials, and trusted TLS
`mica-https-ok` with a presented root-chain, P-384 `CertificateVerify` and a
long CA DNS name; unknown-CA, expired, not-yet-valid and hostname-mismatch
rejections were also observed. The unified Mica/TLS PASS is recorded in
`target/unified-fp-simd-final.log`; browser/network fixture details are in
`target/mica-dns-fixture.log`, `target/mica-qemu.log`,
`target/unified-browser-public-internet-final.log` and
`target/unified-browser-public-internet-fsck.log`.
