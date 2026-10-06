#!/usr/bin/env python3
# QEMU service recovery and desktop cancellation gate. The disk is an isolated copy.
import json
import pathlib
import os
import re
import hashlib
import shutil
import socket
import subprocess
import time
import http.server
import threading

root = pathlib.Path(__file__).resolve().parent.parent
os.chdir(root)
arch = os.environ.get('ARCH', 'aarch64')
profiles = {'aarch64': ('aarch64-unknown-none-softfloat', 'virt-7.2,virtualization=on,gic-version=3,iommu=smmuv3', 'cortex-a72'),
    'riscv64': ('riscv64gc-unknown-none-elf', 'virt,iommu-sys=on', 'rv64'),
    'x86_64': ('x86_64-unknown-none', 'q35,kernel-irqchip=split', 'qemu64,+x2apic')}
if arch not in profiles: raise SystemExit('unsupported architecture')
target, machine, cpu = profiles[arch]
kernel = root / ('target/' + target + '/release/microsystem-kernel')
source_image = root / os.environ.get('MICROSYSTEM_DISK_PATH', 'build/microsystem.img')
artifact = os.environ.get('MICROSYSTEM_RECOVERY_ARTIFACT', 'service-recovery')
if not re.fullmatch(r'[a-z0-9-]{1,48}', artifact): raise SystemExit('invalid recovery artifact name')
image = root / ('target/' + artifact + '.img')
shutil.copyfile(source_image, image)
log_path = root / ('target/' + artifact + '.log')
socket_path = root / ('target/' + artifact + '-qmp.sock')
log = log_path.open('wb')
socket_path.unlink(missing_ok=True)
arguments = [
    'qemu-system-' + arch, '-machine', machine,
    '-cpu', cpu, '-accel', 'tcg,thread=multi', '-smp', '2', '-m', '256M',
    '-display', 'none', '-vnc', '127.0.0.1:' + os.environ.get('MICROSYSTEM_RECOVERY_VNC_DISPLAY', '9'), '-serial', 'stdio', '-monitor', 'none',
    '-qmp', 'unix:' + str(socket_path) + ',server=on,wait=off', '-L', '/usr/lib/ipxe/qemu',
    '-no-reboot',
    '-drive', 'if=none,file=' + str(image) + ',format=raw,cache=writeback,id=disk0',
    '-netdev', 'user,id=net0,restrict=off', '-object', 'rng-random,filename=/dev/urandom,id=rng0',
]
if arch == 'aarch64': arguments += ['-semihosting-config', 'enable=on,target=native', '-kernel', str(kernel)]
elif arch == 'riscv64': arguments += ['-bios', 'default', '-kernel', str(kernel)]
else:
    arguments += ['-device', 'intel-iommu,intremap=on,caching-mode=on,device-iotlb=on',
        '-device', 'isa-debug-exit,iobase=0xf4,iosize=0x04', '-boot', 'd', '-cdrom', 'build/microsystem-x86_64.iso']
for device, address, options in [
    ('virtio-blk-pci', 2, 'drive=disk0,'), ('virtio-gpu-pci', 3, ''),
    ('virtio-keyboard-pci', 4, ''), ('virtio-tablet-pci', 5, ''),
    ('virtio-net-pci', 6, 'netdev=net0,mac=52:54:00:12:34:56,'),
    ('virtio-rng-pci', 7, 'rng=rng0,'),
]:
    arguments += ['-device', f'{device},{options}disable-legacy=on,iommu_platform=on,romfile=,addr={address}']
identity_only = os.environ.get('MICROSYSTEM_RECOVERY_IDENTITY_ONLY') == '1'
if identity_only:
    arguments[arguments.index('user,id=net0,restrict=off')] = 'user,id=net0,restrict=off,hostfwd=tcp:127.0.0.1:2228-:22'
network_only = os.environ.get('MICROSYSTEM_RECOVERY_NETWORK_ONLY') == '1'
if network_only:
    arguments += ['-netdev', 'user,id=net1,net=10.0.3.0/24,ipv6-net=fec1::/64,restrict=off',
        '-device', 'virtio-net-pci,id=nic1,netdev=net1,mac=52:54:00:12:34:57,disable-legacy=on,iommu_platform=on,romfile=,addr=8']
    arguments += ['-device', 'pcie-root-port,id=net-port,chassis=1,slot=9,addr=9']
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = b'maturity-network-ok'
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *_):
            pass
    class DualServer(http.server.ThreadingHTTPServer):
        address_family = socket.AF_INET6
        def server_bind(self):
            self.socket.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
            super().server_bind()
    fixture = DualServer(('::', 18980), Handler)
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    udp_fixture = socket.socket(socket.AF_INET6, socket.SOCK_DGRAM)
    udp_fixture.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
    udp_fixture.bind(('::', 18981))
    def udp_echo():
        while True:
            data, peer = udp_fixture.recvfrom(4096)
            udp_fixture.sendto(data, peer)
    threading.Thread(target=udp_echo, daemon=True).start()
    dns_fixture = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    dns_fixture.bind(('0.0.0.0', 53))
    def dns_reply():
        while True:
            query, peer = dns_fixture.recvfrom(512)
            cursor = 12
            while cursor < len(query) and query[cursor] != 0: cursor += query[cursor] + 1
            if cursor + 5 > len(query): continue
            kind = int.from_bytes(query[cursor + 1:cursor + 3], 'big')
            answer = bytearray(query[:cursor + 5])
            answer[2:4] = b'\x81\x80'; answer[6:8] = (1 if kind == 28 else 0).to_bytes(2, 'big')
            if kind == 28:
                answer.extend(b'\xc0\x0c\x00\x1c\x00\x01\x00\x00\x00\x1e\x00\x10')
                answer.extend(socket.inet_pton(socket.AF_INET6, 'fec0::2'))
            dns_fixture.sendto(answer, peer)
    threading.Thread(target=dns_reply, daemon=True).start()

print(json.dumps({'phase': 'start', 'kernel_sha256': hashlib.sha256(kernel.read_bytes()).hexdigest(), 'arch': arch}), flush=True)
gate_started = time.monotonic()
guest = subprocess.Popen(arguments, stdin=subprocess.PIPE, stdout=log, stderr=log)

def serial_contents():
    return log_path.read_bytes()

def wait_marker(marker, after=0, timeout=120):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        contents = serial_contents()
        if b'[panic]' in contents or b'DMA fault' in contents:
            raise RuntimeError('guest panic or DMA fault')
        if marker in contents[after:]:
            return contents
        if guest.poll() is not None:
            raise RuntimeError(f'guest exited: {guest.returncode}')
        time.sleep(0.05)
    raise RuntimeError(f'missing marker {marker!r}')

try:
    wait_marker(b'[gui] terminal window ready', timeout=int(os.environ.get('MICROSYSTEM_STARTUP_TIMEOUT', '120')))
    wait_marker(b'[service] shell ready')
    print(json.dumps({'phase': 'desktop-ready', 'elapsed_seconds': round(time.monotonic() - gate_started, 3)}), flush=True)
    client = socket.socket(socket.AF_UNIX)
    client.connect(str(socket_path))
    stream = client.makefile('rwb', buffering=0)
    json.loads(stream.readline())
    sequence = 0

    def qmp(execute, arguments=None):
        global sequence
        sequence += 1
        command = {'execute': execute, 'id': sequence}
        if arguments is not None:
            command['arguments'] = arguments
        stream.write(json.dumps(command).encode() + b'\n')
        while True:
            reply = json.loads(stream.readline())
            if reply.get('id') == sequence:
                if 'error' in reply:
                    raise RuntimeError(reply['error'])
                return reply

    qmp('qmp_capabilities')

    def event(value):
        qmp('input-send-event', {'events': [value]})
        time.sleep(0.1)

    def key(code, down):
        event({'type': 'key', 'data': {'down': down, 'key': {'type': 'qcode', 'data': code}}})

    event({'type': 'abs', 'data': {'axis': 'x', 'value': 3200}})
    event({'type': 'abs', 'data': {'axis': 'y', 'value': 4200}})
    event({'type': 'btn', 'data': {'down': True, 'button': 'left'}})
    event({'type': 'btn', 'data': {'down': False, 'button': 'left'}})
    qmp('screendump', {'filename': str(root / ('target/' + artifact + '-before.png')), 'format': 'png'})

    def terminal(command, marker=None):
        before = len(serial_contents())
        codes = {' ': 'spc', '/': 'slash', '-': 'minus', '.': 'dot', "'": 'apostrophe'}
        for char in command:
            code = codes.get(char, char)
            key(code, True)
            key(code, False)
        key('ret', True)
        key('ret', False)
        qmp('screendump', {'filename': str(root / ('target/' + artifact + '-current.png')), 'format': 'png'})
        if marker:
            wait_marker(marker, before)
        time.sleep(0.3)

    def serial(command, marker, timeout=120):
        before = len(serial_contents())
        guest.stdin.write(command.encode() + b'\n')
        guest.stdin.flush()
        return wait_marker(marker, before, timeout)

    def service_state(name, starts, state='online'):
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            before = len(serial_contents())
            contents = serial('service status ' + name, b'micro> ')
            row = rb'^' + name.encode() + b' ' + state.encode() + b' starts=' + str(starts).encode() + rb'\b'
            if re.search(row, contents[before:], re.MULTILINE):
                return
            time.sleep(0.2)
        raise RuntimeError(f'{name} did not reach {state} in epoch {starts}')

    def network_state(marker):
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            before = len(serial_contents())
            contents = serial('net status', b'micro> ')
            if marker in contents[before:]:
                return
            time.sleep(0.2)
        raise RuntimeError(f'network state did not converge: {marker!r}')

    def desktop_gate():
        wait_marker(b'[gui] unifont runtime loaded=true')
        def click(x, y):
            event({'type': 'abs', 'data': {'axis': 'x', 'value': int(x * 32767 / 1023)}})
            event({'type': 'abs', 'data': {'axis': 'y', 'value': int(y * 32767 / 767)}})
            event({'type': 'btn', 'data': {'down': True, 'button': 'left'}})
            event({'type': 'btn', 'data': {'down': False, 'button': 'left'}})

        def tap(code):
            key(code, True)
            key(code, False)

        def chord(code):
            key('ctrl', True)
            tap(code)
            key('ctrl', False)

        before = len(serial_contents())
        serial('mica --gui --timeout 120s --allow gui.window /.system/examples/mica/desktop-probe.mica', b'[mica] gui presented widgets=true')
        deadline = time.monotonic() + 30
        while serial_contents()[before:].count(b'[mica] gui presented widgets=true') < 2:
            if time.monotonic() >= deadline:
                raise RuntimeError('both application windows did not render')
            time.sleep(0.05)
        click(100, 137)
        chord('a')
        chord('c')
        click(530, 137)
        chord('a')
        chord('v')
        click(100, 137)
        chord('a')
        tap('backspace')
        chord('spc')
        for code in 'nihao':
            tap(code)
        qmp('screendump', {'filename': str(root / 'target/desktop-composition.png'), 'format': 'png'})
        commit = len(serial_contents())
        tap('spc')
        wait_marker(b'[gui] composition committed=true', commit)
        tap('backspace')
        for code in 'hao':
            tap(code)
        tap('spc')
        # A cancelled preedit must never enter the field.
        for code in 'ni':
            tap(code)
        tap('esc')
        chord('spc')
        qmp('screendump', {'filename': str(root / 'target/desktop-two-windows.png'), 'format': 'png'})
        closed = len(serial_contents())
        click(40, 46)
        wait_marker(b'[mica] gui close requested delivered=true', closed)
        # The second window continues to accept focus after the first closes.
        click(530, 137)
        qmp('screendump', {'filename': str(root / 'target/desktop-one-window.png'), 'format': 'png'})
        closed = len(serial_contents())
        click(466, 46)
        wait_marker(b'[mica] gui presented widgets=true', closed)
        click(40, 46)
        contents = wait_marker(b'probe:complete=true', before)
        for prefix, expected in [(b'probe-a:', '你好'.encode()), (b'probe-b:', b'Alpha')]:
            rows = re.findall(re.escape(prefix) + rb'([^\r\n]*)', contents[before:])
            if not rows or rows[-1] != expected:
                raise RuntimeError(f'incorrect edited field {prefix!r}: {rows!r}')
        for marker in [b'probe:window-quota=true', b'probe:clipboard-api=true',
                b'probe:widget-isolation=true', b'probe:clipboard-and-composition=true',
                b'probe:stale-widget-rejected=true']:
            if marker not in contents[before:]:
                raise RuntimeError(f'missing desktop outcome {marker!r}')
        if not re.search(rb'mica: pid=\d+ status=0', contents[before:]):
            wait_marker(b' status=0', before)

    if os.environ.get('MICROSYSTEM_RECOVERY_DESKTOP_ONLY') == '1':
        rounds = int(os.environ.get('MICROSYSTEM_DESKTOP_ROUNDS', '2'))
        if not 1 <= rounds <= 20:
            raise RuntimeError('desktop rounds must be 1..20')
        for _ in range(rounds):
            desktop_gate()
        serial('shutdown', b'[system] shutdown')
        guest.wait(timeout=30)
        print(json.dumps({'multiple_windows': True, 'window_quota_reuse': True,
            'cross_window_widgets_rejected': True, 'clipboard': True,
            'pinyin_composition': True, 'utf8_editing': True, 'rounds': rounds, 'guest_exit': guest.returncode}))
        raise SystemExit(0)

    if identity_only:
        wait_marker(b'[identity] accounts loaded roles=enforced')
        private_key = root / 'build/ssh/id_ed25519'
        private_key.chmod(0o600)
        def ssh_args(user, command):
            return ['ssh', '-F', '/dev/null', '-T', '-o', 'BatchMode=yes', '-o', 'IdentitiesOnly=yes',
                '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null', '-o', 'ConnectTimeout=8',
                '-i', str(private_key), '-p', '2228', user + '@127.0.0.1', command]
        def ssh(user, command, expected=None):
            result = subprocess.run(ssh_args(user, command), capture_output=True, timeout=45)
            if expected is not None and (result.returncode != 0 or expected not in result.stdout):
                raise RuntimeError(f'SSH {user} failed: {result.returncode} {result.stdout!r} {result.stderr!r}')
            return result
        ssh('micro', 'uptime', b'uptime:')
        serial('user add alice operator', b'user: add saved')
        key_hex = (root / 'build/ssh-authorized-key.bin').read_bytes().hex()
        serial('user key-add alice ' + key_hex, b'user: key-add saved')
        ssh('alice', 'whoami', b'alice uid=1001 role=operator')
        if ssh('alice', 'user role alice admin').returncode == 0: raise RuntimeError('operator elevated its own role')
        ssh('alice', """mica -e 'print("identity-alice-ok")'""", b'identity-alice-ok')
        ssh('alice', """mica --allow fs.write:/data -e 'local f=require("fs");local ok,e=f.write_file("/data/alice-owned","owned",{atomic=true,fsync=true});assert(ok~=nil);print("identity-owner-ok")'""", b'identity-owner-ok')
        serial('stat /data/alice-owned', b'uid=1001 gid=1001')
        serial('write /data/private root-secret', b'write: ok')
        serial('chmod 600 /data/private', b'chmod: ok')
        ssh('alice', """mica --allow fs.read:/data -e 'local f=require("fs");local v,e=f.read_file("/data/private");if v==nil and e~=nil then print("identity-private-denied") else error("permission bypass") end'""", b'identity-private-denied')
        serial('user role alice reader', b'user: role saved')
        reader = ssh('alice', """mica -e 'print("reader-must-not-run")'""")
        if reader.returncode == 0 or b'reader-must-not-run' in reader.stdout:
            raise RuntimeError('reader could launch a process')
        ssh('alice', 'uptime', b'uptime:')
        serial('user disable alice', b'user: disable saved')
        if ssh('alice', 'uptime').returncode == 0:
            raise RuntimeError('disabled account authenticated')
        serial('user enable alice', b'user: enable saved')
        import select
        live = subprocess.Popen(ssh_args('alice', '')[:-1], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        ready = b''; deadline = time.monotonic() + 30
        while b'micro> ' not in ready and time.monotonic() < deadline:
            if select.select([live.stdout], [], [], 1)[0]: ready += os.read(live.stdout.fileno(), 4096)
            if live.poll() is not None: break
        if b'micro> ' not in ready: raise RuntimeError('live SSH shell did not authenticate')
        serial('user key-remove alice 0', b'user: key-remove saved')
        if live.wait(timeout=15) == 0: raise RuntimeError('revoked live SSH session did not terminate')
        if ssh('alice', 'uptime').returncode == 0:
            raise RuntimeError('revoked key authenticated')
        serial('user key-add alice ' + key_hex, b'user: key-add saved')
        serial('user role alice operator', b'user: role saved')
        ssh('alice', """mica -e 'print("identity-key-rotation-ok")'""", b'identity-key-rotation-ok')
        serial('user host-key rotate', b'SSH host key rotated')
        time.sleep(2)
        ssh('alice', 'uptime', b'uptime:')
        serial('service restart mfs', b'service: mfs accepted')
        time.sleep(2)
        service_state('mfs', 2)
        ssh('alice', 'uptime', b'uptime:')
        serial('user list', b'alice uid=1001')
        serial('user audit', b'action=key-remove subject=alice')
        serial('shutdown', b'[system] shutdown')
        guest.wait(timeout=30)
        stream.close(); client.close(); socket_path.unlink(missing_ok=True)
        guest = subprocess.Popen(arguments, stdin=subprocess.PIPE, stdout=log, stderr=log)
        before = len(serial_contents())
        wait_marker(b'[identity] accounts loaded roles=enforced', before)
        wait_marker(b'[service] shell ready', before)
        ssh('alice', 'uptime', b'uptime:')
        serial('user list', b'alice uid=1001')
        serial('user audit', b'action=host-key-rotated subject=sshd')
        for _ in range(36): ssh('alice', 'uptime', b'uptime:')
        serial('shutdown', b'[system] shutdown')
        guest.wait(timeout=30)
        print(json.dumps({'accounts_persisted': True, 'roles_enforced': True, 'private_file_denied': True,
            'disabled_account_denied': True, 'key_revocation': True, 'live_session_revoked': True, 'host_key_rotation': True,
            'filesystem_recovery': True, 'audit_persisted': True, 'cold_boots': 2, 'ssh_reconnect_rounds': 36, 'guest_exit': guest.returncode}))
        raise SystemExit(0)

    if os.environ.get('MICROSYSTEM_RECOVERY_SOAK_ONLY') == '1':
        rounds = int(os.environ.get('MICROSYSTEM_SOAK_ROUNDS', '12'))
        if not 3 <= rounds <= 240: raise RuntimeError('soak rounds must be 3..240')
        period = float(os.environ.get('MICROSYSTEM_SOAK_PERIOD', '0'))
        if not 0 <= period <= 60: raise RuntimeError('soak period must be 0..60 seconds')
        long_echo = 'x' * 512
        serial('echo ' + long_echo, b'\n' + long_echo.encode() + b'\r\n')
        samples = []
        for iteration in range(rounds):
            start = time.monotonic()
            serial('mica --allow fs.write:/data -e \'local f=require("fs"); local ok,e=f.write_file("/data/soak","cycle",{atomic=true,fsync=true}); assert(ok~=nil); print("soak-file-ok")\'', b'\nsoak-file-ok\r\n')
            before = len(serial_contents())
            serial('app exec /.system/examples/native/counter.elf', b'immutable-copy=true')
            row = re.search(rb'ELF loaded pid=(\d+)', serial_contents()[before:])
            if not row: raise RuntimeError('soak native process was not launched')
            serial('wait ' + row.group(1).decode(), b'status=0')
            before = len(serial_contents())
            serial('service restart netd', b'service: netd accepted')
            service_state('netd', iteration + 2)
            wait_marker(b'[net] netd ready ipv4=10.0.2.15', before)
            before = len(serial_contents())
            serial('sysinfo', b'micro> ')
            match = re.search(rb'free_frames=(\d+)', serial_contents()[before:])
            if not match: raise RuntimeError('soak did not report physical memory')
            samples.append({'round': iteration + 1, 'seconds': round(time.monotonic() - start, 3), 'free_frames': int(match.group(1))})
            print(json.dumps({'phase': 'soak-round', **samples[-1]}), flush=True)
            time.sleep(max(0, period - (time.monotonic() - start)))
        stable = [row['free_frames'] for row in samples[2:]]
        if max(stable) - min(stable) > 64: raise RuntimeError('physical memory drift exceeds 64 pages after warmup')
        serial('cat /data/soak', b'cycle')
        serial('shutdown', b'[system] shutdown')
        guest.wait(timeout=30)
        print(json.dumps({'soak_rounds': rounds, 'file_native_network_lifecycles': True, 'memory_drift_pages': max(stable) - min(stable), 'samples': samples, 'guest_exit': guest.returncode}))
        raise SystemExit(0)

    if os.environ.get('MICROSYSTEM_RECOVERY_VFS_ONLY') == '1':
        for directory in ['/volumes', '/mnt/a', '/mnt/ab']:
            serial('mkdir -p ' + directory, b'mkdir: ok')
        for name in ['a', 'ab']:
            serial(f'volume create /volumes/{name}.img 3', b'volume: ok')
            serial(f'mount /volumes/{name}.img /mnt/{name}', b'volume: ok')
        serial('write /mnt/a/note one', b'write: ok')
        serial('chmod 640 /mnt/a/note', b'chmod: ok')
        serial('chown 1234:56 /mnt/a/note', b'chown: ok')
        serial('append /mnt/a/note -two', b'append: ok')
        serial('fsync /mnt/a/note', b'fsync: ok')
        serial('write /mnt/ab/note other', b'write: ok')
        serial('fsync /mnt/ab/note', b'fsync: ok')
        serial('mount', b'/mnt/ab /volumes/ab.img rw')
        serial('df /mnt/a', b'filesystem blocks=768')
        serial('mv /mnt/a/note /mnt/ab/moved', b'mv: failed')
        serial('write /volumes/a.img corrupt', b'write: failed')
        serial('cat /mnt/a/note', b'one-two')
        before = len(serial_contents())
        serial('service restart mfs', b'service: mfs accepted')
        wait_marker(b'[gui] windowd ready', before)
        service_state('mfs', 2)
        serial('cat /mnt/a/note', b'one-two')
        serial('cat /mnt/ab/note', b'other')
        serial('stat /mnt/a/note', b'mode=640 uid=1234 gid=56 modified=')
        serial('umount /mnt/a', b'volume: ok')
        serial('mount /volumes/a.img /mnt/a ro', b'volume: ok')
        serial('write /mnt/a/note denied', b'write: failed')
        serial('append /mnt/a/note denied', b'append: failed')
        serial('chmod 777 /mnt/a/note', b'chmod: failed')
        serial('cat /mnt/a/note', b'one-two')
        before = len(serial_contents())
        serial('service restart mfs', b'service: mfs accepted')
        wait_marker(b'[gui] windowd ready', before)
        service_state('mfs', 3)
        serial('mount', b'/mnt/a /volumes/a.img ro')
        serial('cat /mnt/a/note', b'one-two')
        serial('shutdown', b'[system] shutdown')
        guest.wait(timeout=30)
        print(json.dumps({'multiple_volumes': True, 'prefix_isolation': True,
            'cross_volume_rename_rejected': True, 'backing_image_protected': True,
            'metadata_persisted': True, 'readonly_persisted': True, 'restart_rounds': 2,
            'guest_exit': guest.returncode}))
        raise SystemExit(0)

    if network_only:
        wait_marker(b'[net] netd ready ipv4=10.0.2.15')
        wait_marker(b'[net] netd ready ipv4=10.0.3.15')
        serial('net status', b'interface=1 mac=52:54:00:12:34:57 link=true')
        serial('curl http://10.0.2.2:18980/', b'maturity-network-ok')
        serial('curl http://10.0.3.2:18980/', b'maturity-network-ok')
        serial('mica --allow net.connect:10.0.2.2:18980 -e \'local n=require("net"); local a=n.tcp_connect("10.0.2.2",18980); n.tcp_close(a); local c=n.tcp_connect("10.0.2.2",18980); assert(c~=a); local d,e=n.tcp_read(a,1); assert(d==nil and e~=nil); n.tcp_close(c); print("stale-network-handle-rejected")\'', b'\nstale-network-handle-rejected\r\n')
        # QEMU's IPv6 router advertisements must configure a non-link-local address.
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            before = len(serial_contents())
            contents = serial('net status', b'micro> ')
            if b'fec0:' in contents[before:]:
                break
            time.sleep(1)
        else:
            raise RuntimeError('IPv6 SLAAC did not acquire QEMU prefix')
        serial('mica --allow net.connect:fec0::2:18980 -e \'local n=require("net"); local b=require("bytes"); local c,e=n.tcp_connect("fec0::2",18980); assert(c~=nil); n.tcp_write(c,"GET / HTTP/1.0\\r\\n\\r\\n"); local r=""; for i=1,8 do local d=n.tcp_read(c,512); if d==nil or b.len(d)==0 then break end; r=r+b.to_string(d) end; print(r); n.tcp_close(c)\'', b'maturity-network-ok')
        serial('mica --allow net.connect:fec1::2:18981 -e \'local n=require("net"); local b=require("bytes"); local u=n.udp_open(0,1); assert(u~=nil); n.udp_send_to(u,"fec1::2",18981,"ipv6-udp-ok"); local r,e=n.udp_recv_from(u,64); assert(r~=nil); assert(r.address=="fec1::2"); print(b.to_string(r.data)); n.udp_close(u)\'', b'\nipv6-udp-ok\r\n')
        serial('net static 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 10.0.2.3 fec0::20/64 fe80::2', b'network configuration saved')
        wait_marker(b'[net] netd ready ipv4=10.0.2.20')
        serial('curl http://10.0.2.2:18980/', b'maturity-network-ok')
        serial('net static 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 10.0.2.2 fec0::20/64 fe80::2', b'network configuration saved')
        serial('mica --allow net.connect:ipv6.maturity.test:18980 -e \'local n=require("net"); local a=n.resolve("ipv6.maturity.test","ipv6"); assert(a=="fec0::2"); print("ipv6-dns-ok")\'', b'\nipv6-dns-ok\r\n')
        serial('curl http://ipv6.maturity.test:18980/', b'maturity-network-ok')
        serial('net default 52:54:00:12:34:57', b'network configuration saved')
        serial('netstat', b'ipv4=10.0.3.15')
        qmp('set_link', {'name': 'nic1', 'up': False})
        network_state(b'interface=1 mac=52:54:00:12:34:57 link=false')
        serial('netstat', b'ipv4=10.0.2.20')
        qmp('set_link', {'name': 'nic1', 'up': True})
        network_state(b'interface=1 mac=52:54:00:12:34:57 link=true')
        before = len(serial_contents())
        serial('service restart netd', b'service: netd accepted')
        service_state('netd', 2)
        wait_marker(b'[net] netd ready ipv4=10.0.2.20', before)
        serial('net config', b'iface 52:54:00:12:34:56 10.0.2.20/24')
        serial('curl http://10.0.3.2:18980/', b'maturity-network-ok')
        serial('net static 52:54:00:12:34:56 10.0.2.20/24 10.0.9.2 -', b'net: Invalid')
        serial('net dhcp 52:54:00:12:34:56', b'network configuration saved')
        wait_marker(b'[net] netd ready ipv4=10.0.2.15', before)
        qmp('netdev_add', {'type': 'user', 'id': 'net2', 'net': '10.0.4.0/24', 'ipv6-prefix': 'fec2::', 'ipv6-prefixlen': 64})
        qmp('device_add', {'driver': 'virtio-net-pci', 'id': 'nic2', 'netdev': 'net2',
            'bus': 'net-port', 'addr': '0', 'mac': '52:54:00:12:34:58',
            'disable-legacy': 'on', 'iommu_platform': True, 'romfile': ''})
        time.sleep(2)
        wait_marker(b'[net] netd ready ipv4=10.0.4.15')
        serial('net status', b'interface=2 mac=52:54:00:12:34:58 link=true')
        serial('curl http://10.0.4.2:18980/', b'maturity-network-ok')
        before = len(serial_contents())
        qmp('device_del', {'id': 'nic2'})
        wait_marker(b'[net] interface removed/reset index=2', before, 30)
        for _ in range(2):
            before = len(serial_contents())
            qmp('device_add', {'driver': 'virtio-net-pci', 'id': 'nic2', 'netdev': 'net2',
                'bus': 'net-port', 'addr': '0', 'mac': '52:54:00:12:34:58',
                'disable-legacy': 'on', 'iommu_platform': True, 'romfile': ''})
            wait_marker(b'[net] netd ready ipv4=10.0.4.15', before)
            serial('curl http://10.0.4.2:18980/', b'maturity-network-ok')
            before = len(serial_contents())
            qmp('device_del', {'id': 'nic2'})
            wait_marker(b'[net] interface removed/reset index=2', before, 30)
        serial('shutdown', b'[system] shutdown')
        guest.wait(timeout=30)
        print(json.dumps({'dhcp_interfaces': 2, 'ipv6_slaac': True, 'ipv6_tcp': True, 'ipv6_udp': True, 'ipv6_dns': True, 'stale_handles_rejected': True,
            'static_config_persisted': True, 'multiple_routes': True, 'link_failover': True,
            'invalid_gateway_rejected': True, 'device_hotplug': True, 'device_remove': True, 'hotplug_rounds': 3, 'guest_exit': guest.returncode}))
        raise SystemExit(0)

    service_state('terminal', 1)
    # Native code must be loaded from a versioned filesystem image.
    serial('app install sample one /.system/examples/native/counter.elf 8 stats', b'app: installed sample version=one')
    serial('app run sample', b'immutable-copy=true')
    serial('app update sample two /.system/examples/native/counter.elf 16 all', b'app: installed sample version=two')
    serial('app info sample', b'sample active=two previous=one')
    serial('append /.system/apps/sample/two.elf tamper', b'append: ok')
    serial('fsync /.system/apps/sample/two.elf', b'fsync: ok')
    serial('app run sample', b'app: failed status=Corrupt')
    serial('app rollback sample', b'app: rolled back sample version=one')
    serial('app run sample', b'immutable-copy=true')
    serial('app install sample one /.system/examples/native/counter.elf', b'app: failed status=Busy')
    before_pressure = len(serial_contents())
    pressure_pids = []
    for _ in range(16):
        before = len(serial_contents())
        contents = serial('app exec /.system/examples/native/memoryprobe.elf', b'immutable-copy=true')
        row = re.search(rb'filesystem ELF loaded pid=(\d+) immutable-copy=true', contents[before:])
        if row is None:
            raise RuntimeError('missing native pressure PID')
        pressure_pids.append(int(row.group(1)))
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        contents = serial_contents()[before_pressure:]
        allocated_rows = re.findall(rb'allocated pid=(\d+) resident=4096', contents)
        denied_rows = re.findall(rb'NoMemory pid=(\d+) resident=0 atomic=true', contents)
        if len(allocated_rows) + len(denied_rows) == 16:
            break
        time.sleep(0.05)
    else:
        raise RuntimeError('not all pressure processes reported an allocation outcome')
    allocated = set(map(int, allocated_rows))
    denied = set(map(int, denied_rows))
    if not allocated or not denied or not allocated.issubset(set(pressure_pids)):
        raise RuntimeError('physical pressure outcomes are missing')
    for pid in allocated:
        serial(f'kill {pid}', b'kill:')
        serial(f'wait {pid}', b'wait:')
    before = len(serial_contents())
    serial('app exec /.system/examples/native/memoryprobe.elf', b'immutable-copy=true')
    contents = wait_marker(b' resident=4096', before)
    row = re.search(rb'allocated pid=(\d+) resident=4096', contents[before:])
    if row is None:
        raise RuntimeError('large memory allocation was not reusable')
    restored_pid = int(row.group(1))
    serial(f'kill {restored_pid}', b'kill:')
    serial(f'wait {restored_pid}', b'wait:')
    serial('app run sample', b'immutable-copy=true')


    serial('mkdir -p /recovery-proof', b'micro> ')
    serial('write /recovery-proof/note survives-recovery', b'micro> ')
    serial('chmod 600 /recovery-proof/note', b'chmod: ok')
    serial('chown 1000:1000 /recovery-proof/note', b'chown: ok')
    serial('fsync /recovery-proof/note', b'micro> ')
    serial('stat /recovery-proof/note', b'mode=600 uid=1000 gid=1000 modified=')
    before = len(serial_contents())
    terminal('sleep 20s')
    if b'[gui] terminal command completed asynchronous=true' in serial_contents()[before:]:
        raise RuntimeError('long sleep completed before responsiveness check')
    # Move Terminal by its uncovered title bar while its worker sleeps.
    event({'type': 'abs', 'data': {'axis': 'x', 'value': 5500}})
    event({'type': 'abs', 'data': {'axis': 'y', 'value': 2800}})
    event({'type': 'btn', 'data': {'down': True, 'button': 'left'}})
    event({'type': 'abs', 'data': {'axis': 'x', 'value': 6500}})
    event({'type': 'abs', 'data': {'axis': 'y', 'value': 3600}})
    event({'type': 'btn', 'data': {'down': False, 'button': 'left'}})
    qmp('screendump', {'filename': str(root / 'target/service-recovery-sleep-drag.png'), 'format': 'png'})
    key('ctrl', True)
    key('c', True)
    key('c', False)
    key('ctrl', False)
    wait_marker(b'[gui] terminal command completed asynchronous=true', before, 10)
    terminal('uptime', b'[gui] terminal command executed name=uptime')
    qmp('screendump', {'filename': str(root / 'target/service-recovery-cancel.png'), 'format': 'png'})
    before = len(serial_contents())
    serial('service restart terminal', b'service: terminal accepted')
    wait_marker(b'[gui] fixed client reconnected client=1', before)
    service_state('terminal', 2)
    terminal('uptime', b'[gui] terminal command executed name=uptime')
    serial('service stop db', b'service: db accepted')
    serial('service status db', b'db stopped starts=1 exit=-15 held=1')
    serial('sql SELECT * FROM missing', b'micro> ')
    serial('service restart db', b'service: db accepted')
    serial('sql SELECT * FROM missing', b'micro> ')
    service_state('db', 2)
    before = len(serial_contents())
    serial('service restart netd', b'service: netd accepted')
    wait_marker(b'[net] netd ready', before)
    service_state('netd', 2)
    before = len(serial_contents())
    serial('service restart mfs', b'service: mfs accepted')
    wait_marker(b'[ipc] resident mfs endpoint=3 ready', before)
    wait_marker(b'[gui] windowd ready', before)
    serial('cat /recovery-proof/note', b'survives-recovery')
    service_state('mfs', 2)
    serial('stat /recovery-proof/note', b'mode=600 uid=1000 gid=1000 modified=')
    before = len(serial_contents())
    serial('service restart block', b'service: block accepted')
    wait_marker(b'[ipc] resident block endpoint=2 ready', before)
    wait_marker(b'[ipc] resident mfs endpoint=3 ready', before)
    wait_marker(b'[gui] windowd ready', before)
    serial('cat /recovery-proof/note', b'survives-recovery')
    service_state('block', 2)
    before = len(serial_contents())
    serial('service restart devmgr', b'service: devmgr accepted')
    wait_marker(b'[ipc] resident block endpoint=2 ready', before)
    wait_marker(b'[gui] windowd ready', before)
    service_state('devmgr', 2)
    serial('cat /recovery-proof/note', b'survives-recovery')
    serial('service list', b'netd online starts=5')
    serial('app info sample', b'sample active=one previous=two')
    serial('app run sample', b'immutable-copy=true')
    # The shared cancellation notification must survive a complete GUI restart.
    event({'type': 'abs', 'data': {'axis': 'x', 'value': 3200}})
    event({'type': 'abs', 'data': {'axis': 'y', 'value': 4200}})
    event({'type': 'btn', 'data': {'down': True, 'button': 'left'}})
    event({'type': 'btn', 'data': {'down': False, 'button': 'left'}})
    before = len(serial_contents())
    terminal('sleep 20s')
    key('ctrl', True)
    key('c', True)
    key('c', False)
    key('ctrl', False)
    wait_marker(b'[gui] terminal command completed asynchronous=true', before, 10)
    terminal('uptime', b'[gui] terminal command executed name=uptime')
    qmp('screendump', {'filename': str(root / ('target/' + artifact + '-restored.png')), 'format': 'png'})
    guest.stdin.write(b'shutdown\n')
    guest.stdin.flush()
    guest.wait(timeout=30)
    print(json.dumps({'physical_oom_atomic': True, 'memory_reusable_after_pressure': True, 'native_install_update_rollback': True, 'native_corruption_rejected': True, 'async_cancel': True, 'terminal_reconnect': True, 'db_hold_resume': True,
        'network_restart': True, 'storage_dependency_recovery': True, 'guest_exit': guest.returncode}))

finally:
    if guest.poll() is None:
        guest.terminate()
        try:
            guest.wait(timeout=5)
        except subprocess.TimeoutExpired:
            guest.kill()
    log.close()
