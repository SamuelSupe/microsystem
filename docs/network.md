# Network and netd

`netd` is the sole EL0 owner of the raw VirtIO-net device. Its boot capability
is `NETWORK_DEVICE` (slot 58, generation 1, `READ|WRITE`), and the kernel only
allows task 11 (`netd`) to invoke `NetReceive=27`, `NetSend=28` and
`NetworkInterfaceInfo=44`. Every other
service, including Mica and SSHD, uses endpoint protocol 9 (`NETWORK`). This
is the boundary represented by the final marker:

```text
[net] netd ready ipv4=10.0.2.15 outbound=true raw-device-isolated=true tcp-owner=true dns=true tcp=64 udp=8 interface=0
```

The VirtIO-net RX path is deliberately bounded; the final GUI/SSH acceptance
marker reports `rx-buffers=2`.

## Guest network

netd maintains up to four independent smoltcp interfaces. Each has its own
MAC, IP addresses, routes, DHCP client, DNS socket and connection buffers.
The default is DHCPv4 and IPv6 SLAAC with an EUI-64 link-local address.
On QEMU's first default user network the resulting lease is normally:

```text
MAC       52:54:00:12:34:56
address   10.0.2.15/24
gateway   10.0.2.2
DNS       10.0.2.3:53 (local socket 53053)
SSH       TCP/22
```

The service has buffers for 64 outbound TCP sockets, 8 UDP sockets and 2 SSH
listeners per interface. A global limit admits 64 script connections, including
at most 8 UDP sockets; one interface can use the complete budget. There are at
most 8 script network sessions, each with 8 logical
connections. Requests and payloads are capped at 32 KiB by the ABI/session
broker. DNS supports A and AAAA queries with per-session transaction IDs,
question-name/type checking and bounded packet parsing; the Mica fixture resolves `mica.test` to `10.0.2.2`.
UDP payloads are limited to the unfragmented IP MTU: 1472 IPv4 or 1452 IPv6
bytes. Oversized datagrams fail before queueing.
TCP and UDP accept IPv6 literals and AAAA-only hostnames. `net.resolve(host, "ipv6")` selects AAAA; ordinary connections try A then AAAA if no A answer exists. `net.udp_open(port, interface)` optionally
selects a zero-based hardware interface; omitting it selects the default.
TCP uses the longest matching connected prefix, then the preferred available
interface. Interfaces with a down link are excluded. Connection handles include
a generation so a closed or reset handle cannot access a later connection.

## Persistent configuration

The serial shell and Terminal expose:

```text
net status
net config
net dhcp 52:54:00:12:34:56
net static 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 10.0.2.3
net static 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 10.0.2.3 fec0::20/64 fe80::2
net default 52:54:00:12:34:57
net reset
```

Use `-` for an omitted gateway or DNS server, and `auto`/`off` for the IPv6
address field. Configurations are keyed by MAC and fsynced to
`/.system/network.conf` before applying them. Invalid addresses, duplicate MACs,
multicast/unspecified DNS servers and IPv4 gateways outside the configured
subnet are rejected. A corrupt saved configuration falls back to DHCP/SLAAC
with a diagnostic. Configuration changes, link transitions, hardware epochs
and replacement DHCP addresses invalidate affected connections. `netd` depends
on MFS readiness; storage recovery restarts netd and SSHD to reload configuration.
Readiness acknowledges an initialized service; address acquisition continues
asynchronously and absence of a DHCP server does not cause a restart loop.

## Endpoint operations

Protocol 9 operations are:

```text
OpenSession=1 CloseSession=2 Resolve=3
TcpConnect=4 TcpRead=5 TcpWrite=6 TcpClose=7
UdpOpen=8 UdpSendTo=9 UdpRecvFrom=10 UdpClose=11
TlsConnect=12 Cancel=13
SshListen=14 SshAccept=15 SshSharedFrame=16
SshReceive=17 SshSend=18 SshClose=19 SshStatus=20
Stats=21 Configure=22
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
`Configure` accepts a transferred 4 KiB payload frame and a command of at most
512 bytes in word 0. It returns UTF-8 output length in word 0. The broker checks
the kernel-reported IPC peer; shell, Terminal and SSHD are the trusted callers.
Mutations also require the administrator role; status/config inspection remains available to operators.

DNS Resolve uses request word 2 as address family (0/4 for A, 6 for AAAA).
An IPv4 response retains the address in word 0 and reports family 4 in word 2;
IPv6 returns two network-order u64 halves in words 0/1 and family 6 in word 2.
TCP/UDP hostname requests use the `IPV6_ADDRESS` address-word marker and append
16 IPv6 bytes after the original hostname; UDP data follows those bytes.
Authorization continues to use the original hostname. DNS responses validate
ID, question name/type/class and lengths; truncated UDP responses are rejected.
The resolver does not implement DNSSEC or TCP fallback for truncated DNS.

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
API. The generic HTTP API returns the first `Content-Type` and `Location`
values as bounded response-table fields (not an arbitrary header map) and never
follows redirects automatically. The Mica Reader follows at most eight
HTTP(S) redirects using those fields. Its page links remain same-origin; an
explicit address-bar URL or redirect may select another HTTP(S) origin through
`net.browse`. Fragments are stripped before the request. netd remains a bounded
single-owner dataplane: no arbitrary scatter/gather,
generic device drivers are promised. Kernel PCI scanning tracks up to four
modern VirtIO-net devices and publishes MAC/link/epoch/queue readiness. QEMU
device lifecycle qualification is tracked in the maturity ledger. The
SSH protocol is a separate endpoint consumer of netd's listener sockets;
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
