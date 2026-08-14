# Test packet results

验证时间：2026-08-09（OrbStack Docker context）

## 环境

```text
$ docker context show
orbstack
$ docker info --format '{{.OperatingSystem}} | {{.ServerVersion}}'
OrbStack | 29.4.0
```

镜像 `microsystem-dev:rust-1.97.1` 使用 Rust 1.97.1、QEMU 7.2，并包含
`qemu-efi-aarch64` 与 `ipxe-qemu` 的 VirtIO option ROM。容器命令显式设置
`RUSTUP_TOOLCHAIN=1.97.1`，避免容器内 rustup 因证书环境重复同步工具链。

## 结果

### Host/logic tests

```text
docker run --rm -e RUSTUP_TOOLCHAIN=1.97.1 \
  -v "$PWD:/workspace" -w /workspace \
  rust:1.97.1-bookworm \
  cargo test -p microsystem-kernel -p microsystem-abi -p mfs1
```

OrbStack 容器内通过：

- `mfs1` recovery：6/6（fsync 与未同步写入、双超级块回退、CRC 破坏、失败事务、GC arena 切换、路径/目录边界）。
- `microsystem-abi` capability ABI：6/6（slot/generation、rights 交集、消息默认值和编号稳定性）。
- `microsystem-kernel` core boundaries：8/8（capability generation/rights/revoke、帧保留与释放、W^X/alignment、同页 PT_LOAD W^X 拒绝、调度抢占/唤醒、IPC 队列容量/FIFO）。
- 所有 doc-tests：通过（0 failures）。

### 公共入口验收

```text
make test  # exit 0
make fsck  # exit 0
```

`make test` 完成 host AArch64 ELF 构建、容器逻辑测试和 QEMU smoke；
`make fsck` 输出：

```text
MFS1 clean generation=1 transactions=0 entries=1 used_blocks=0
```

### 五个独立静态服务 ELF 回归（2026-08-09）

bootfs 现由五个独立构建的 AArch64 静态 ELF 组成；smoke 进一步要求
`console`、`block`、`mfs`、`shell` 各自打印 EL0 入口并得到对应内核执行确认。
OrbStack 最终入口均通过：

```text
make test  # exit 0
make fsck  # exit 0
```

QEMU 关键串口：

```text
[bootfs] valid=true entries=6 static-elfs=5
[user] bootfs init ELF entered EL0
[bootfs] init ELF executed at EL0
[user] console service ELF entered EL0
[bootfs] console ELF executed at EL0
[user] block service ELF entered EL0
[bootfs] block ELF executed at EL0
[user] mfs service ELF entered EL0
[bootfs] mfs ELF executed at EL0
[user] shell service ELF entered EL0
[bootfs] shell ELF executed at EL0
[sched] cpu-bound complete counters=[80000000,80000000] timer-preemptions=[10,9]
PID 2 shell running cpu=0 ticks=[329,324]
uptime: 2470 ms
[system] shutdown
```

smoke 摘要：`PASS`，CPU0/CPU1 timer ticks=329/324，uptime=2470 ms。离线
`fsck` 输出 `MFS1 clean generation=1 transactions=0 entries=1 used_blocks=0`。

### ELF 页粒度 W^X 回归（2026-08-09）

在 `crates/kernel/tests/core_boundaries.rs` 增加了聚焦测试
`elf_loader_rejects_wx_segments_sharing_one_page`：构造两个字节范围不重叠、
但同落在 `0x0040_0000` 页内的 PT_LOAD（RX 与 RW），确认
`ElfImage::parse` 返回 `Status::Invalid`，防止后续页映射覆盖前一段权限。

OrbStack `make test` 通过（exit 0）：kernel 边界测试 8/8（新增 W^X 回归通过），
mfs1 6/6、ABI 6/6、QEMU smoke 也通过；本次纯 ELF 改动按要求未重复执行
`make fsck`。

```text
smoke-qemu: PASS (serial milestones present; make run status=0)
smoke-qemu: timers cpu0=344 cpu1=340 uptime_ms=2540
smoke-qemu: scheduler counters=[80000000,80000000] timer-preemptions=[12,10]
```

### 单核 EL0 round-robin context switch（2026-08-09）

Luna MAX 首次 OrbStack smoke 定位到真实上下文恢复缺陷：双核负载通过后，
RR 尚未输出即发生 EL1 data abort（`ESR=0x96000006 FAR=0x228`），原因是
返回内核时未恢复 x19 等 AAPCS callee-saved 寄存器。生产侧补齐 EL0 enter/return
的 x19–x30 保存恢复后，重跑 smoke 与 `make test` 均通过：

```text
[sched] round-robin context-switches=20 progress=[25047371,26077121]
smoke-qemu: PASS (serial milestones present; make run status=0)
smoke-qemu: timers cpu0=330 cpu1=326 uptime_ms=2480
smoke-qemu: scheduler counters=[80000000,80000000] timer-preemptions=[9,9]
smoke-qemu: round-robin context-switches=20 progress=[25047371,26077121]
```

本轮 `make test` exit 0；kernel 边界 8/8、mfs1 6/6、ABI 6/6，且无 panic 或
timeout。纯 QEMU/调度改动未重复执行 `make fsck`。

### RR 独立地址空间与 ASID 回归（2026-08-09）

Luna MAX 在新增 MMU/ASID 版本上先执行未加新断言的现有 OrbStack `make test`，
结果 exit 0；QEMU 串口确认两个独立地址空间和 ASID：

```text
[sched] round-robin context-switches=20 progress=[24380816,26652448]
[mmu] round-robin address-spaces=2 asids=[1,2]
```

同次 smoke 还通过双核 80M 负载（timer-preemptions=`[13,12]`）、EL0 fault
隔离、五个 bootfs 服务和 shutdown（ticks=`[325,321]`，uptime=2445 ms）。
在该通过证据基础上，现有 `scripts/smoke-qemu.sh` 已增加精确 marker 断言
`address-spaces=2 asids=[1,2]`；本轮不重复启动 QEMU。

### 同步 IPC call/recv/reply 回归（2026-08-09）

Luna MAX 在修正 SVC 返回路径及 IPC_ACTIVE 处理后，单次 OrbStack smoke 通过，
并观察到精确协议结果：

```text
[ipc] call/recv/reply endpoint=1 request=0x1234 reply=0x2468 asids=[3,4]
```

现有 smoke 已增加该 marker 断言。随后执行 `make test` 时，host 逻辑测试、
IPC/RR、bootfs 和 shutdown 均完成，但 smoke 因既有 `timer=true` 串口 marker
被两核 UART 并发交错破坏而返回失败（非 IPC 失败）：

```text
[boot] cpu1 onli[smp] cpu1-online=ne el1 high-half timer=trutrue
smoke-qemu: missing serial milestones: timer=true
```

本次不将该 `make test` 运行记为全绿；待 UART 输出原子化或 marker 策略修正后
再作最终门禁确认。

UART ticket lock 修正后，Luna MAX 重新执行含 IPC marker 的完整 OrbStack
`make test`，最终 exit 0：

```text
[sched] cpu-bound complete counters=[80000000,80000000] timer-preemptions=[9,9]
[sched] round-robin context-switches=20 progress=[23630615,25729494]
[mmu] round-robin address-spaces=2 asids=[1,2]
[ipc] call/recv/reply endpoint=1 request=0x1234 reply=0x2468 asids=[3,4]
PID 2 shell running cpu=0 ticks=[326,323]
uptime: 2460 ms
[system] shutdown
```

五个 bootfs ELF、EL0 fault 隔离、Shell ready 均通过；无 panic 或 timeout。

### 统一验证点（generation/revoke、VirtIO+SMMU、同扇区写回，2026-08-09）

在现有 `core_boundaries.rs` 增加了 generation-reuse 回归：旧父 cap 删除并复用
同 slot 后，撤销新 generation 不得触及旧 generation 的后代。OrbStack 完整
入口均通过：

```text
make test  # exit 0
make fsck  # exit 0
```

计数：kernel core boundaries 9/9（含新增回归）、mfs1 6/6、ABI 6/6，doc-tests
0；smoke 无 panic、timeout 或 translation fault。稳定串口断言均命中：

```text
[virtio] modern features=VERSION_1+ACCESS_PLATFORM+FLUSH ...
[iommu] SMMUv3 domain stream-id=0x10 iova=0x100000 idr0=0xd40101a idr5=0x74 gerror=0x0
[virtio] queue0 read+write sector=0 status=ok mfs1=true flush=ok
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x402a6000 completion-error=false
```

同次 RR progress=`[23354180,26987085]`、IPC request/reply=`0x1234/0x2468`，
Shell ticks=`[330,326]`、uptime=`2475 ms`，最终正常 shutdown。随后 fsck 输出：

```text
MFS1 clean generation=1 transactions=0 entries=1 used_blocks=0
```

### VirtIO INTx 单次 smoke（2026-08-09）

独立 OrbStack smoke 已确认 DTB interrupt-map 到 GIC SPI 的绑定和完成计数：

```text
[irq] virtio-blk INTx pin=1 gic-id=37 bound cpu0
[irq] virtio-blk INTx completions=4
```

同次 queue0 read/write/flush 与 SMMU fault probe 均通过，Shell 正常 shutdown，
未观察 IRQ storm、translation error、panic 或 timeout。

现有 `scripts/smoke-qemu.sh` 已加入稳定门禁：必须看到
`pin=1 gic-id=37 bound cpu0`，并解析 `completions=N` 要求 `N>0`。使用既有
`target/intx-smoke-qemu.log` 做匹配检查得到 `completions=4`，同时 `bash -n`
通过；本次未重复启动 QEMU。

`MICROSYSTEM_HOST_TEST_LOG=target/test-host-final.log scripts/test-host.sh`
同样通过（wrapper exit 0）。

### QEMU smoke

```text
docker run --rm -e RUSTUP_TOOLCHAIN=1.97.1 \
  -v "$PWD:/workspace" -w /workspace \
  microsystem-dev:rust-1.97.1 scripts/smoke-qemu.sh
```

退出码 0。串口门禁依次观察到 CPU0、MMU/TTBR、DTB `cpus=2`、PCIe/SMMUv3、
EL0 init SVC、`PSCI CPU_ON result=0`、CPU1 online、root task、block、MFS1 和
shell ready；随后 `help`、`ps`、`uptime`、`sync`、`shutdown` 均得到响应，QEMU
正常退出。完整日志保存在 `target/smoke-qemu.log`（脚本在容器中输出对应挂载路径）。

## 未覆盖边界

- 单个非法用户指针触发的 data-abort probe 已通过并确认任务终止、内核继续运行；
  尚未覆盖通用 procman 回收，以及页故障/特权指令的完整隔离矩阵。
- SMMUv3 一页 queue IOVA、单扇区 read/write/flush 和越界 event/sentinel probe
  已通过；当前剩余边界是 EL1 启动探针而非常驻 EL0 block service，以及尚未覆盖
  通用多-frame DMA、INTx capability 传递和 IRQ ack 协议。
- 未执行真实硬件、MSI-X/ITS、网络、动态链接、浮点上下文或性能基准。

这些项目是当前源码明确的功能边界，不作为已通过的测试声称。

## 最新 GICv3/EL0 probe 回归（2026-08-09）

增强后的 `scripts/smoke-qemu.sh` 还要求 `ps` 的两个 CPU tick 和 `uptime` 均
大于零，并保留启动期 IRQ 自检；脚本默认在启动和命令之间留 1 秒，避免在
guest 尚未启用两个 PPI 时把整段输入一次性灌入。当前一次 OrbStack 回归未通过：

```text
[irq] self-test ticks=0 cntp_ctl=0x1 gicd_ctl=0x53 pending=0x0 hppir=1023
[isolation] launching EL0 invalid-pointer/privilege probe
qemu-system-aarch64: terminating on signal 15 from pid 11 (timeout)
```

此次运行没有观察到 `[fault]`、`faulted task terminated` 或 `shell ready`；
故障 probe 在进入可观测 fault 前卡住。该结果保留为当前回归证据，不能将
增强 smoke 声称为通过。

### 最新增强 smoke（PPI27 + EL0 probe，2026-08-09）

父任务将隔离 probe 改为先进入 EL0，再执行非法 DebugWrite 和未映射地址
读取；随后在 OrbStack 中重新执行同一脚本，结果通过（exit 0）：

```text
[irq] self-test ticks=4 cntv_ctl=0x1 gicd_ctl=0x53 pending=0x0 hppir=1023
[user] isolation probe entered EL0
[fault] user exception ESR=0x92000005 FAR=0xfffffffffffffff8; task terminated
[isolation] faulted task terminated; kernel survived
[boot] cpu1 online el1 high-half [smp] cpu1-online=ttimer=true
PID 2 shell running cpu=0 ticks=[181,177]
uptime: 1770 ms
[system] shutdown
```

脚本摘要：`smoke-qemu: PASS`，CPU0 ticks=181、CPU1 ticks=177、uptime=1770
ms；probe 触发 data abort 后内核继续完成 CPU1、shell 和 shutdown 门禁。

### PCI/双核无-yield 调度门禁（2026-08-09）

`scripts/smoke-qemu.sh` 新增三项行为断言：DTB ECAM 必须发现 VirtIO block
`vendor=1af4 device=1042`；两个 EL0 CPU-bound 任务必须各完成 8,000,000 次；
两个 CPU 的 `timer-preemptions` 必须均大于零。OrbStack `make test` 的 host
逻辑测试仍全部通过（mfs1 6/6、ABI 6/6、kernel 7/7），但最新 QEMU 门禁因
CPU1 没有观测到 timer preemption 失败：

```text
[device] virtio-blk-pci 00:02.0 vendor=1af4 device=1042
[sched] cpu-bound complete counters=[8000000,8000000] timer-preemptions=[1,0]
```

CPU0 的计数为 1，CPU1 为 0；因此未声称双核抢占门禁通过。完整串口在
`target/smoke-qemu.log`，`make test` exit 2（`xtask test` 的 smoke 失败）。

独立执行 `make fsck` 通过（exit 0）：

```text
MFS1 clean generation=1 transactions=0 entries=1 used_blocks=0
```

### Bootfs/静态 ELF 与 80M 双核负载（2026-08-09）

smoke 门禁已提升为 `counters=[80000000,80000000]`，并要求 bootfs 汇总为
`valid=true entries=6 static-elfs=5`、静态 init ELF 的 EL0 进入/完成标记，以及
两核 timer preemption 均大于零。OrbStack `make test` 的 host 逻辑测试仍全部
通过（mfs1 6/6、ABI 6/6、kernel 7/7），QEMU 串口已通过 bootfs 汇总、PCI、
80M counters 和抢占：

```text
[bootfs] valid=true entries=6 static-elfs=5
[device] virtio-blk-pci 00:02.0 vendor=1af4 device=1042
[sched] cpu-bound complete counters=[80000000,80000000] timer-preemptions=[9,9]
```

但静态 init ELF 尚未真正完成 EL0 入口：其首个用户栈访问触发
`[fault] user exception ESR=0x92000047 FAR=0x5feff0`，因此没有观察到
`[user] bootfs init ELF entered EL0`；随后内核仍打印了
`[bootfs] init ELF executed at EL0`，这是当前生产侧成功返回而非用户程序成功
执行的误报，smoke 因缺少入口标记失败（`make test` exit 2）。

独立 `make fsck` 通过（exit 0）：

```text
MFS1 clean generation=1 transactions=0 entries=1 used_blocks=0
```

### 最终 OrbStack 回归（SP_EL0 修正后，2026-08-09）

生产侧将所有 EL0 入口的 `SP_EL0` 调整到已映射栈页顶后，重新执行完整入口：

```text
make test  # exit 0
make fsck  # exit 0
```

QEMU smoke 通过全部 bootfs、PCI、EL0 隔离、双核调度和 Shell 门禁：

```text
[bootfs] valid=true entries=6 static-elfs=5
[device] virtio-blk-pci 00:02.0 vendor=1af4 device=1042
[fault] user exception ESR=0x92000005 FAR=0xfffffffffffffff8; task terminated
[isolation] faulted task terminated; kernel survived
[sched] cpu-bound complete counters=[80000000,80000000] timer-preemptions=[9,10]
[user] bootfs init ELF entered EL0
[bootfs] init ELF executed at EL0
PID 2 shell running cpu=0 ticks=[327,324]
uptime: 2495 ms
[system] shutdown
```

脚本摘要：`smoke-qemu: PASS`，CPU0/CPU1 ticks=327/324、uptime=2495 ms，
调度计数精确 80,000,000 且两核 timer preemption 均大于零。`make fsck` 输出：

```text
MFS1 clean generation=1 transactions=0 entries=1 used_blocks=0
```

### 最终整合 OrbStack 门禁（2026-08-10 00:07 +08）

在加入 INTx 稳定 marker 和 `completions=N`（要求 `N>0`）解析后，Luna MAX
在 OrbStack 执行一次完整 `make test`（本轮未重复执行 `make fsck`），结果为
exit 0。Host 计数为：mfs1 recovery 6/6、ABI 6/6、kernel
`core_boundaries` 9/9（含 generation-reuse/revoke 回归），doc-tests 0。

QEMU smoke 通过 VirtIO、SMMU、INTx、双核调度、IPC、bootfs、Shell 和 shutdown
门禁；关键稳定输出如下：

```text
[virtio] modern features=VERSION_1+ACCESS_PLATFORM+FLUSH ...
[irq] virtio-blk INTx pin=1 gic-id=37 bound cpu0
[irq] virtio-blk INTx completions=4
[iommu] SMMUv3 domain stream-id=0x10 iova=0x100000 ... gerror=0x0
[virtio] queue0 read+write sector=0 status=ok mfs1=true flush=ok
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x402a6000 completion-error=false
[sched] round-robin context-switches=20 progress=[26222005,26217632]
[ipc] call/recv/reply endpoint=1 request=0x1234 reply=0x2468 asids=[3,4]
PID 2 shell running cpu=0 ticks=[323,318]
uptime: 2530 ms
[system] shutdown
```

五个 bootfs 服务 ELF 均进入/执行于 EL0；EL0 fault probe 正常终止目标任务且
内核继续运行。未观察 panic、timeout、IRQ storm 或 SMMU translation fault。

### 常驻 EL0 服务 smoke 门禁迁移（2026-08-10）

常驻服务底座重建成功（OrbStack `make build` exit 0）。已有
`target/resident-smoke-qemu.log` 显示 QEMU 已完成完整服务启动和 shutdown；此前
smoke 脚本 exit 1 的唯一原因是旧门禁仍要求五条
`[bootfs] ... ELF executed at EL0`，不是 guest fault、panic 或 timeout。

```text
[service] resident EL0 address-spaces=5 asids=[0x20..0x24]
[user] bootfs init ELF entered EL0
[user] console service ELF entered EL0
[user] block service ELF entered EL0
[user] mfs service ELF entered EL0
[user] shell service ELF entered EL0
[service] resident EL0 ready=5/5 switches=4
[service] root task ready
[service] block transport ready (VirtIO-PCI)
[service] mfs1 ready (host format v1)
[service] shell ready
PID 2 shell running cpu=0 ticks=[119,115]
uptime: 1490 ms
sync: no dirty kernel buffers
[system] shutdown
```

`scripts/smoke-qemu.sh` 已移除五条过时的 `executed at EL0` required markers，保留
五条用户态入口 marker，并新增 resident address-space/ASID 与
`ready=5/5 switches=N` marker；解析要求 `N>0`。使用上述既有日志做匹配得到
`switches=4`，`bash -n scripts/smoke-qemu.sh` 通过；本次未启动 QEMU 或执行全量测试。

### 常驻服务 IPC 诊断（2026-08-10）

OrbStack `make build` 成功（exit 0）。随后仅执行一次 smoke，日志保存为
`target/resident-ipc-smoke-qemu.log`。guest 正常完成服务启动、Shell 命令和
shutdown；smoke exit 1 的直接原因是脚本 readiness grep 使用了单引号包裹的
双重反斜杠（`scripts/smoke-qemu.sh:147`），未匹配实际存在的 resident ready
行，不是 guest fault、panic 或 timeout。本次不修改 required markers、不重复运行。

六条 resident IPC marker 全部命中：

```text
[ipc] resident console endpoint=1 ready
[ipc] resident init->console reply=0x11
[ipc] resident block endpoint=2 ready
[ipc] resident mfs->block capacity-sectors=131072
[ipc] resident mfs endpoint=3 ready
[ipc] resident shell->mfs magic=MFS1
```

其余关键输出：

```text
[service] resident EL0 ready=5/5 switches=5
[sched] round-robin context-switches=20 progress=[20272715,21533451]
PID 2 shell running cpu=0 ticks=[173,168]
uptime: 1745 ms
[system] shutdown
```

无 panic、timeout 或 translation fault；仅预期 isolation fault
`ESR=0x92000006 ELR=0x400070 FAR=0x600000`，随后报告 kernel survived。

### resident readiness/IPC smoke 门禁修复（2026-08-10）

修复 `scripts/smoke-qemu.sh` 的 resident readiness 正则双重转义，并修正
`switches` 提取的替换表达式；保留 `switches>0` 门禁。将本轮真实出现的六条
resident IPC 行加入 required markers：console endpoint ready、init→console
reply、block endpoint ready、mfs→block capacity、mfs endpoint ready、shell→mfs
MFS1 magic。

未启动 QEMU。使用 `target/resident-ipc-smoke-qemu.log` 逐项执行脚本中的 32 个
required marker 匹配，结果全部通过；INTx completions=4、resident switches=5，
并执行 `bash -n scripts/smoke-qemu.sh` 通过。

### EL0 block-driver 纵向切片（2026-08-10）

OrbStack `make build` exit 0；随后仅执行一次 smoke，日志为
`target/el0-block-smoke-qemu.log`，脚本 PASS（exit 0）。用户态 block driver
已完成 queue0 单扇区读写及 flush：

```text
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
```

六条 resident IPC marker 均出现（日志中为异步顺序）：console endpoint=1
ready、init→console reply=0x11、block endpoint=2 ready、mfs→block
capacity-sectors=131072、mfs endpoint=3 ready、shell→mfs magic=MFS1。

```text
[service] resident EL0 ready=5/5 switches=9
[irq] virtio-blk INTx completions=4
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x402fc000 completion-error=false
PID 2 shell running cpu=0 ticks=[120,114]
uptime: 1460 ms
[system] shutdown
```

无 panic、timeout 或 translation fault；仅预期 isolation fault
`ESR=0x92000006 ELR=0x400070 FAR=0x600000`，随后 kernel survived。RR
context-switches=20、progress=`[21891546,24886736]`，scheduler
timer-preemptions=`[10,9]`。

### EL0 MFS1 纵向切片诊断（2026-08-10）

OrbStack `make build` 成功（exit 0）；单次 smoke 日志为
`target/el0-mfs-smoke-qemu.log`，smoke exit 1。block、SMMU 和 INTx 已先通过：

```text
[virtio] queue0 read+write sector=0 status=ok mfs1=true flush=ok
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x4080b000 completion-error=false
[irq] virtio-blk INTx completions=4
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
```

MFS ELF 尚未打印 entered 行即发生首个新增故障：

```text
[fault] user exception ESR=0x92000047 ELR=0x40001c FAR=0x5fefa0 x0=0x0 x1=0x0 x2=0x0 x8=0x0 ttbr0=0x230000405e1000; task terminated
```

因此本次未观察到 `mfs1 mounted via EL0 block IPC`、`mfs1 fsync
/boot-proof persistent=true`、mfs endpoint、shell→mfs、resident ready 或
Shell/shutdown；没有用户态正常 exit。无 panic、timeout、allocator fault 或 QEMU
SMMU translation fault；该 ESR=0x92000047 是 EL0 Data Abort/translation fault。
日志定位为 MFS `_start` 的 `sub sp,#0x1000` 后栈写入落到 `0x5fefa0`，而
`service_runtime::load_task` 仅映射 `0x5ff000` 栈页。

### EL0 MFS1 栈扩容回归（2026-08-10）

服务栈扩为 8 页后，OrbStack `make build` 成功（exit 0）。单次 smoke 日志为
`target/el0-mfs-stack-smoke-qemu.log`，guest 已完整启动并正常 shutdown；脚本
exit 1 的唯一缺失 marker 是 mfs→block capacity 行被宿主输入交错破坏：

```text
micro> [ipc] resident mfs->block capacity-helpsectors=131072
```

因此不是运行时故障。其余 MFS 与服务链路均成功：

```text
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 fsync /boot-proof persistent=true
[ipc] resident mfs endpoint=3 ready
[ipc] resident shell->mfs magic=MFS1
[service] resident EL0 ready=5/5 switches=5
[system] shutdown
```

无新 translation fault、allocator fault、panic 或 timeout；仅预期 isolation
fault `ESR=0x92000006 FAR=0x600000`，随后 kernel survived。

### EL0 block-driver queue0 门禁（2026-08-10）

将已在 `target/el0-block-smoke-qemu.log` 真实命中的用户态 block-driver 行加入
`scripts/smoke-qemu.sh` required markers：

```text
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
```

未启动 QEMU；`bash -n scripts/smoke-qemu.sh` 通过，并使用上述既有日志确认该
marker 命中。

### EL0 MFS1 online 服务回归（2026-08-10）

更新 resident online 门禁后，OrbStack `make build` 成功（exit 0）；单次 smoke
日志为 `target/el0-mfs-online-smoke-qemu.log`，exit 0 / PASS。输入无交错，
完整门禁均通过：

```text
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 fsync /boot-proof persistent=true
[ipc] resident block endpoint=2 ready
[ipc] resident mfs->block capacity-sectors=131072
[ipc] resident mfs endpoint=3 ready
[ipc] resident shell->mfs magic=MFS1
[service] resident EL0 ready=5/5 online=5/5 switches=405
[irq] virtio-blk INTx completions=4
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x4082e000 completion-error=false
PID 2 shell running cpu=0 ticks=[183,179]
uptime: 1775 ms
[system] shutdown
```

无 panic、timeout、translation fault 或 allocator fault；仅预期 isolation fault
后 kernel survived。RR context-switches=20、progress=`[22276446,22562908]`，
调度 timer-preemptions=`[10,10]`。`bash -n scripts/smoke-qemu.sh` 通过，并以
该日志确认 35 个 required markers、resident switches=405 且无输入交错。

### EL0 MFS1 重启恢复诊断（2026-08-10）

OrbStack `make build` 成功（exit 0）；随后仅执行一次 smoke，日志为
`target/el0-mfs-recovery-smoke-qemu.log`，exit 0 / PASS。本轮没有执行 fsck。
恢复顺序严格早于本轮重新 fsync：

```text
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 recovered /boot-proof after restart=true
[user] mfs1 fsync /boot-proof persistent=true
```

resident/IPC、VirtIO、SMMU 和 INTx 门禁均通过：resident
`ready=5/5 online=5/5 switches=41`，INTx completions=4，fault probe
`blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x4082e000 completion-error=false`。
Shell/ps 无输入交错：`PID 5 shell running cpu=1 ticks=[77,72]`、uptime
`1255 ms`、sync clean，最终 `[system] shutdown`。RR
context-switches=20、progress=`[25251354,24371190]`，timer-preemptions=`[10,9]`。
无新 translation/allocator fault、panic 或 timeout；仅预期 isolation fault 后
kernel survived。

### MFS1 重启恢复 required marker（2026-08-10）

将已在 `target/el0-mfs-recovery-smoke-qemu.log` 命中的恢复行加入
`scripts/smoke-qemu.sh` required markers：

```text
[user] mfs1 recovered /boot-proof after restart=true
```

仅对现有日志执行验证：`bash -n scripts/smoke-qemu.sh` 通过，恢复 marker 位于
第 52 行，本轮 fsync marker 位于第 53 行，顺序为 recovered 先于 fsync。未启动
QEMU、未执行全量测试。

### EL0 console + shell 迁移诊断（2026-08-10）

OrbStack `make build` 成功（exit 0）；单次 smoke 日志为
`target/el0-shell-smoke-qemu.log`，exit 0 / PASS。输入无交错，EL0 shell 经
console IPC 完整响应命令：

```text
micro> help
help ps uptime ls cat write mkdir run sync shutdown
micro> ps
PID 5 shell running cpu=1 ticks=[190,186]
micro> uptime
uptime: 2165 ms
micro> sync
sync: ok
micro> shutdown
[system] shutdown
```

`[service] shell ready`、MFS mount/recovery/fsync、resident online
`ready=5/5 online=5/5 switches=39`、resident IPC、block/SMMU/INTx 门禁均通过；
SMMU fault probe blocked=true，INTx completions=4。无 UART mapping、console IPC、
shell parser、FS sync 或退出故障；无 panic、timeout、translation/allocator fault，
仅预期 isolation fault 后 kernel survived。RR context-switches=20、progress=
`[22160271,24626109]`，timer ticks=`[190,186]`。

### EL0 Shell 文件链回归（2026-08-10）

smoke 输入顺序扩展为 `mkdir /demo`、`write /demo/hello
hello-microsystem`、`sync`、`ls /demo`、`cat /demo/hello` 后 shutdown。OrbStack
`make build` exit 0；单次日志 `target/el0-shell-fs-smoke-qemu.log`，smoke
exit 0 / PASS，脚本摘要 `file-chain mkdir/write/sync/ls/cat=ok`。独立输出门禁
均命中：

```text
micro> mkdir /demo
mkdir: ok
write /demo/hello hello-microsystem
write: ok
sync: ok
/demo/hello
hello-microsystem
[system] shutdown
```

MFS mount/recovery/fsync、resident `ready=5/5 online=5/5 switches=48`、IPC、
block queue、SMMU fault probe blocked、INTx completions=4 均通过；Shell/ps
ticks=`[191,186]`、uptime=`2159 ms`。无输入回显误判、UART/IPC 错误、panic、
timeout、translation/allocator fault；仅预期 isolation fault 后 kernel survived。
RR context-switches=20、progress=`[23004741,22913633]`，timer-preemptions=`[10,9]`。

### 正式 smoke procman/run 门禁更新（2026-08-10）

正式 `scripts/smoke-qemu.sh` 已更新为 bootfs `entries=7 static-elfs=6`，并要求
procman endpoint、`run: counter pid=6`、counter EL0 started/completed 及
`[proc] application pid=6 exited status=0`。文件链之后的输入现在发送
`run counter`，等待 2 秒，再发送一次 `ps` 后 shutdown，以验证应用回收后 Shell
仍可响应。

未启动 QEMU。`bash -n scripts/smoke-qemu.sh` 通过；使用既有
`target/el0-procman-smoke-qemu.log` 验证新 bootfs/procman/counter markers，使用
既有 `target/el0-shell-fs-smoke-qemu.log` 分别验证文件链 markers；输入顺序及
2 秒等待均确认。正式门禁已更新，待下一次统一回归执行。

### procman/run EL0 专用诊断（2026-08-10）

`make build` 成功（exit 0，包含 counter bootfs ELF）。使用有界自定义输入
`run counter` → 等待约 2 秒 → `ps` → `shutdown`，QEMU status=0，日志为
`target/el0-procman-smoke-qemu.log`；未修改正式 smoke 门禁。

```text
[bootfs] valid=true entries=7 static-elfs=6
[ipc] resident procman endpoint=4 ready
run: counter pid=6
[app] counter pid=6 started at EL0
[app] counter pid=6 completed
[proc] application pid=6 exited status=0
PID 1 init running cpu=1
PID 2 console running cpu=1
PID 3 block blocked cpu=1
PID 4 mfs blocked cpu=1
PID 5 shell running cpu=1 ticks=[450,445]
[system] shutdown
```

MFS mount/recovery/fsync、resident online `ready=5/5 online=5/5 switches=50986`、
普通 resident IPC、VirtIO queue、SMMU fault probe blocked、INTx completions=4 均
正常。无 panic、timeout、translation/allocator fault 或其他 fault；仅预期
isolation fault 后 kernel survived。日志有一处非致命 UART 交错
`[service] mfs1 ready (resident EL0)micro>`，但 procman、应用、ps 和 shutdown
marker 均未受影响。

### 最终统一 OrbStack 验收（2026-08-10）

新增 capability generation/rights/revoke、IPC absolute-deadline 和 block
notification bitset 门禁后，完整 OrbStack `make test` exit 0；随后条件性
`make fsck` 也 exit 0。Host 逻辑计数：mfs recovery 6/6、ABI capability_abi
6/6、kernel core_boundaries 9/9，其余 unit/doc tests 0 passed/0 failed。QEMU
smoke 日志为 `target/smoke-qemu.log`（96 行），PASS。

关键串口门禁全部命中：

```text
[bootfs] valid=true entries=7 static-elfs=6
[cap] resident generation rights revoke stale-handle=true
[ipc] resident absolute-deadline timeout=true
[irq] resident block notification bitset=true
[service] resident EL0 ready=5/5 online=5/5 switches=7657
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 recovered /boot-proof after restart=true
[user] mfs1 fsync /boot-proof persistent=true
mkdir: ok
write: ok
sync: ok
/demo/hello
hello-microsystem
[ipc] resident procman endpoint=4 ready
run: counter pid=6
[app] counter pid=6 started at EL0
[app] counter pid=6 completed
[proc] application pid=6 exited status=0
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x40967000 completion-error=false
[irq] virtio-blk INTx completions=4
PID 5 shell running cpu=1 ticks=[1865,1858]
uptime: 2982 ms
[system] shutdown
```

resident IPC、MFS、文件链、procman 回收、SMMU 和 INTx 均通过；无非预期 fault、
panic、timeout、translation 或 allocator fault，仅预期 isolation fault 后
kernel survived。`make fsck` 输出：

```text
MFS1 clean generation=10 transactions=9 entries=4 used_blocks=19
```

### 跨表 capability / notification 严格门禁（2026-08-10）

审查发现，原有 kernel 9/9 仅覆盖单表派生树、同槽 generation 回收和普通
revoke；没有跨 `CapabilityTable` 的稳定 node 派生/revoke，也没有目标槽已满时
move 失败的原子性回归。ABI 测试只锁定基础 syscall 编号，未锁定
ThreadStatus/ThreadKill 与 process Wait/List/Kill；原 smoke 也没有约束跨任务
copy/move/revoke 或 NotificationWait 阻塞计数。

已补充两个聚焦 kernel 测试（跨表 stable-node + generation reuse/global revoke、
满槽 move failure 保持源句柄）和 ABI 的 ThreadStatus/ThreadKill、Wait/List/Kill
编号断言；正式 smoke 增加 cross-task cap、absolute-deadline、notification
bitset、wait/list 退出状态门禁。

本轮一次完整 OrbStack `make test` 的 host 计数为 mfs 6/6、ABI 6/6、kernel
11/11，QEMU 输出包含 cap/deadline/notification、cross-task `atomic=true`、
procman wait/list、文件链、MFS、SMMU、INTx、双核 ticks 和 shutdown；此前条件性
fsck 输出 generation=12、transactions=11、entries=4、used_blocks=23。为兑现
“三个 notification N 均 >0”而加入严格解析并复核同一日志，首个失败为：

```text
[irq] resident notification blocking-waits=0 irq-acks=141 sgi-wakeups=154
```

因此当前严格 notification gate 未通过（blocking-waits 必须大于零），不将本轮
称为最终全绿；未再次启动 QEMU 或 fsck，未修改生产代码。此前日志其余门禁无
非预期 fault、panic 或 timeout，仅预期 isolation fault 后 kernel survived。

### NotificationWait 修复后的严格回归（2026-08-10）

生产加入真实 2 ms absolute-deadline NotificationWait 探针后，OrbStack
`make build` 成功（exit 0），但正式 `make test` 首错失败（exit 2），因此按条件
未执行 fsck。Host 逻辑仍全部通过：mfs 6/6、ABI 6/6、kernel core_boundaries
11/11。QEMU 日志为 `target/smoke-qemu.log`：

```text
[bootfs] valid=true entries=7 static-elfs=6
[virtio] queue0 read+write sector=0 status=ok mfs1=true flush=ok
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x4096b000 completion-error=false
[irq] virtio-blk INTx completions=4
[ipc] resident absolute-deadline timeout=true
[ipc] resident procman endpoint=4 ready
[user] shell service ELF entered EL0
[service] critical service shell exited status=2
```

首个运行时失败是 Shell 初始 MFS `ipc_call(3, Stat)` 返回错误，导致 shell status=2；
后续 notification counters、MFS recovery/file-chain、procman wait/list/counter
未到达。已通过的 stale/cross-task capability、deadline、bootfs、VirtIO/SMMU/INTx、
RR/IPC 门禁未回退。无 CPU fault、panic、timeout、translation 或 allocator fault；
推断 service runtime 在该初始 IPC 时没有可运行的 next task，返回 `Busy`。未改生产、
未重复运行、未执行 fsck。

### IPC Busy 重试后的 procman 首错（2026-08-10）

共享 user-rt `ipc_call` 增加 Busy 重试后，OrbStack `make build` 成功（exit 0），但
strict `make test` 仍 exit 2，按条件未执行 fsck。Host 逻辑全绿：mfs 6/6、ABI
6/6、kernel core_boundaries 11/11。QEMU `target/smoke-qemu.log` 已通过 bootfs
7/6、VirtIO/SMMU/INTx、RR/IPC、absolute-deadline、stale/cross-task capability
markers，并到达 procman endpoint=4：

```text
[bootfs] valid=true entries=7 static-elfs=6
[ipc] resident absolute-deadline timeout=true
[cap] resident generation rights revoke stale-handle=true
[cap] resident cross-task copy-move revoke atomic=true
[ipc] resident procman endpoint=4 ready
[service] critical service init exited status=3
```

首错为 init 的 endpoint4 `ipc_recv` 在没有可运行 next task 时返回 Busy，init 将其
映射为 status=3 并退出；procman wait/list/counter、notification 三计数、MFS/file
chain 和 shutdown 均未到达。无 CPU fault、panic、timeout、translation 或 allocator
fault；未改生产、未重复运行、未执行 fsck。

### IPC Busy 重试与 NotificationWait 最终严格回归（2026-08-10）

共享 user-rt 扩展 `ipc_recv`/`ipc_reply_recv` Busy 重试后，OrbStack 严格流程全绿：
`make build`、`make test`、`make fsck` 均 exit 0。Host 计数为 mfs recovery 6/6、
ABI 6/6、kernel core_boundaries 11/11，其余 unit/doc 0/0；QEMU 日志为
`target/smoke-qemu.log`。

```text
[bootfs] valid=true entries=7 static-elfs=6
[cap] resident generation rights revoke stale-handle=true
[cap] resident cross-task copy-move revoke atomic=true
[ipc] resident absolute-deadline timeout=true
[irq] resident notification blocking-waits=2 irq-acks=162 sgi-wakeups=171
[service] resident EL0 ready=5/5 online=5/5 switches=10516
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 recovered /boot-proof after restart=true
[user] mfs1 fsync /boot-proof persistent=true
mkdir: ok
write: ok
sync: ok
/demo/hello
hello-microsystem
run: counter pid=6
[app] counter pid=6 started at EL0
[app] counter pid=6 completed
wait: pid=6 status=0
PID 6 counter exited status=0
[proc] application pid=6 exited status=0
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x4096c000 completion-error=false
[irq] virtio-blk INTx completions=4
PID 5 shell running cpu=1 ticks=[1895,1891]
uptime: 2973 ms
[system] shutdown
```

三项 notification 计数均为正；cap 原子/跨任务 revoke、MFS recovery/fsync、文件链、
proc wait/list/counter、SMMU 与 INTx 均通过。无非预期 fault、panic、timeout、
translation 或 allocator fault，仅预期 isolation fault 后 kernel survived。`make fsck`：

```text
MFS1 clean generation=14 transactions=13 entries=4 used_blocks=27
```

### devmgr 四-cap IPC 首错（2026-08-10）

现有 smoke 原先只验证 block 从启动寄存器获得 transport，未验证 root/devmgr
通过 endpoint5 原子 copy 四个真实 cap（MmioRegion、queue Frame、DmaDomain、Irq）
并由 block 回复；已将稳定 marker
`[devmgr] resident root granted mmio/frame/dma/irq to block via IPC` 加入门禁。

本轮 OrbStack `make build` 成功（exit 0），但 strict `make test` exit 2，未执行
fsck。Host mfs 6/6、ABI 6/6、kernel core_boundaries 11/11 全绿；QEMU 首错：

```text
[bootfs] valid=true entries=7 static-elfs=6
[virtio] queue0 read+write sector=0 status=ok mfs1=true flush=ok
[iommu] fault-probe blocked=true sentinel=true event=0x10 stream-id=0x10 iova=0x4096c000 completion-error=false
[cap] resident cross-task copy-move revoke atomic=true
[service] critical service init exited status=15
```

init status=15 对应 `grant_block_transport` 收到 common/notify/isr/device/
multiplier/DMA 参数中的零值；当前运行时只向 BLOCK_TASK 注入 grant，未向 init task
注入 transport 描述。故 devmgr marker、notification、MFS/proc/file/shutdown 未到达。
无 CPU fault、panic、timeout、translation 或 allocator fault；未改生产、未重复运行、
未执行 fsck。

### devmgr transport 注入后的 block 首错（2026-08-10）

修复 `enter_service` 完整注入 init transport 寄存器后，OrbStack
`make build` 成功（exit 0）；strict `make test` exit 2，未执行 fsck。Host MFS/ABI/
kernel 11/11 全绿；QEMU 在 block 服务真实初始化阶段首错：

```text
[user] block service ELF entered EL0
[user] mfs service ELF entered EL0
[user] shell service ELF entered EL0
[cap] resident cross-task copy-move revoke atomic=true
[service] critical service block exited status=4
```

devmgr 四-cap marker 尚未出现，后续 queue/notification/proc/MFS/file/shell/shutdown
均未到达。block status=4 是其合并 guard（IrqBind 失败、NotificationWait 超时探针
未返回 `TimedOut` 或真实 block probe 失败），当前日志没有更细的子项诊断。无 CPU
fault、panic、timeout、translation 或 allocator fault；未改生产、未重复运行、未执行
fsck。

### block status4 拆分定位（2026-08-10）

按生产新增的拆分状态码执行一次有界诊断：`make build` exit 0。宿主 macOS 预检因
缺少 `timeout(1)` 未启动 guest；随后在 OrbStack 容器内执行唯一实际 smoke，日志为
`target/devmgr-status-smoke-qemu.log`，脚本 exit 1。相邻串口为：

```text
[user] block service ELF entered EL0
[user] mfs service ELF entered EL0
[user] shell service ELF entered EL0
[ipc] resident console endpoint=1 ready
[ipc] resident init->console reply=0x11
[ipc] resident absolute-deadline timeout=true
[cap] resident generation rights revoke stale-handle=true
[cap] resident cross-task copy-move revoke atomic=true
[service] critical service block exited status=8
```

拆分结果明确为 status=8（deadline/NotificationWait probe 失败），不是 status=4
参数、7 IrqBind 或 9 block_probe。devmgr 四-cap、block queue/notification、resident
ready、proc/MFS/file/shell/shutdown 未到达；早期 bootfs/VirtIO/SMMU/INTx/RR/IPC/cap
门禁通过。无非预期 fault、panic、timeout、translation 或 allocator fault；未改生产、
未跑 host tests/fsck。

### Notification probe 16 轮修复后的 devmgr smoke（2026-08-10）

修复 stale-bit/IRQ storm 处理后，OrbStack `make build` exit 0；仅运行一次正式
smoke（日志 `target/devmgr-status16-smoke-qemu.log`），exit 0 / PASS。本轮未跑
host tests 或 fsck。关键链路：

```text
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
[irq] resident block notification bitset=true
[ipc] resident block endpoint=2 ready
[ipc] resident mfs->block capacity-sectors=131072
[devmgr] resident root granted mmio/frame/dma/irq to block via IPC
[ipc] resident procman endpoint=4 ready
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 recovered /boot-proof after restart=true
[user] mfs1 fsync /boot-proof persistent=true
[ipc] resident mfs endpoint=3 ready
[ipc] resident shell->mfs magic=MFS1
[service] resident EL0 ready=5/5 online=5/5 switches=6897
[irq] resident notification blocking-waits=6 irq-acks=184 sgi-wakeups=198
PID 6 counter exited status=0
PID 5 shell running cpu=1 ticks=[1700,1695]
uptime: 2067 ms
[system] shutdown
```

无 block status/首错、panic、timeout、translation 或 allocator fault；RR
context-switches=20、progress=`[24280600,25186695]`，INTx completions=4，文件链
通过。

### MFS 故障计划与 DmaMap/后台回归（2026-08-10）

在现有 `crates/mfs1/tests/recovery.rs` 的 `MemDisk` 上增加了四阶段提交故障
（事务记录写、第一次 flush、备用超级块写、第二次 flush）及 GC 搬迁中断；每个
场景重挂载后只允许 old/new 完整快照，且执行 `check()`。OrbStack `make build`
成功（exit 0）。Host 回归全绿：MFS recovery 8/8（含上述 fault-plan 与 GC
migration）、ABI 6/6、kernel `core_boundaries` 11/11。

随后唯一一次正式 `make test` exit 2，按首错策略未执行 `make fsck`。QEMU 日志
`target/smoke-qemu.log` 的新 DmaMap 门禁已命中，但 block 在 deadline/
NotificationWait 探针退出 status=8：

```text
[iommu] resident block DmaMap capability=true
[service] critical service block exited status=8
```

因此 devmgr 四-cap、`background-writeback=1s online-gc=true`、queue/notification
bitset、MFS mount/recovery/fsync、resident/proc/file/shell/shutdown 尚未到达；无
非预期 panic、translation 或 allocator fault（仅预期 isolation fault）。生产代码
未修改；fsck 因 smoke 首错未执行。

### Generic Notification deadline 门禁回归（2026-08-10）

新增 smoke required marker：
`[irq] resident generic notification deadline-block=true`，用于区分未绑定
Notification 的 2ms deadline 阻塞探针与真实设备 IRQ 的 blocking/ack/SGI 计数。
`bash -n scripts/smoke-qemu.sh` 通过。

OrbStack `make build` 成功（exit 0）；唯一一次严格 `make test` exit 2，按首错策略
未执行 fsck。Host 回归仍全绿：MFS fault-plan 8/8、ABI 6/6、kernel
`core_boundaries` 11/11。QEMU `target/smoke-qemu.log` 首个新错误为：

```text
[service] critical service init exited status=19
```

status=19 对应 `verify_notification_deadline` 的 wait/delete 断言失败，因此
generic deadline marker、notification 三计数、DmaMap/background-writeback、devmgr、
block/MFS/proc/file/shell/shutdown 均未到达。无非预期 panic、translation 或 allocator
fault（仅预期 isolation fault）；未改生产代码，fsck 未执行。

### Proc Kill/reclaim 与 bootfs8 严格回归（2026-08-10）

现有 smoke 门禁更新为 `[bootfs] valid=true entries=8 static-elfs=7`，并要求
spinner 真实进入 EL0 以及 Kill 后回收：`[app] spinner pid=6 started at EL0`、
`[proc] spinner pid=6 killed status=-15 reclaimed=true`。OrbStack `make build`、
严格 `make test`、`make fsck` 均 exit 0。

Host：MFS 8/8、ABI 6/6、kernel `core_boundaries` 11/11。QEMU 通过 spinner
Kill/reclaim 后 PID6 复用 counter（started/completed/exited=0/wait=0）；endpoint
capability、generic deadline、DmaMap、后台回写/在线 GC、devmgr、block/MFS/proc/file
链、resident ready=5/5 switches=10503 均通过。Notification
`blocking-waits=3 irq-acks=355 sgi-wakeups=355`，ticks=`[1895,1891]`、uptime=2984ms，
RR 20、INTx completions=4，最终 shutdown；无非预期 fault/panic/timeout/translation/
allocator。fsck：`MFS1 clean generation=22 transactions=21 entries=4 used_blocks=43`。

### Generic Notification deadline 修复后的严格全量回归（2026-08-10）

生产将 generic deadline probe 提前到 init 首条 debug/IPC 之前，并让 user-rt
`notification_wait` 仅对 Busy 重试、保持原 absolute deadline。OrbStack `make build`
exit 0；严格 `make test` exit 0，随后 `make fsck` exit 0。

Host 计数：MFS recovery/fault-plan 8/8、ABI 6/6、kernel `core_boundaries` 11/11。
QEMU `target/smoke-qemu.log` 全部门禁通过：

```text
[irq] resident generic notification deadline-block=true
[iommu] resident block DmaMap capability=true
[user] mfs1 background-writeback=1s online-gc=true
[devmgr] resident root granted mmio/frame/dma/irq to block via IPC
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
[irq] resident block notification bitset=true
[ipc] resident block endpoint=2 ready
[ipc] resident mfs->block capacity-sectors=131072
[user] mfs1 mounted via EL0 block IPC
[user] mfs1 recovered /boot-proof after restart=true
[user] mfs1 fsync /boot-proof persistent=true
[service] resident EL0 ready=5/5 online=5/5 switches=8190
[irq] resident notification blocking-waits=8 irq-acks=293 sgi-wakeups=291
```

文件链、proc counter run/start/complete/wait/exit、MFS endpoint/shell IPC、INTx
completions=4 均通过；双核 ticks=`[1882,1874]`、uptime=2967ms，最终
`[system] shutdown`。RR context-switches=20，progress=`[19420488,21376045]`。
无非预期 fault、panic、timeout、translation 或 allocator fault。

离线检查：`MFS1 clean generation=18 transactions=17 entries=4 used_blocks=35`。

### Endpoint capability enforcement 严格回归（2026-08-10）

现有 smoke 增加 `[ipc] resident endpoint capabilities enforced=true` 门禁，覆盖
未授权 init 调用 BLOCK_ENDPOINT 必须返回 `BadCapability`；未新增测试文件，现有
ABI CapHandle/Message 契约测试保持不变。OrbStack `make build`、严格 `make test`
及 `make fsck` 均 exit 0。

Host：MFS 8/8、ABI 6/6、kernel `core_boundaries` 11/11。QEMU 门禁完整通过：

```text
[ipc] resident endpoint capabilities enforced=true
[irq] resident generic notification deadline-block=true
[iommu] resident block DmaMap capability=true
[user] mfs1 background-writeback=1s online-gc=true
[devmgr] resident root granted mmio/frame/dma/irq to block via IPC
[service] resident EL0 ready=5/5 online=5/5 switches=12377
[irq] resident notification blocking-waits=2 irq-acks=317 sgi-wakeups=320
```

block queue read/write/flush、notification、MFS mount/recovery/fsync、proc counter
start/complete/wait/exit、文件链、INTx completions=4、双核 ticks=`[1833,1828]`、
uptime=2979ms、RR 20 switches、shell shutdown 均通过；无非预期 fault/panic/timeout/
translation/allocator。fsck：`MFS1 clean generation=20 transactions=19 entries=4
used_blocks=39`。

### Privileged instruction isolation 与 bootfs9 回归（2026-08-10）

现有 smoke 门禁更新为 `entries=9 static-elfs=8`，要求 privprobe 进入 EL0、内核
应用 fault 回收及 init 的 `status=-8 reclaimed=true`。首次严格 `make test` 的
guest 已完整运行，但测试解析器错误地只接受 EC=0x18；cortex-a72 实际报告
`ESR=0x2000000`（EC=0、IL=1），因此该次 exit 2，未执行 fsck。Host 仍为 MFS
8/8、ABI 6/6、kernel 11/11；无 guest panic/timeout。

随后将 ESR gate 修正为接受 EC=0/IL=1 或架构实现的 EC=0x18（仍不锁定 FAR/ELR），
`bash -n` 通过；复用同一构建产物仅运行一次 smoke，日志为
`target/privprobe-esr-smoke-qemu.log`，PASS：

```text
[bootfs] valid=true entries=9 static-elfs=8
[app] spinner pid=6 started at EL0
[proc] spinner pid=6 killed status=-15 reclaimed=true
[app] privileged probe pid=6 entered EL0
[proc] application fault ESR=0x2000000 FAR=0x0; pid=6 terminated
[isolation] privileged instruction task faulted status=-8 reclaimed=true
[app] counter pid=6 started at EL0
[app] counter pid=6 completed
```

后续 endpoint capability、generic deadline、DmaMap/background、devmgr、block/MFS/
proc/file/shell、resident ready=5/5 switches=11609、notification
`blocking-waits=6 irq-acks=405 sgi-wakeups=403`、ticks=`[1721,1716]`、RR/INTx 及
shutdown 全通过；无非预期 fault、panic、timeout、translation 或 allocator。`make
fsck` exit 0：`MFS1 clean generation=26 transactions=25 entries=4 used_blocks=51`。

### MemoryPool quota 与 FrameMap/Unmap remap 回归（2026-08-10）

该行为由 EL0 init 的实际配额/映射路径覆盖，未新增形式化 unit；现有 smoke 增加
`[mmu] resident memory-pool quota=4 frame-map/unmap remap=true` 门禁。OrbStack
`make build`、严格 `make test`、`make fsck` 均 exit 0。

Host：MFS 8/8、ABI 6/6、kernel `core_boundaries` 11/11。QEMU memory-pool marker、
privprobe fault/reclaim、spinner Kill/reclaim、counter PID6 复用及 endpoint-capability、
generic deadline、DmaMap/background、devmgr、VirtIO/SMMU/MFS/proc/file/shell 全链通过。
Resident switches=9776；notification `blocking-waits=5 irq-acks=426 sgi-wakeups=428`；
ticks=`[1889,1885]`、uptime=2978ms、RR 20、INTx=4、shutdown 正常。无非预期
fault/panic/timeout/translation/allocator。fsck：`MFS1 clean generation=28 transactions=27
entries=4 used_blocks=55`。

### MFS checkpoint/segment 持久化回归（2026-08-10）

现有 `recovery.rs` 的旧 CRC 负向契约已改为两次提交后破坏最新事务 payload，断言
挂载回退上一代、old/new 不混合且 `check()` 成功；GC 测试新增 checkpoint arena 起点、
segment 计数与 arena 切换断言。这些是持久化恢复不变量回归，并非覆盖率测试。smoke
新增 `[user] mfs1 checkpoint valid=true segment-blocks=256` 门禁；rustfmt/bash-n 均通过。

OrbStack `make build`、严格 `make test`、`make fsck` 均 exit 0。Host：MFS recovery
8/8（含最新事务回退与 GC checkpoint/segment 断言）、ABI 6/6、kernel 11/11。QEMU
checkpoint marker、旧磁盘恢复/本轮 fsync、新提交及 bootfs9/privprobe、spinner/counter
PID6 复用、endpoint/memory-pool/DmaMap/background/devmgr/block/MFS/proc/file/shell 全链
通过；resident switches=17212，notification `blocking-waits=2 irq-acks=464 sgi-wakeups=463`，
ticks=`[1843,1837]`、uptime=2981ms、RR 20、INTx=4，最终 shutdown。无非预期
fault/panic/timeout/translation/allocator。fsck：`MFS1 clean generation=30 transactions=29
entries=4 used_blocks=59`。

### 小盘在线 GC 与 QEMU power-cut 回归（2026-08-10）

在现有 `recovery.rs` 增加最小双 arena 小盘的 180 次覆盖写：实际跨过 15% low-water
触发 GC，断言每次最终内容为最新版，重挂载和 `check()` 一致，且 GC 后 free 至少
达到 25% high-water。新增测试保护的是在线 GC 的容量/恢复不变量，不是覆盖率计数。

新增 `scripts/powercut-qemu.sh` 并接入 `xtask test`：正常 smoke PASS 后生成唯一 token，
第一次启动执行 `write /powercut <token>`、确认独立 `sync: ok` 后由宿主 SIGTERM 强制
终止；第二次启动用独立 token 行匹配 `cat` 结果、MFS mount 和正常 shutdown，且检查
无 panic/timeout/translation/allocator fault。

OrbStack `make build`、`make test`（正常 smoke + powercut 双启动）、`make fsck` 均 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel 11/11。正常 smoke 的 resident switches=9257，
notification `blocking-waits=3 irq-acks=492 sgi-wakeups=497`，ticks=`[1827,1823]`、
uptime=2970ms、RR 20、INTx=4，完整服务链及 shutdown 全通过。Powercut token
`powercut-1786300771-481-19077`：first-status=0，日志
`target/powercut-1786300771-481-19077-first.log` 在 SIGTERM 前包含写入与 `sync: ok`；
second-status=0，`...-second.log` 重新挂载 MFS1、精确读回 token 并正常 shutdown。无
非预期 fault/panic/timeout/translation/allocator。fsck：`MFS1 clean generation=35
transactions=34 entries=5 used_blocks=69`。

### Powercut 第一阶段门禁收紧（2026-08-10）

`scripts/powercut-qemu.sh` 现明确要求 first log 含宿主 SIGTERM 证据
(`terminating on signal 15`/`SIGTERM`)，并拒绝出现 `[system] shutdown`，防止正常
关机误判为断电。`bash -n` 通过；对既有
`target/powercut-1786300771-481-19077-first.log` 静态验证两项门禁均命中（同时保留
`write: ok`、`sync: ok` 证据）。本次未重复启动 QEMU、未执行全量测试、未修改生产代码。

### OrbStack 容器化构建入口最终复核（2026-08-10）

Docker context=`orbstack`，现有镜像 `microsystem-dev:rust-1.97.1`
(`sha256:b833532c...`)。主入口切换为容器内构建后，唯一一次 `make build` 在
Dockerfile 预装 `aarch64-unknown-none-softfloat` 阶段首错退出 2：rustup 下载
rust-std（2026-07-16）因 `invalid peer certificate: UnknownIssuer` 失败。故本轮未
执行 `make test`（正常 smoke/powercut）或 `make fsck`；不是代码或 guest 失败，未改
代码。

### OrbStack target 下载重试（2026-08-10）

Docker context=`orbstack`，existing image `microsystem-dev:rust-1.97.1`
(`sha256:b833532c...`)。Dockerfile 的 apt/curl 安装阶段成功，但唯一一次
`make build` 仍在 `RUSTUP_USE_CURL=1 rustup target add aarch64-unknown-none-softfloat
--toolchain 1.97.1` 失败：curl `[60] SSL peer certificate or SSH remote key was not
OK`，OpenSSL verify result 19（self-signed certificate in chain），下载源为
`static.rust-lang.org` rust-std 2026-07-16。按首错未执行 `make test`（normal/powercut）
或 `make fsck`；未修改代码。

### Keychain CA 注入后的容器化最终验收（2026-08-10）

Docker context=`orbstack`；初始 image `sha256:b833532c...`，最终重建 image
`sha256:5e726088...`。`scripts/build-image.sh` 通过 BuildKit secret 注入 macOS
Keychain 公开 CA（Dockerfile #8 bundle/digest，#9 rustup target CACHED）；唯一
`make build` exit 0，`cargo run -p xtask -- build`、AArch64 target 的 ELF/kernel/mfsctl
均在容器内完成。

随后唯一 `make test` exit 0（内部 build、MFS 9/9、ABI 6/6、kernel 11/11、normal
smoke + powercut），`make fsck` exit 0。正常 smoke 的 bootfs9/8、checkpoint=256、
privprobe、spinner/counter PID6 复用、endpoint/memory-pool/DmaMap/background/devmgr/
block/MFS/proc/file/shell 全链通过；resident switches=10372，notification
`12/559/561`，ticks=`[1892,1888]`、uptime=2961ms、RR20、INTx4、shutdown。Powercut
token `powercut-1786301477-219-30830`，first/second status=0，first log 在 SIGTERM
前含 write+sync:ok，second log MFS mount、精确 token、正常 shutdown；无非预期
panic/timeout/translation/allocator。fsck：`MFS1 clean generation=40 transactions=39
entries=5 used_blocks=79`。

### 多应用槽 PID/ASID 隔离回归（2026-08-10）

现有 smoke 已从单槽 PID6 更新为独立应用槽：spinner PID7 启动、Kill 后
`slot-reused=true`，privprobe PID8 进入并 fault/reclaim；新增
`[proc] application slots=3 pids=[6,7,8] independent-asids=true`，并要求最终 ps
独立列出 PID6 counter status 0、PID7 spinner status -15、PID8 privprobe status -8。
`bootfs entries=9/static-elfs=8` 保持，`bash -n` 通过。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS 9/9、ABI 6/6、kernel 11/11。QEMU 多槽 markers、checkpoint、endpoint/
memory-pool/DmaMap/background/devmgr/block/MFS/proc/file/shell 全链通过；resident
switches=16775，notification `blocking-waits=2 irq-acks=642 sgi-wakeups=640`，
ticks=`[1867,1860]`、uptime=2980ms、RR20、INTx4、shutdown。Powercut token
`powercut-1786305232-222-27052`，first/second status=0，写入+sync 后 SIGTERM，重启
精确 token/MFS mount/shutdown；无非预期 fault/panic/timeout/translation/allocator。
fsck：`MFS1 clean generation=45 transactions=44 entries=5 used_blocks=89`。

### 独立 EL0 devmgr 与六服务槽回归（2026-08-10）

现有 smoke 更新为 bootfs `entries=10 static-elfs=9`、resident
`address-spaces=6 asids=[0x20..0x25] ready=6/6 online=6/6`，并要求 devmgr EL0
入口、root→devmgr 四 cap 委派及 devmgr→block 四 cap 转发。应用槽门禁更新为
counter PID7、spinner PID8（slot reuse）、privprobe PID9；新增
`pids=[7,8,9] independent-asids=true`，最终 ps 三行状态独立匹配。`bash -n` 通过。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS 9/9、ABI 6/6、kernel 11/11。QEMU devmgr/block/MFS/checkpoint/recovery/
fsync/background/proc/file/shell 全链通过；notification `blocking-waits=6 irq-acks=744
sgi-wakeups=744`，resident switches=14874，ticks=`[1879,1872]`、uptime=2975ms、RR20、
INTx4、shutdown。Powercut token `powercut-1786315204-510-14421`，first/second
status=0，write+sync 后重启精确读回 token/MFS mount/shutdown；无非预期
fault/panic/timeout/translation/allocator。fsck：`MFS1 clean generation=50 transactions=49
entries=5 used_blocks=99`。

### 最终格式与脚本语法门禁（2026-08-10）

使用 Rust `1.97.1` / rustfmt `1.9.0` 格式化三个测试文件；
`cargo fmt --all -- --check` 与 `bash -n scripts/*.sh` 全部通过。本轮未运行测试，
未修改语义。

### 独立 devmgr modern transport 验证门禁（2026-08-10）

现有 smoke 增加 `[devmgr] resident EL0 verified modern transport
features-ok=true queues-positive=true capacity-positive=true`，保护 devmgr 实际
MMIO 映射/读取与现代 VirtIO 配置，而非仅验证 capability 转发。`bash -n` 通过。

OrbStack context=`orbstack`，`make build` exit 0（最终 image
`sha256:a66cdd249718702ada0f3988ac1b6eea0f24bf5fc5775802fff17c97c99b8c02`）；`make
test` exit 0（normal smoke + powercut，MFS 9/9、ABI 6/6、kernel 11/11），`make fsck`
exit 0。QEMU bootfs10/9、resident6/6、devmgr modern marker、四-cap 委派/转发、
DmaMap/background/block/MFS/proc/file/shell 全链通过；notification
`blocking-waits=3 irq-acks=802 sgi-wakeups=805`，switches=27739，ticks=`[1929,1927]`、
uptime=2980ms、RR20、INTx4，最终 shutdown。Final ps PID7 counter0/PID8 spinner-15/
PID9 privprobe-8，devmgr PID6 blocked。Powercut token
`powercut-1786315579-225-9900`，first/second status=0，sync 后精确 token/MFS mount/
shutdown；无非预期 fault/panic/timeout/translation/allocator。fsck：`MFS1 clean
generation=55 transactions=54 entries=5 used_blocks=109`。

### Devmgr PCI ECAM/function 验证门禁首错（2026-08-10）

smoke 新增并命中 devmgr PCI ECAM marker：
`[devmgr] resident EL0 verified PCI function vendor=1af4 device=1042 command=memory+bus-master caps=common+notify+isr+device`；此前 modern transport
features/queues/capacity marker 也命中，证明实际 ECAM 读取与命令位/capability 校验路径
执行。`bash -n` 通过。

OrbStack `make build` exit 0；`make test` 在 normal smoke 首错 exit 1，Host MFS 9/9、
ABI 6/6、kernel 11/11 全绿。日志 `target/smoke-qemu.log` 的唯一门禁失败为：

```text
smoke-qemu: resident notification counters must all be positive: blocking-waits=0 irq-acks=851 sgi-wakeups=849
```

其余 bootfs10/9、resident6/6、PID7/8/9 槽、DmaMap/devmgr/PCI/block/MFS/proc/file/shell
链已到达 shutdown，无 panic/timeout/translation/allocator fault。因 normal smoke 首错，
按要求未运行 powercut 或 fsck；未修改生产代码。

### PCI ECAM 与 NotificationWait 50ms 修复后严格回归（2026-08-10）

生产将 generic NotificationWait deadline 提升至 50ms；未新增或修改测试。OrbStack
`make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS 9/9、ABI 6/6、kernel 11/11。两个 devmgr marker 均通过：modern
features/queues/capacity，以及 PCI function `vendor=1af4 device=1042 command=memory+bus-master
caps=common+notify+isr+device`。Bootfs10/9、resident6/6、PID7/8/9 槽、DmaMap、
delegated/forwarded caps、block/MFS/proc/file/shell 全链通过；notification
`blocking-waits=8 irq-acks=910 sgi-wakeups=909`，switches=41675，ticks=`[1848,1844]`、
uptime=2977ms、RR20、INTx4、shutdown。Powercut token
`powercut-1786316011-226-26919`，first/second status=0，write+sync 后精确 token/MFS
mount/shutdown；无非预期 fault/panic/timeout/translation/allocator。fsck：`MFS1 clean
generation=62 transactions=61 entries=5 used_blocks=123`。

### DmaMap PTE/TLBI 授权回归（2026-08-10）

现有 smoke 的 DmaMap 门禁扩展为
`[iommu] resident block DmaMap capability=true pte=true tlbi=true`，保护 queue PTE
实际授权、TLBI 失效缓存及 EL0 queue 可用性；未新增单测。`bash -n` 通过。

OrbStack context=`orbstack`；`make build`、`make test`（host MFS 9/9、ABI 6/6、kernel
11/11、normal smoke + powercut）、`make fsck` 全部 exit 0。QEMU DmaMap marker、VirtIO
queue read/write/flush、SMMU fault-probe `blocked=true sentinel=true event=0x10`、devmgr
modern/PCI 两 marker、resident6/6、bootfs10/9、MFS checkpoint/recovery/fsync、PID7/8/9
槽及 shutdown 全通过；notification `blocking-waits=8 irq-acks=1003 sgi-wakeups=1002`，
switches=42326。Powercut token `powercut-1786316436-226-5643`，first/second status=0，
sync 后精确恢复 token/MFS mount/shutdown；无非预期 fault/panic/timeout/translation/
allocator。fsck：`MFS1 clean generation=67 transactions=66 entries=5 used_blocks=133`。

### EL1 probe reset / EL0 ownership 首错（2026-08-10）

smoke 已移除旧 devmgr `verified` 文本并加入 boot probe reset、EL0 negotiated modern
transport、configured PCI function 三条门禁；`bash -n` 通过。OrbStack `make build`
exit 0；Host MFS 9/9、ABI 6/6、kernel 11/11 全绿。唯一一次 `make test` 在 normal
smoke 首错 exit 1，未启动 powercut，也未执行 fsck。

新 VirtIO/devmgr markers 均命中。随后首错为：

```text
[service] critical service block exited status=9
```

status=9 对应 block queue probe：devmgr 协商后的 common status 仅为
ACK|DRIVER|FEATURES_OK (`0x0b`)，未设置 DRIVER_OK bit 2；`reset_for_user` 先清零
status，故 block probe 失败。尚未出现用户 queue read/write、MFS/IPC/shell/shutdown；
无 panic/timeout/translation/allocator fault。未修改生产代码，按首错停止。

### EL0 split queue 配置回归（2026-08-10）

smoke 新增 `[user] block configured split queue0 size=16 driver-ok=true` 门禁，保护
reset 后由 EL0 block 重新选择 queue0、配置 descriptor/avail/used IOVA、清 ring index、
enable queue 并最后设置 DRIVER_OK 的顺序；`bash -n` 通过。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。正常串口顺序为 boot probe
reset → devmgr modern negotiation/PCI configuration → DmaMap `pte=true tlbi=true` →
EL0 split queue marker → block read/write/flush；SMMU fault-probe
`blocked=true sentinel=true event=0x10`、INTx completions=4、notification
`blocking-waits=13 irq-acks=1067 sgi-wakeups=1070`、resident `ready=6/6 online=6/6`
及 MFS/proc/file/shell 全链通过，最终 shutdown。Powercut token
`powercut-1786317119-228-20099`，first/second status=0，sync 后 SIGTERM，重启精确恢复
token 并 shutdown。fsck：`MFS1 clean generation=72 transactions=71 entries=5 used_blocks=143`。
无非预期 panic/timeout/translation/allocator fault。

### SMMUv3 CMDQ TLBI/SYNC 回归（2026-08-10）

smoke 将 SMMUv3 domain 门禁扩展为同一行内匹配 `stream-id=0x10 iova=0x100000` 与
`cmdq=true`，并将双页 DmaMap marker 更新为
`frames=2 capability=true pte=true cmdq=true unmap-remap=true`。首次验证仅因串口
中 `idr/gerror` 位于 `cmdq=true` 前导致正则过窄，已修正为行内顺序无关匹配；`bash -n`
通过，未改生产。

修正后 OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck`
全部 exit 0。Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。SMMU domain
输出含 `gerror=0x0 cmdq=true`；双页 DmaMap、MemoryPool mapped-delete/reclaim、split
queue/I/O、SMMU fault-probe blocked、INTx=4、notification `blocking-waits=12
irq-acks=1370 sgi-wakeups=1367`、resident 6/6 及完整服务链均通过。Powercut token
`powercut-1786318742-228-12492`，first/second status=0，重启恢复 token 并 shutdown；
fsck：`MFS1 clean generation=89 transactions=88 entries=5 used_blocks=177`。无 CMDQ
fault、非零 GERROR、translation/panic/timeout/allocator fault。

### 两页 DMA capability map/unmap/remap 回归（2026-08-10）

smoke 将 DmaMap 门禁升级为
`[iommu] resident block DmaMap frames=2 capability=true pte=true tlbi=true unmap-remap=true`，
保护 queue@0x100000 与 data@0x101000 两页的 capability 授权、逐页 PTE/TLBI、data
unmap 后 NotFound 以及 remap 后 I/O；`bash -n` 通过。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。双页 marker、MemoryPool
`mapped-delete=busy reclaimed=true`、split queue、VirtIO read/write/flush、SMMU
fault-probe blocked、INTx=4、notification `blocking-waits=23 irq-acks=1220 sgi-wakeups=1219`、
resident 6/6 及完整 MFS/proc/file/shell 链均通过。Powercut token
`powercut-1786318269-228-10179` 两次 status=0，重启恢复 token 并 shutdown；fsck：
`MFS1 clean generation=82 transactions=81 entries=5 used_blocks=163`。无非预期
panic/timeout/translation/allocator fault。

### MemoryPool/Frame mapped-delete 与 quota 回收回归（2026-08-10）

smoke 将 memory-pool 门禁扩展为
`[mmu] resident memory-pool quota=4 frame-map/unmap remap=true mapped-delete=busy reclaimed=true`，
覆盖映射中删除返回 Busy、解除映射后删除，以及重新分配证明 runtime bitmap/quota 回收；
`bash -n` 通过，未新增单测。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。顺序 reset → devmgr
negotiate/configure → DmaMap `pte=true tlbi=true` → split queue `size=16 driver-ok=true`
→ queue read/write/flush 完整；SMMU fault-probe blocked、INTx completions=4、notification
`blocking-waits=2 irq-acks=1170 sgi-wakeups=1168`、resident 6/6、MFS/proc/file/shell
及 shutdown 全部通过。Powercut token `powercut-1786317915-359-20859`，first/second
status=0，重启恢复成功；fsck：`MFS1 clean generation=77 transactions=76 entries=5
used_blocks=153`。无非预期 panic/timeout/translation/allocator fault。

### 动态授权 Frame DMA 回归（2026-08-10）

smoke 新增 `[iommu] resident block dynamic-frame iova=0x102000 read=true reclaimed=true`
门禁，保护 block 通过 MemoryPool authority 创建临时 Frame、动态 IOVA 映射、VirtIO
sector 0 MFS1 读取，以及 DmaUnmap/FrameUnmap/CapDelete 后回收；`bash -n` 通过。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。双页 DmaMap/CMDQ、动态 Frame
marker、MemoryPool mapped-delete/reclaim、split queue/I/O、SMMU `cmdq=true gerror=0`
与越界 blocked probe、INTx=4、notification `blocking-waits=4 irq-acks=1421
sgi-wakeups=1422`、resident 6/6 及完整服务链均通过。Powercut token
`powercut-1786319092-421-30706`，first/second status=0，重启恢复 token 并 shutdown；
fsck：`MFS1 clean generation=94 transactions=93 entries=5 used_blocks=187`。无非预期
panic/timeout/translation/CMDQ/GERROR/allocator fault。

### EL0 idle 可运行退路回归（2026-08-10）

smoke 更新 bootfs 门禁为 `entries=11 static-elfs=10`，并新增
`[sched] EL0 idle thread entered`，保护普通任务全部阻塞时仍有可运行 idle，不依赖
Busy 作为无 runnable 退路；`bash -n` 通过。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。Idle marker、dynamic Frame、
DmaMap/CMDQ/SMMU blocked、MemoryPool、split queue/I/O、devmgr、MFS/proc/file/shell
完整链均通过；resident `ready=6/6 online=6/6 switches=3526`，notification
`blocking-waits=1527 irq-acks=1537 sgi-wakeups=1537`，INTx=4，最终 shutdown。Powercut
token `powercut-1786319505-230-4801`，first/second status=0，重启恢复 token 并
shutdown；fsck：`MFS1 clean generation=99 transactions=98 entries=5 used_blocks=197`。
无非预期 panic/timeout/translation/CMDQ/GERROR/allocator fault。未覆盖边界仍为真实硬件
SMMU 多 stream、多队列 VirtIO 及 idle 长时间压力，而非本次 idle 可运行性门禁。

### 双核 resident 共享 ready-queue 回归（2026-08-10）

smoke 新增并解析 `[sched] resident shared-ready-queue cpus=2 ticket-lock=true idle-tasks=2
switches=[A,B]`，要求两核 switches 均大于零；保留 EL0 idle marker。`bash -n` 通过，
未新增 unit test。

OrbStack `make build`、`make test`（normal smoke + powercut）、`make fsck` 全部 exit 0。
Host：MFS recovery 9/9、ABI 6/6、kernel core 11/11。正常日志共享队列
`switches=[1,7707]`，powercut 第二次启动 `switches=[1,8118]`；两核 timer/ps ticks、
RR20、resident ready=6/6、INTx=4、notification `blocking-waits=1605 irq-acks=1621
sgi-wakeups=1621`、idle/DMA/CMDQ/SMMU/MFS/proc/file/shell 全链通过。Powercut token
`powercut-1786320033-236-20849`，两次 status=0，token 恢复并 shutdown；fsck：
`MFS1 clean generation=104 transactions=103 entries=5 used_blocks=207`。无 deadlock、
panic/timeout/translation/CMDQ/GERROR/allocator fault；现有 ps CPU 断言未冲突。

### 双核共享调度严格 switches/non-idle 与 ps CPU 回归（2026-08-10）

smoke 将共享 marker 收紧为
`switches=[A,B] non-idle=[C,D]`，解析并要求 A/B≥20、C/D>0；同时要求 ps 命中
`PID 5 shell running cpu=[01]` 与 resident 服务 `cpu=shared`。`bash -n` 通过。

按限定流程仅执行 OrbStack `make build`（exit 0）及一次容器内 smoke（PASS），未重复
host logic、powercut 或 fsck。共享队列输出 `switches=[364,500066] non-idle=[1,498347]`；
ps 首次 shell 在 CPU1、后续在 CPU0，PID1-4/6 resident 服务均报告 `cpu=shared`，双核
ticks 正常。Idle、RR20、resident ready=6/6、notification `1694/1701/1701`、INTx=4、
MFS/proc/file/shell/shutdown 全过；无 panic/timeout/translation/CMDQ/GERROR/allocator
fault。完整 host/powercut/fsck 绿色基线沿用上一节（generation=104）。

### Bootfs 名称加载与应用槽复用聚焦回归（2026-08-10）

smoke 输入在原 counter 前加入 `run privprobe`，随后再运行 counter，并追加
`run missing` 负向契约；门禁覆盖名称 FNV64 转发、ELF/W^X 重新校验、Exited 槽复用及
未找到名称不影响系统继续运行。`bash -n` 通过。

按限定流程仅执行 OrbStack `make build`（exit 0）及一次容器内 smoke（PASS），未重复
host logic、powercut 或 fsck。串口顺序：`run: privprobe pid=7` → name-based loader
marker/static ELF → privprobe pid7 entered/fault/wait=-8；随后 `run: counter pid=7`
复用同一槽，name-based loader → started/completed/wait=0；`run missing` 返回
`not found`，系统继续并最终 ps 显示 PID7 counter exit0、PID8 spinner -15、PID9
privprobe -8。共享队列 `switches=[163,279061] non-idle=[1,277317]`、RR20、idle、
resident6/6、notification `1713/1729/1730`、INTx4、MFS/proc/file/shell/shutdown 全过；
无 panic/timeout/translation/CMDQ/GERROR/allocator fault。

### 最终统一 OrbStack 验收（2026-08-10）

`bash -n scripts/smoke-qemu.sh` 通过；正式 smoke 已包含 shared queue A/B≥20、
non-idle C/D>0、shell CPU 迁移与 resident `cpu=shared`、PID7 privprobe→counter
name-based loader/TLBI 槽复用及 `run missing` NotFound 门禁。OrbStack `make build`、
`make test`（Host MFS recovery 9/9、ABI 6/6、kernel core 11/11；normal smoke +
powercut）、`make fsck` 全部 exit 0。

正常 smoke shared `switches=[170,282536] non-idle=[1,280760]`，ps PID1–4 resident
`cpu=shared`，shell CPU1，ticks `[333,328]→[2394,2387]`；PID7 privprobe fault/wait=-8
后复用为 counter/wait=0，missing 返回 not found。Idle、RR20、resident6/6、notification
`1758/1766/1766`、INTx4、完整 MFS/proc/file/shell 链及 shutdown 全过。Powercut token
`powercut-1786320989-254-7892`，两次 bootfs entries=11/static-elfs=10、shared marker
（first `[198,296182]`，second `[169,293818]`）及 runtime 恢复/shutdown 均通过。fsck：
`MFS1 clean generation=113 transactions=112 entries=5 used_blocks=225`。无非预期
panic/timeout/translation/CMDQ/GERROR/allocator fault；仅预期 privileged isolation fault。

### 正常 VirtIO 数据面 EL0 委派聚焦回归（2026-08-10）

smoke 移除旧 EL1 sector0 read/write/flush 门禁，改为
`[virtio] EL1 isolation queue armed; block data I/O delegated to EL0`，并保留 EL0
block read/write/flush、dynamic Frame、INTx 与 SMMU blocked 门禁；`bash -n` 通过。

按限定流程仅执行 OrbStack `make build`（exit 0）及一次容器内 smoke（PASS），未重复
host logic、powercut 或 fsck。串口顺序为 EL1 isolation queue armed → SMMU blocked/
INTx → boot reset/devmgr negotiate → DmaMap/dynamic-frame → EL0 block queue
read/write/flush；shared `switches=[167,298014] non-idle=[1,296155]`、resident6/6、
notification `1836/1844/1844`、RR20、MFS/IPC/shell/shutdown 全过。无 panic/timeout/
translation/CMDQ/GERROR/allocator fault；仅预期 isolation fault。完整 gen113
build/test/fsck 绿色基线沿用上一节。

### MFS1 段尾 Padding/CRC 回归（2026-08-10）

在现有 `crates/mfs1/tests/recovery.rs` 扩展集成测试：用接近 1MiB 的多块记录将
head 推至段尾，校验 raw MemDisk 上 Padding(kind=6) 从旧 head 填充至 256-block
边界，下一条真实记录从下一段起始且不跨段；随后 remount/read/check 验证恢复。再破坏
padding CRC，确认最新 superblock 被拒绝并回退上一代，不暴露 `/tail` 混合事务。首次
构造误占满 256 blocks，调整为 250-block 数据记录+commit 后通过；这是测试夹具修正，
非生产失败。

OrbStack 仅运行 `cargo test -p mfs1 --tests`：unit 0/0，recovery 10/10 passed、0
failed，命令 exit 0。未运行 QEMU、powercut 或 fsck。本测试未覆盖真实块设备断电时序与
多段 GC 搬迁（已有既有 recovery/GC 测试覆盖逻辑契约）。

### MFS1 多 extent 大文件回归（2026-08-10）

在现有 recovery 测试中加入 2MiB+12,345 尾巴的确定性文件，覆盖同一事务拆成多个
SEGMENT_BLOCKS 内 extent；立即 read、remount/read/check，再 GC 后 remount/read/check。
raw MemDisk 额外确认第二 extent 为 Put 且 `logical_offset>0`，保护旧实现的 NoSpace、
覆盖前一 extent 或 compaction 截断风险。

OrbStack 仅运行 `cargo test -p mfs1 --tests`：unit 0/0，recovery 11/11 passed、0
failed（含 `multi_extent_put_roundtrips_through_remount_and_gc`），命令 exit 0。未运行
QEMU、powercut 或 fsck。未覆盖极端多文件事务的 extent 交错与真实断电窗口，现有事务
故障计划仍覆盖提交原子性。

### MFS1 旧 v1 features=0 兼容迁移回归（2026-08-10）

在现有 recovery.rs 增加兼容测试：提交文件后将双 superblock offset12 feature bits
清零并用公开 `crc32c` 重算 CRC，验证旧镜像 mount/read/check；随后执行 GC，确认当前
superblock feature bit0（segment-layout）置位，remount/read/check 仍成功。首次测试夹具
出现 Rust E0502 借用冲突，改为暂存 CRC 后通过，非生产失败。

OrbStack 仅运行 `cargo test -p mfs1 --tests`：unit 0/0，recovery 12/12 passed、0
failed，命令 exit 0（padding/CRC、多 extent 及旧 feature 兼容均通过）。未运行 QEMU、
powercut 或 fsck；未构造复杂旧跨段镜像，保留真实迁移边界最小化。

### MFS1 segment-directory GC/故障注入回归（2026-08-10）

按新 feature bit1 有序 segment directory 调整 recovery 夹具：legacy features=0 测试补
线性 `log_head`，GC 故障注入改指向新 segment，checkpoint 断言改为 raw directory；重复
覆盖改为 6 segments/500 事务，确保至少两个近满 segment，显式 GC 后 raw directory
移除低 live-ratio victim、保留最新 segment、free 增长且 remount/read/check 一致。
Padding、multi-extent、legacy 迁移与事务故障计划均保留。

OrbStack 仅运行 `cargo test -p mfs1 --tests`：unit 0/0，recovery 12/12 passed、0
failed，命令 exit 0。未运行 QEMU、powercut 或 fsck。GC 故障阶段覆盖 record 写入失败及
旧/新目录恢复；未覆盖每个 sealed-superblock flush 阶段的独立故障矩阵与超大目录上限。

### MFS1 GC 双 superblock 故障矩阵回归（2026-08-10）

扩展现有 GC 故障边界为六个停点：record write、data first flush、第一份
superblock write/flush、sealed 第二份 superblock write/flush。每个停点均要求 `gc()`
返回 `Io`，清除故障后在同一 FileSystem 提交 `/after`，再 remount 验证旧 live 内容与
新内容均存在且 `check()` 成功，保护保守 old+candidate directory 不复用仍可能被任一
superblock 引用的 segment。

OrbStack 仅运行 `cargo test -p mfs1 --tests`：unit 0/0，recovery 12/12 passed、0
failed，命令 exit 0；新增 `gc_failure_matrix_never_reuses_old_or_candidate_segments`
通过。未运行 QEMU、powercut 或 fsck；仍未覆盖超大 segment directory 上限及真实设备
断电窗口。

### Segment-directory 迁移统一验收首错（2026-08-10）

先执行 `bash -n scripts/*.sh`，通过；smoke 已加入可观测 marker
`[user] mfs1 segment-directory active-segments=N low-water=15 high-water=25`，并解析
要求 `N>0`，没有绑定动态 N 字面量。随后 OrbStack `make build` exit 0；`make test`
Host 阶段 MFS recovery 12/12、ABI 6/6、kernel core 11/11 全绿。

normal smoke 首错为：

```text
[service] resident EL0 loader failed
```

QEMU 随后 45s 超时并由 SIGTERM 结束，仅到 `root task ready`、`block transport ready`、
`mfs1 ready`，未进入 boot-proof 首次 fsync、segment-directory marker、shell、powercut
或 fsck；故按首错停止，未执行 powercut/fsck。早期 bootfs11/10、VirtIO/SMMU/CMDQ、
EL1 isolation、SMMU blocked、INTx、RR/IPC 全部已通过；无 panic/translation/CMDQ/GERROR/
allocator fault。现有 gen113 绿色基线保持，但本轮不能宣称旧 features0/1 镜像迁移成功，
需定位 resident loader failure 后再验收。

### Resident loader 修复后 segment-directory 聚焦 smoke（2026-08-10）

生产将 resident `IMAGE_BYTES` 从 64KiB 扩至 128KiB 后，按限定流程在 OrbStack
执行 `make build`（exit 0）及唯一一次 `scripts/smoke-qemu.sh`（exit 0）。本轮未重复
host tests、powercut 或 fsck。

smoke 通过并命中旧盘恢复链：bootfs `entries=11/static-elfs=10`，六个 resident ELF
进入 EL0，`ready=6/6 online=6/6`，共享队列 `switches=[168,296670]
non-idle=[1,294733]`，RR context-switches=20；VirtIO EL0 queue read/write/flush、
SMMU `cmdq=true` 且 fault probe `blocked=true`、INTx completions=1、notification
`blocking-waits=1912 irq-acks=1927 sgi-wakeups=1927` 均正常。

MFS 证据包括 `mounted via EL0 block IPC`、checkpoint valid、`recovered /boot-proof
after restart=true`、`fsync /boot-proof persistent=true`，以及新门禁
`[user] mfs1 segment-directory active-segments=1 low-water=15 high-water=25`；shell
文件链和 shutdown 均通过。PID7 privprobe fault/wait=-8 后复用为 counter并完成/wait=0。
无 panic、translation/CMDQ/GERROR、allocator 或 timeout（仅预期隔离 fault 与 deadline
探针）。

### 最终 segment-directory 统一门禁（2026-08-10）

OrbStack 唯一 `make test` exit 0：Host MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11；normal smoke PASS，segment-directory
`active-segments=1`，resident `ready=6/6 online=6/6`，notification
`blocking-waits=117 irq-acks=118 sgi-wakeups=118`，正常 shutdown。

Powercut PASS，token `powercut-1786325152-262-25231`。第一次写入并 `sync: ok` 后收到
SIGTERM，未出现 `[system] shutdown`；第二次启动成功挂载 MFS，`active-segments=1`，读取
精确 token 后正常 shutdown。日志分别为
`target/powercut-1786325152-262-25231-first.log` 与
`target/powercut-1786325152-262-25231-second.log`。

随后 `make fsck` exit 0：`MFS1 clean generation=123 transactions=122 entries=5
used_blocks=19`。未发现 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### 动态应用槽并发 smoke 设计（2026-08-10，待 OrbStack 验收）

按 `FIRST_APPLICATION_PID=7`、`MAX_APPLICATIONS=8` 调整 init 的集成自检：同时启动两个
同名 spinner，确认两个不同 PID 均 running；kill 第一实例并轮询到真实 Exited，确认第二实例
仍 running；再回收第二实例，使用 counter 复用第一个空槽并等待 status=0。自检 marker 为：

```text
[proc] duplicate spinner independent=true survivor-running=true first-status=-15 second-status=-15 reclaimed=true slot-reuse=true
[proc] counter slot-reuse=true wait-status=0
[proc] dynamic application capacity=8 first-pid=7 independent-slots=true
```

privprobe marker 改为动态 PID。smoke 不再锁死 PID 7/8/9：从 EL0 spinner 启动行解析至少两个
不同 PID，按匹配 PID 校验 privileged fault 与 `status=-8`/reclaimed 证据，并校验最终 ps 中
counter status=0 与 spinner status=-15 使用不同 PID；missing 仍要求 `not found`。本轮仅完成
`bash -n scripts/smoke-qemu.sh`、init rustfmt/check，尚未启动 OrbStack，等待主 agent 复核后统一
验收。

### badptr/pageprobe 隔离矩阵设计（2026-08-10，待 OrbStack 验收）

init 在既有动态槽自检后按 bootfs hash 依次启动 `badptr`、`pageprobe`、`privprobe`，分别
轮询 `status=0`、`status=-8`、`status=-8`，并通过动态 PID marker 关联结果。smoke 更新
bootfs 门禁为 `entries=13/static-elfs=12`，新增三条独立 probe 链：badptr 入口→非法指针
被拒绝且任务 status=0（其入口到 wait 窗口内不得出现 application fault）；pageprobe 入口→
同一 PID 的 `FAR=0x600000` application fault→reclaimed；privprobe 入口→同一 PID 的
特权指令 fault，并继续执行原 EC 校验。PID 可复用，解析按 entry/wait 行区间而非全日志 PID
否定，避免把后续 probe 的 fault 误归因给 badptr。完成静态 bash/rustfmt 检查，尚未启动
OrbStack。

### 动态应用槽最终统一验收（2026-08-10）

确认 Docker context 为 `orbstack`（镜像 `microsystem-dev:rust-1.97.1`，image id
`sha256:7ad3cecaa4c5479f21a80d5aaf085443f12a60d354854d65691bd3fbe2e62ff1`）。按首错顺序执行：

```text
make build  # exit 0
make test   # exit 0
make fsck   # exit 0
```

Host 计数为 MFS recovery 12/12、ABI 6/6、kernel core-boundaries 11/11。
normal smoke 日志 `target/smoke-qemu.log` 通过，动态应用 marker 为：

```text
[app] spinner pid=7 started at EL0
[app] spinner pid=8 started at EL0
[proc] duplicate spinner independent=true survivor-running=true first-status=-15 second-status=-15 reclaimed=true slot-reuse=true
[proc] counter slot-reuse=true wait-status=0
[app] privileged probe pid=7 entered EL0
[isolation] privileged instruction task pid=7 faulted status=-8 reclaimed=true
[proc] dynamic application capacity=8 first-pid=7 independent-slots=true
```

最终 shell `ps` 独立显示 `PID 7 counter exited status=0` 和
`PID 8 spinner exited status=-15`；MFS `active-segments=1`，recovery、checkpoint、
fsync、EL0 block I/O、file-chain 和 shutdown 均通过。Resident ready 为 6/6 online=6/6，
shared-ready-queue `switches=[333,452874] non-idle=[1,452674]`，notification
`blocking-waits=196 irq-acks=198 sgi-wakeups=198`，VirtIO capacity=131072 sectors、
INTx completions=1，SMMU `cmdq=true`/fault probe blocked=true。

Powercut PASS，token `powercut-1786327047-469-14487`，两次 status=0：
`target/powercut-1786327047-469-14487-first.log` 在写入并 sync 后收到 SIGTERM 且无
shutdown；`target/powercut-1786327047-469-14487-second.log` 挂载 MFS、读取精确 token、
报告 `active-segments=1` 后 shutdown。`make fsck` 输出：

```text
MFS1 clean generation=128 transactions=127 entries=5 used_blocks=29
```

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout；日志中的
isolation/privileged faults 与 deadline marker 均为预期测试路径。

### 隔离矩阵最终统一验收（2026-08-10）

确认 Docker context 为 `orbstack`；按首错顺序执行：

```text
make build  # exit 0
make test   # exit 0
make fsck   # exit 0
```

Host 计数为 MFS recovery 12/12、ABI 6/6、kernel core-boundaries 11/11。normal
smoke 日志 `target/smoke-qemu.log` 命中 bootfs `entries=13/static-elfs=12`，并保留
procman/MFS/Shell 链、动态槽、shared-ready queue 和 shutdown。

三类隔离 probe 均按同一动态 PID 关联且窗口顺序正确：

```text
[app] invalid-pointer probe pid=7 entered EL0
[isolation] invalid user pointer rejected status=-8 task-survived=true
[proc] invalid-pointer probe pid=7 wait-status=0 task-survived=true

[app] page-fault probe pid=7 entered EL0
[proc] application fault ESR=0x92000006 FAR=0x600000; pid=7 terminated
[proc] page-fault probe pid=7 wait-status=-8 reclaimed=true

[app] privileged probe pid=7 entered EL0
[proc] application fault ESR=0x2000000 FAR=0x600000; pid=7 terminated
[isolation] privileged instruction task pid=7 faulted status=-8 reclaimed=true
```

Invalid-pointer 的 entry→reject→wait 窗口没有 `application fault`；pageprobe 的
entry→same-PID FAR=0x600000 fault→wait=-8/reclaimed 和 privprobe 的
entry→same-PID EC=0/IL=1 fault→reclaimed 均通过。动态 app capacity=8，两个 spinner
PID 7/8 独立启动并完成 kill/reclaim，counter 槽复用 wait=0；最终 ps 为 PID 7 counter
status=0、PID 8 spinner status=-15。

Resident ready=6/6 online=6/6 switches=707，shared-ready queue
`switches=[180,271730] non-idle=[1,271447]`，notification
`blocking-waits=279 irq-acks=278 sgi-wakeups=278`；MFS segment-directory
`active-segments=1`，VirtIO capacity=131072/INTx completions=1，SMMU `cmdq=true`
且 fault probe blocked=true。

Powercut PASS，token `powercut-1786328504-316-10357`，两次 status=0：
`target/powercut-1786328504-316-10357-first.log` 写入并 sync 后 SIGTERM 且无 shutdown；
`target/powercut-1786328504-316-10357-second.log` 挂载 MFS、读取精确 token、报告
`active-segments=1` 后 shutdown。`make fsck` 输出：

```text
MFS1 clean generation=133 transactions=132 entries=5 used_blocks=39
```

未发现非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout；预期
isolation faults 与 deadline marker 除外。

### PCI ECAM/BAR 单次 smoke 首错（2026-08-10）

确认 Docker context 为 `orbstack`；按限定顺序执行 `make build`（exit 0）后仅运行
一次正式 `scripts/smoke-qemu.sh`，日志为 `target/bar-assignment-smoke-qemu.log`。
Smoke exit=1，故按首错策略未运行 `make test` 或 `make fsck`。

bootfs `entries=13/static-elfs=12`、EL0 init/六服务、RR/IPC、动态应用槽与三类 probe
均先出现；首个失败为：

```text
[proc] dynamic application capacity=8 first-pid=7 independent-slots=true
[service] critical service init exited status=16
xtask: command failed with exit status: 1
```

`status=16` 来自 init 的 `delegate_block_transport`，表示向 devmgr 的 BLOCK 配置
IPC 返回非零。因该委派在新增 ECAM/BAR 发现 marker 之前失败，以下新增 marker 及其后续
EL1/SMMU/INTx/EL0 queue 链尚未出现：

```text
[devmgr] resident EL0 discovered PCI ECAM function and assigned BARs
[virtio] validated EL0 BAR assignment queues=... capacity-sectors=... notify-multiplier=...
```

本轮未观察 panic/translation/CMDQ/GERROR/allocator fault 或 timeout；停止点为 devmgr
委派返回非零，不能据此宣称 BAR 分配或后续链路通过。

### EL0 PCI/VirtIO BAR 所有权迁移 smoke 首错（2026-08-10）

ABI 测试已补充 `Syscall::SystemControl=22` 与
`boot_cap::DEVICE_SYSTEM_CONTROL == CapHandle::from_parts(19, 1)`；smoke 移除了旧
EL1 device/modern/reset/configuration 断言，新增并解析 EL0 devmgr 发现 ECAM/BAR 与
`[virtio] validated EL0 BAR assignment queues=N capacity-sectors=N notify-multiplier=N`
（N 均要求大于零）。`bash -n scripts/smoke-qemu.sh` 与 rustfmt check 通过。

OrbStack `context=orbstack` 下 `make build` exit 0；唯一 smoke
`target/bar-assignment-smoke-qemu.log` exit 1，按首错未执行完整 `make test` 或 `make fsck`。
bootfs13/12、resident services、RR/IPC、动态三 probe 均先通过，随后首个 guest 错误为：

```text
[service] critical service init exited status=16
```

该状态来自 init 的 `delegate_block_transport`：DEVMGR IPC 返回非零，因此尚未出现新
ECAM/BAR、validated transport、EL1/SMMU/INTx/EL0 queue marker；无 panic、timeout、
translation/CMDQ/GERROR 或 allocator fault。需定位生产 devmgr IPC status=16 后再继续完整
门禁。

补充只读链路证据（GUI 停止后执行，非绕过验证）：`openssl s_client` 的
`example.com` leaf SHA-256 为 `66fcf762…`，签发/自签 Gateway CA SHA-256 为
`FE1B1D6824EEB49DE0A59B3F19BDB73BCFFE2AA16D8CFD865C4FB78EE88500C1`，与
derpack 中第 164 条完全相同；宿主 OpenSSL `Verify return code: 0 (ok)`。
因此当前 InvalidCertificate 可归因于 guest `embedded_tls` verifier/链处理，
而非缺失 Cloudflare Gateway 根。

### Browser TLS verifier-stage diagnostic (2026-08-14)

按新增有界诊断 marker 重跑：`rustfmt --check`（`xtask/src/main.rs`、
`services/mica/src/tls.rs`）与 OrbStack `cargo check -p xtask` 均通过；fresh
`make build`（`target/browser-internet-ca-build-diag.log`）完成并注入完整
bundle hash `70572323…6d50`。唯一 GUI profile（5903/2224）Reader 点击后，
串口先出现 ARP first TX/RX，随后首个 TLS 阶段错误精确为：
`[tls] presented-root validation error=DecodeError`。该 marker 表明服务端
提供的 Gateway 根已被选中但 guest 证书链解析/验证返回 DecodeError；未到
CertificateVerify，页面仍未产生 HTTP 200/`mica-reader loaded`。QMP 截图
`target/browser-internet-reader-diag.ppm`（1024x768，PNG 同名 `.png`）保留，
内容与 `TLS handshake validation failed: InvalidCertificate` 一致。

严格首错后 Ctrl-C 停止 QEMU，5903/2224、QMP/serial FIFO、qemu/xtask 进程和
OrbStack 容器均清理；本轮不运行 `make test`/`make fsck`。

### Browser TLS root-tail fix validation (2026-08-14)

根尾证书修正后，聚焦 rustfmt（xtask 与 `services/mica/src/tls.rs`）、OrbStack
`cargo check -p xtask` 均通过；fresh build 日志
`target/browser-internet-ca-build-rootfix.log` exit **0**，仍注入最终 bundle
hash `70572323…6d50`，CA marker `pinned=121 host-extra=43 bytes=173925`。
唯一 GUI profile（5903/2224）Reader 点击后无 `presented-root` 错误，说明
Gateway 根/中间链已通过；随后首个错误为
`[tls] handshake-signature validation error=DecodeError`。未产生 HTTP 200 或
`mica-reader loaded`，QMP 截图 `target/browser-internet-reader-rootfix.ppm`
（1024x768，PNG 同名 `.png`）保留；无 panic/DMA fault。按首错停止，未运行
`make test`/`make fsck`、未关闭 TLS 校验。

Ctrl-C 后 GUI/QEMU 正常清理，5903/2224、QMP/serial FIFO、进程与 OrbStack
容器均无残留。

### Browser TLS P-384 retry (2026-08-14)

P-384 代码格式化后 focused rustfmt（xtask 与 Mica TLS）及 OrbStack
`cargo check -p xtask` 均通过；fresh build
`target/browser-internet-ca-build-p384-final.log` 完成，CA marker
`pinned=121 host-extra=43 bytes=173925`、Mica service 注入 full hash
`70572323…6d50`。唯一真实 GUI Reader 尝试首错发生在启动层：点击后串口为
`[gui] desktop launch application=4 accepted=false`，没有 Mica registration、
HTTP 或 TLS marker；QMP 截图 `target/browser-internet-reader-p384.ppm`
（1024x768，PNG 同名 `.png`）保留。按首错停止，未重试、未运行统一门禁；
Ctrl-C 后 5903/2224、QMP/serial FIFO、QEMU/xtask 与容器均清理。

### Browser TLS P-384 verifier preflight (2026-08-14)

本轮在 build 前严格首错：`xtask/src/main.rs` focused rustfmt **PASS**，但
`services/mica/src/tls.rs` focused rustfmt **FAIL**（P-384 新路径 import 顺序及
`Signature::from_der` 长行）。按策略未执行 fresh build 或 GUI；待生产格式化后
再继续验证。

### PCI ECAM/BAR activation diagnostic smoke（2026-08-10）

确认 Docker context 为 `orbstack`；重新执行 `make build`（exit 0）后仅运行一次正式
smoke，日志 `target/bar-diagnostic-smoke-qemu.log`，exit=1。串口在动态应用自检后首错
停止：

```text
[proc] dynamic application capacity=8 first-pid=7 independent-slots=true
[service] critical service init exited status=16
xtask: command failed with exit status: 1
```

本轮没有出现 `[devmgr] resident EL0 staged PCI ECAM discovery and BAR assignment`、
`[device] EL0 devmgr requested activation ...` 或
`[device] EL0 BAR validation failed: ...`，也未进入 validated transport、SMMU、INTx
或 EL0 queue 阶段；init 的 BLOCK 配置委派在 activation 诊断前返回非零。按首错策略未
运行完整 `make test` 或 `make fsck`。

### PCI ECAM/BAR activation smoke recovered（2026-08-10）

在后续生产修复后确认 Docker context 为 `orbstack`，执行 `make build` exit 0，随后仅
运行一次正式 smoke，日志 `target/bar-diagnostic2-smoke-qemu.log`，exit=0（PASS）。
新增 activation/validation 阶段完整出现：

```text
[devmgr] resident EL0 staged PCI ECAM discovery and BAR assignment
[device] EL0 devmgr requested activation for 00:02.0
[virtio] validated EL0 BAR assignment queues=2 capacity-sectors=131072 notify-multiplier=4
[devmgr] resident EL0 discovered PCI ECAM function and assigned BARs
```

后续 SMMU `cmdq=true`/fault-probe blocked=true、INTx `completions=1`、EL0 split queue
read/write/flush、MFS mount/recovery/checkpoint/fsync/segment-directory、resident
ready=6/6 online=6/6、notification `350/357/356`、shared-ready queue
`switches=[175,263095] non-idle=[1,262733]`、shell file-chain 与 shutdown 均通过。
本轮按限定要求未运行完整 `make test` 或 `make fsck`；未观察非预期 panic、translation、
CMDQ/GERROR、allocator fault 或 timeout。

### 最终统一门禁首错：INTx completion 计数（2026-08-10）

`bash -n scripts/*.sh` 通过；容器 Rust 1.97.1 缺少 `cargo-fmt`，但主机
`cargo fmt --all -- --check`（rustfmt 1.9.0）exit 0。随后 OrbStack `make test` 的
Host 阶段通过（MFS 12/12、ABI 6/6、kernel 11/11），normal smoke 在完整 ECAM/BAR
所有权和 MFS 链之后首错：

```text
[device] EL0 devmgr requested activation for 00:02.0
[virtio] validated EL0 BAR assignment queues=2 capacity-sectors=131072 notify-multiplier=4
[iommu] fault-probe blocked=true sentinel=true ...
[irq] virtio-blk INTx completions=0
smoke-qemu: VirtIO INTx completion count is not positive
```

其余 devmgr/BAR、SMMU、EL0 split queue、三隔离 probe、动态应用、MFS/procman/Shell 和
shutdown markers 均已出现；`make test` exit 1，按首错策略未执行 powercut 或 fsck。本轮
未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout（仅预期 deadline
marker）。

### 最终统一门禁恢复（2026-08-10）

`bash -n scripts/*.sh` 与主机 `cargo fmt --all -- --check`（rustfmt 1.9.0）均通过；
Docker context=`orbstack`。OrbStack `make test` exit 0：Host MFS recovery 12/12、ABI
6/6、kernel core-boundaries 11/11；normal smoke PASS，`INTx completions=2`，powercut
PASS（token `powercut-1786330581-319-27356`，first/second status=0）。随后 `make fsck`
exit 0：

```text
MFS1 clean generation=142 transactions=141 entries=5 used_blocks=57
```

Normal smoke 日志 `target/smoke-qemu.log` 命中所有权顺序：staged ECAM → activation
`00:02.0` → validated BAR `queues=2 capacity-sectors=131072 notify-multiplier=4` →
EL0 devmgr discovery/negotiation/configuration → SMMU `cmdq=true`, fault blocked →
EL0 split queue。`INTx completions=2` 位于 queue 配置之后；随后真实 EL0 block
`read+write sector=0 mfs1=true flush=ok` 与 notification bitset、MFS mount/recovery/
checkpoint/fsync/segment-directory 均通过。注意该单次 completion 行在原始串口中位于
真实 block I/O 行之前（行 79 对比 81/82），脚本仅要求其正数，不强制文本顺序。

三隔离 probe 同 PID 7、动态 spinner/counter 槽复用、procman、Shell file-chain、shared
ready queue `switches=[168,264730] non-idle=[1,264288]`、resident ready=6/6 online=6/6、
notification `421/422/422`、shutdown 全通过。Powercut 首次 SIGTERM 无 shutdown，二次
挂载 MFS、恢复精确 token、正常 shutdown；日志为
`target/powercut-1786330581-319-27356-first.log` 与 `...-second.log`。无非预期
panic/translation/CMDQ/GERROR/allocator fault 或 timeout（仅预期 deadline marker）。

### Root manifest + proc name-loader smoke PASS（2026-08-10）

Docker context=`orbstack`。按本轮限定执行 `make build` exit=0，随后唯一正式 smoke
`target/root-manifest-proc-smoke-qemu.log` exit=0（PASS）。串口完整命中：

```text
[bootfs] root task started manifest services=devmgr,console,block,mfs,shell
[service] resident EL0 ready=6/6 online=6/6 switches=1288
[proc] bootfs name-based loader program=privprobe pid=7 static-elf=true
wait: pid=7 status=-8
[proc] bootfs name-based loader program=counter pid=7 static-elf=true
wait: pid=7 status=0
```

六服务 EL0 entered（init/devmgr/console/block/mfs/shell）、spinner 双实例与槽复用、
privprobe/page-fault/invalid-pointer 隔离、ECAM/BAR validated、SMMU `cmdq=true` 与
fault-probe blocked、INTx `completions=2`、EL0 queue read/write/flush、MFS mount/
recovery/checkpoint/fsync/segment-directory、procman、Shell file-chain、shared-ready
queue 与 shutdown 均通过。无非预期 panic、translation/CMDQ/GERROR、allocator fault 或
timeout（仅预期 deadline marker）；本轮未运行完整 `make test`、powercut 或 `make fsck`。

### Root task manifest smoke 首错（2026-08-10）

Docker context=`orbstack`；按本轮限定先执行 `make build`，exit=0，随后仅运行一次正式
smoke，日志为 `target/root-manifest-smoke-qemu.log`。新增 root manifest 与六个服务的
EL0 进入/ready/online 链均出现：

```text
[bootfs] root task started manifest services=devmgr,console,block,mfs,shell
[service] resident EL0 ready=6/6 online=6/6 switches=1220
```

staged ECAM/BAR → devmgr activation/validated BAR（queues=2、capacity=131072、notify=4）→
SMMU CMDQ/fault-probe blocked → INTx `completions=2` → EL0 block queue I/O、MFS
mount/recovery/checkpoint/fsync/segment-directory、procman/isolation、Shell 与
`[system] shutdown` 均在串口中出现；无非预期 panic、translation/CMDQ/GERROR、allocator
fault 或 timeout（仅预期 deadline marker）。

smoke exit=1 的唯一门禁是 proc counter name-loader 行被 UART 并发输出拆开，未形成脚本
要求的独立行：

```text
[proc] bootfs name-based loader [app] counter pid=7 started at EL0
program=counter pid=7 static-elf=true
```

因此本轮按首错策略未运行完整 `make test`、powercut 或 `make fsck`；该退出不是 guest
fault/halt。

### Root manifest 最终统一门禁（2026-08-10）

`bash -n scripts/*.sh` 与主机 `cargo fmt --all -- --check` 均通过；Docker context=
`orbstack`。OrbStack `make test` exit=0：MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11；normal smoke PASS，`round-robin context-switches=20`、shared
ready queue `switches=[509,718253] non-idle=[1,717682]`、INTx `completions=2`、resident
`ready=6/6 online=6/6 switches=1367`、notification `blocking-waits=562 irq-acks=566
sgi-wakeups=577`、MFS segment-directory active-segments=1、file-chain 均通过。

Normal 串口命中 root manifest、六 service EL0 entered/ready/online、devmgr/console →
block → mfs → shell、PCI/BAR/SMMU CMDQ 与 blocked fault probe、EL0 queue read/write/flush、
spinner 槽复用、三隔离 probe、procman；`run privprobe` 后 loader/wait=`pid=7 status=-8`，
`run counter` 后 loader/wait=`pid=7 status=0`，最终 ps 与 shell shutdown 均通过。

Powercut PASS：token=`powercut-1786332212-320-2724`，first/second status=0；首次日志
仅包含 write/sync 后的 powercut 输入且无 shutdown，二次日志恢复 `/boot-proof`、MFS
mount/segment marker、精确 token 后正常 shutdown。随后 `make fsck` exit=0：

```text
MFS1 clean generation=151 transactions=150 entries=5 used_blocks=75
```

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout（仅预期
deadline marker）。

### Dynamic MemoryPool cross-task move 最终验收（2026-08-10）

本轮测试侧仅扩展现有 `scripts/smoke-qemu.sh` required marker，未新增测试文件或改动
其他生产代码；静态 `bash -n scripts/*.sh` 与
`cargo fmt --all -- --check` 均通过。

OrbStack context=`orbstack`。`make build` exit=0；`make test` exit=0（MFS recovery
12/12、ABI 6/6、kernel core-boundaries 11/11，normal smoke + powercut 全绿）。
Normal `target/smoke-qemu.log` 原始证据：

    [mm] MemoryPool dynamic quota=2 third=NoMemory live-delete=Busy reclaimed=true registry-reused=true cross-task-move=true
    [mm] MemoryPool independent pools=2 quotas=[4,4] root=true driver=true
    [sched] round-robin context-switches=20 progress=[24280761,23852939]
    [sched] resident shared-ready-queue cpus=2 ticket-lock=true idle-tasks=2 switches=[20,26] non-idle=[8,15]
    [irq] virtio-blk INTx completions=1
    [service] resident EL0 ready=6/6 online=6/6 switches=38012
    [irq] resident notification blocking-waits=2839 irq-acks=2842 sgi-wakeups=5696
    [user] mfs1 segment-directory active-segments=2 low-water=15 high-water=25

动态池跨任务 move 的真实验证由生产 probe 完成：init→console 移交 pool cap，console
创建带授权的 Frame，再将 pool+Frame cap 原子移回 init；仍有 live frame 时删除返回 `Busy`，释放后删除成功；
同时保留 quota NoMemory、registry unregister/reuse 证据。其余 root manifest、六
resident services、PCI/BAR、SMMU/DmaMap/dynamic-frame、EL0 queue I/O、三隔离、
MFS/file-chain/proc/Shell/shutdown 全部通过；未见非预期 panic、translation/CMDQ/
GERROR、allocator fault 或 timeout。

Powercut PASS：token=`powercut-1786343431-389-29288`，first/second status=0；首次
sync 后 SIGTERM 无 guest shutdown，二次恢复 MFS/boot-proof、精确 token 并正常
shutdown。随后 `make fsck` exit=0：

    MFS1 clean generation=256 transactions=255 entries=5 used_blocks=373

### Dynamic MemoryPool registry 最终验收（2026-08-10）

本轮仅扩展 `scripts/smoke-qemu.sh` required marker，生产代码未改动；新增并严格匹配：

    [mm] MemoryPool dynamic quota=2 third=NoMemory live-delete=Busy reclaimed=true registry-reused=true

静态检查 `bash -n scripts/*.sh`、`cargo fmt --all -- --check` 均通过。只读复核
`crates/kernel/src/service_runtime.rs` 的实际契约：registry 最大 8 个总槽位，root/driver
固定池各占 1 个，因此最多 6 个动态池；单池 quota 上限 16，所有已注册池（含固定池）
的 quota 总和受 32 个 runtime frame 限制。root/driver 固定池不可注销。动态池由 owner
cap 创建，第三帧按 quota 返回 `NoMemory`；
仍有 live frame 时删除池返回 `Busy`，释放 frame 后池回收并清空 registry，下一池可
复用该槽位；最后引用删除路径执行 unregister/reclaim。

OrbStack context=`orbstack`。`make build` exit=0；`make test` exit=0（MFS recovery
12/12、ABI 6/6、kernel core-boundaries 11/11，normal smoke + powercut 全绿）。
Normal `target/smoke-qemu.log` 原始证据：

    [mm] MemoryPool dynamic quota=2 third=NoMemory live-delete=Busy reclaimed=true registry-reused=true
    [mm] MemoryPool independent pools=2 quotas=[4,4] root=true driver=true
    [mm] EL0 user heap free-list reuse allocations=512 bytes=4096 true
    [sched] round-robin context-switches=20 progress=[21177650,25040972]
    [sched] resident shared-ready-queue cpus=2 ticket-lock=true idle-tasks=2 switches=[26,20] non-idle=[17,10]
    [irq] virtio-blk INTx completions=2
    [service] resident EL0 ready=6/6 online=6/6 switches=40775
    [irq] resident notification blocking-waits=2497 irq-acks=2509 sgi-wakeups=5130
    [user] mfs1 segment-directory active-segments=2 low-water=15 high-water=25

随后 root manifest、六 resident services、PCI/BAR、SMMU/DmaMap/dynamic-frame、EL0
queue I/O、三隔离 probe、MFS/file-chain/proc/Shell/shutdown 全部通过；日志未见
非预期 `panic`、translation/CMDQ/GERROR、allocator fault 或 timeout。

Powercut PASS：token=`powercut-1786342695-389-30338`，first/second status=0；首次
sync 后 SIGTERM 日志无 guest shutdown，二次日志恢复 MFS/boot-proof、精确 token 并
正常 shutdown。随后 `make fsck` exit=0：

    MFS1 clean generation=245 transactions=244 entries=5 used_blocks=339

### EL0 allocator free-list reuse 最终验收（2026-08-10）

本轮仅在 `scripts/smoke-qemu.sh` 增加 exact required marker：

    [mm] EL0 user heap free-list reuse allocations=512 bytes=4096 true

未改生产代码；`bash -n scripts/*.sh`、`cargo fmt --all -- --check`、OrbStack
`make build` 均通过。

`make test` exit=0：MFS recovery 12/12、ABI 6/6、kernel core-boundaries 11/11，
normal smoke 与 powercut 全绿。Normal 命中 free-list marker、final cpu-mask=21（OR=3）、
counter 同 PID wait0/ps、MemoryPool/protocol、MFS/proc/Shell/shutdown；resident
ready=6/6 online=6/6 switches=40703、notification=2294/2298/4581、INTx completions=2。

Powercut PASS：token=`powercut-1786341013-388-5576`，first/second status=0；两次均命中
free-list marker与 MFS recovery/fsync，first `sync: ok` 后 SIGTERM 无 guest shutdown，
second 精确回读 token 后正常 shutdown。无 panic 或 `memory allocation ... failed`。

随后 `make fsck` exit=0：

    MFS1 clean generation=234 transactions=233 entries=5 used_blocks=305

### 独立 powercut mask 诊断通过（2026-08-10）

仅执行一次 OrbStack `scripts/powercut-qemu.sh`，日志为
`target/powercut-mask-diagnostic-first.log` 与
`target/powercut-mask-diagnostic-second.log`；未运行 host/full test/fsck，未修改生产或
正式门禁。

Powercut PASS：token=`powercut-1786340793-1-8037`，first/second status=0。两次启动均
观察 spinner PID7/8 与 final `cpu-masks-pair=33`（OR=3）。First 完成 MFS mount/recovery/
fsync/segment-directory、filesystem protocol，收到 `sync: ok` 后 host SIGTERM，日志无
guest shutdown；second 精确读回 token，MFS mount/recovery/fsync/segment-directory 后
正常 `[system] shutdown`。未见 panic、memory allocation failure、translation/CMDQ/GERROR、
allocator fault 或 timeout。

### 独立 powercut 诊断首错（2026-08-10）

仅执行一次 OrbStack `scripts/powercut-qemu.sh`，first/second 日志分别指定为
`target/powercut-diagnostic-first.log` 与 `target/powercut-diagnostic-second.log`；
未运行 host/full test/fsck，未修改生产或正式门禁。

首启动在发出 `/powercut` 写入前即发生 MFS service panic，second 日志未生成。完整
位置（first log lines 95--105）：

    [user] mfs1 mounted via EL0 block IPC
    [user] mfs1 checkpoint valid=true segment-blocks=256
    [user] mfs1 recovered /boot-proof after restart=true
    [user] mfs1 fsync /boot-proof persistent=true
    [user] mfs1 segment-directory active-segments=1 low-water=15 high-water=25
    [ipc] resident mfs endpoint=3 ready
    [user] mfs1 background-writeback=1s online-gc=true
    [ipc] resident shell->mfs magic=MFS1
    [panic] mfs service: panicked at .../library/alloc/src/alloc.rs:573:9:
    memory allocation of 4096 bytes failed
    [service] critical service mfs exited status=1

故障发生在 MFS mount/recovery/fsync/checkpoint 与 shell->mfs 初始化之后、powercut
脚本发送 `sync` 之前；脚本 exit=1（first boot did not acknowledge sync），无 second
token/status 可报告。日志未见 translation/CMDQ/GERROR/allocator fault 或 timeout；该
panic 是 MFS 用户服务的 4096-byte heap allocation 失败。

### 最终统一 SMP mask 验收首错（2026-08-10）

`bash -n scripts/*.sh`、`cargo fmt --all -- --check`、OrbStack `make build` 均通过。
`make test` host 与 normal smoke 通过：normal 最终 mask=`12`（spinner PID7/8，
两位 1/2，OR=3），counter 同 PID started/completed/wait0、最终 ps、shared-ready、
resident/ticks、MemoryPool/protocol 及全部既有门禁均命中；normal RR=20、resident
ready=6/6 online=6/6 switches=42645、notification=1861/1871/3793。

Powercut 首启动写入并 `sync: ok`，token=`powercut-1786340281-387-15773`，host
SIGTERM 后无 guest shutdown；第二启动 spinner mask=`33`（仍满足 OR=3），MFS mount/
recovery/fsync/segment-directory 均出现，但随后：

    [service] critical service mfs exited status=1

powercut 因 second boot 未读回 token 首错，`make test` exit=2；未运行 `make fsck`。
未观察 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### SMP mask bounded diagnostic（2026-08-10）

按要求仅在 OrbStack 容器复用已构建 ELF 执行一次有界 smoke，日志为
`target/smp-mask-diagnostic.log`；未运行 host/powercut/fsck，未修改门禁或生产代码。
串口实际证据：

    [app] spinner pid=7 started at EL0
    [app] spinner pid=8 started at EL0
    [sched] application spinners observed cpu-masks-pair=22
    [service] critical service init exited status=22

实际 mask pair=22：两位均为 2（各自位值合法 1..3），但按位或为 2 而非 3，表示两
spinner 仅观察到 CPU1，未覆盖 CPU0；因此 init status=22 与 mask gate 一致。未出现
要求的 shared-ready-queue 行、resident ticks/ps 或后续 service 链；已有早期 CPU 证据为
`[sched] cpu-bound complete counters=[80000000,80000000] timer-preemptions=[9,9]`、
`[sched] round-robin context-switches=20 progress=[22788764,25068155]`，以及 timer
self-test ticks=3。诊断命令 exit=1（预期因现有 strict smoke 门禁未命中其它后续 marker）。

### SMP mask bounded smoke 重验（2026-08-10）

按要求仅在 OrbStack 容器复用当前重建 ELF 执行一次 strict smoke，日志
`target/smp-mask-diagnostic2.log`；未运行 host/powercut/fsck，未修改生产或门禁。
Observed/final mask 均为：

    [app] spinner pid=7 started at EL0
    [app] spinner pid=8 started at EL0
    [sched] application spinners observed cpu-masks-pair=33
    [sched] application spinners dual-core=true tasks=2 cpus=2 cpu-masks-pair=33

pair=33 表示两位均为 3（各位 1..3、按位或=3），覆盖 CPU0/CPU1；未出现 init
critical-exit。相邻调度/ready/ticks 证据：

    [sched] resident shared-ready-queue cpus=2 ticket-lock=true idle-tasks=2 switches=[20,22] non-idle=[9,12]
    [service] resident EL0 ready=6/6 online=6/6 switches=38253
    PID 6 devmgr resident cpu=shared ticks=[2057,2051]
    [irq] self-test ticks=3 ...

strict smoke exit=0，RR=20、counter/PID lifecycle、三隔离 probe、MFS/proc/Shell、
shutdown 及其他既有门禁均通过；无 panic、translation/CMDQ/GERROR、allocator fault 或
timeout。按任务范围未执行 powercut/fsck。

### UART 职责清理与文件协议最终验收（2026-08-10）

本轮仅修改 `scripts/smoke-qemu.sh`：移除 generic `wait status=0` 门禁及旧的
`make run status=...` PASS 输出，新增 `run counter`、counter started/completed、
`wait status=0`、最终 ps `counter exited status=0` 的同 PID 关联校验；保留文件协议、
heap/frame/TaskMemory、DTB allocator、cross-page/no-XPG 及全部既有门禁。未改生产代码。
`bash -n scripts/*.sh` 与 `cargo fmt --all -- --check` 通过。

OrbStack `make build` exit=0；`make test` exit=0：MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11，normal smoke 与 powercut 均通过。Normal 串口确认：

    run: counter pid=7
    [app] counter pid=7 started at EL0
    [app] counter pid=7 completed
    wait: pid=7 status=0
    PID 7 counter exited status=0
    [ipc] filesystem open/read/write/fsync/sync/stat/readdir/mkdir/rename/unlink protocol=true

Normal 同时通过 bootfs14/13、heap/frame/TaskMemory、cross-page no-XPG、MFS
mount/recovery/fsync/segment-directory、PCI/SMMU/INTx、resident/notification、proc/Shell
及 shutdown；RR=20、resident ready=6/6 online=6/6 switches=2974、notification
1245/1257/1270、INTx completions=2。

Powercut PASS：token=`powercut-1786337802-381-559`，first/second status=0。first 日志
确认 `sync: ok` 后 host SIGTERM、无 guest shutdown；second 日志精确读回同一 token，MFS
mount/recovery 后正常 shutdown。随后 `make fsck` exit=0：

    MFS1 clean generation=191 transactions=190 entries=5 used_blocks=175

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### SMP dual-core application mask 验收首错（2026-08-10）

只读复核 `services/init::verify_process_kill`：两个 spinner 需保持 running，分别记录
`thread_status_with_cpu_mask`，mask 非零且按位或为 `0b11` 后才输出
`[sched] application spinners dual-core=true tasks=2 cpus=2 cpu-masks-pair=N`；mask 失败、
kill/reclaim 或 counter slot reuse/wait 失败均以 status=22 退出。本轮在
`scripts/smoke-qemu.sh` 新增 marker 与两位 mask 解析（每位 1..3、按位或=3），未改生产。
`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 均通过。

OrbStack `make build` exit=0；host 阶段 MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11 全通过。Normal smoke 在两个 spinner 启动后首错：

    [app] spinner pid=7 started at EL0
    [app] spinner pid=8 started at EL0
    [service] critical service init exited status=22

未出现 `cpu-masks-pair` marker，也未进入 counter/后续服务链，因此无法证明本轮双核
mask 满足约束；`make test` exit=2，按首错策略未运行 powercut 或 fsck。未观察
panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### MemoryPool 独立池最终验收（2026-08-10）

复核 `service_runtime` 的只读实现：`ROOT_MEMORY_POOL_OBJECT` 与
`DRIVER_MEMORY_POOL_OBJECT` 分别映射独立 pool bit，`MEMORY_POOL_QUOTA=4`；Frame 创建
按 authority 校验 MemoryPool/Manage 权限，单池超额返回 `Status::NoMemory`，失败插入
会释放 runtime frame，回收按 owner 清理。smoke 新增 required marker：

    [mm] MemoryPool independent pools=2 quotas=[4,4] root=true driver=true

未修改生产代码；`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 通过。

OrbStack `make build` exit=0；`make test` exit=0：MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11，normal smoke 与 powercut 全绿。Normal 串口同时命中独立池、
DTB quota、DmaMap/dynamic-frame、filesystem protocol、counter 同 PID wait0/ps、
MFS/proc/Shell/shutdown；RR=20、resident ready=6/6 online=6/6 switches=3603、
notification=1517/1529/1542、INTx completions=2。

Powercut PASS：token=`powercut-1786338892-381-23302`，first/second status=0；first
`sync: ok` 后 host SIGTERM 且无 guest shutdown，second 精确恢复 token 并正常 shutdown。
随后 `make fsck` exit=0：

    MFS1 clean generation=202 transactions=201 entries=5 used_blocks=209

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### Powercut wait-timeout 修复复验首错（2026-08-10）

本轮未改生产代码；保留文件协议、kernel heap/frame/TaskMemory、DTB allocator、
cross-page/no-XPG 及全部既有 smoke 门禁。`make build` exit=0；host 阶段 MFS recovery
12/12、ABI 6/6、kernel core-boundaries 11/11 全通过。

Normal smoke 在串口阶段命中 filesystem protocol marker：

    [ipc] filesystem open/read/write/fsync/sync/stat/readdir/mkdir/rename/unlink protocol=true

但 strict required marker `wait: pid=<n> status=0` 首错。`target/smoke-qemu.log` 中
counter 已实际完成并退出 status=0，随后输出交错为：

    [proc] application pid=7 exitewait: pid=7 std status=atus=0
    0

最终 `ps` 仍显示 `PID 7 counter exited status=0`，但严格 wait 行匹配失败；因此
normal smoke/make test exit=2，未进入 powercut，未运行 fsck。该证据表现为 shell wait
回复与 kernel proc-exit 串口输出交错，而非 wait timeout。故本轮没有新的 powercut token、
二次恢复或 entries=5 结果可报告；未观察 panic、translation/CMDQ/GERROR、allocator
fault 或 timeout。

### EL0 文件协议验收首错（2026-08-10）

本轮仅修改 `scripts/smoke-qemu.sh`：新增 required marker
`[ipc] filesystem open/read/write/fsync/sync/stat/readdir/mkdir/rename/unlink protocol=true`；
保留 kernel heap/frame/TaskMemory、DTB allocator、cross-page no-XPG 及其余现有门禁。
`bash -n scripts/*.sh` 与 `cargo fmt --all -- --check` 均通过，未改生产代码。

OrbStack `make build` exit=0。`make test` 的 host 阶段通过 MFS recovery 12/12、ABI
6/6、kernel core-boundaries 11/11；normal smoke PASS（`target/smoke-qemu.log`），
并命中新协议 marker：

    [ipc] filesystem open/read/write/fsync/sync/stat/readdir/mkdir/rename/unlink protocol=true

同一串口还确认 `/protocol-proof` 的 mkdir/write/open/read/fsync/sync/stat/readdir/
rename/unlink 流程由 EL0 shell 完成；协议实现会删除临时文件及目录，故预期最终 MFS
entries=5，但本轮未到 fsck 阶段，未把旧 fsck 结果冒充为本轮证据。Normal 链的
heap/frame/TaskMemory、cross-page no-XPG、MFS mount/recovery/fsync/segment-directory、
PCI/SMMU/INTx、resident ready/notification、proc/Shell、sync/shutdown 均通过。

随后 powercut 首启动未收到 `sync: ok`，`make test` exit=2（错误：
`powercut-qemu: first boot did not acknowledge sync`），因此按首错策略未启动第二次
powercut、未运行 `make fsck`；本轮新 token 为 `powercut-1786337042-367-9869`，
仅生成 `target/powercut-1786337042-367-9869-first.log`。首轮日志止于
`[service] critical service init exited status=22`，没有 panic、translation/CMDQ/GERROR、
allocator fault 或 timeout；第二轮 token 回读与 fsck entries=5 均未验证。

### Shared filesystem frame isolation 最终验收（2026-08-10）

本轮新增 ABI 回归断言 SHARED_FILESYSTEM_FRAME = CapHandle::from_parts(25, 1)，
并在 smoke required markers 加入 [ipc] filesystem payload frame isolated from block
DMA=true；保留 bootfs14/13、cross-page PID/no-XPG、DTB allocator、设备与全链门禁。
bash -n scripts/*.sh 与主机 cargo fmt --all -- --check 均通过，生产代码未改动。

OrbStack make build exit=0；make test exit=0（MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11，normal smoke + powercut 全绿）。Normal 串口命中：

    [ipc] filesystem payload frame isolated from block DMA=true
    [bootfs] valid=true entries=14 static-elfs=13
    [app] cross-page pointer probe pid=7 entered EL0
    [isolation] cross-page user buffer rejected-before-copy status=-8 task-survived=true
    [proc] cross-page pointer probe pid=7 wait-status=0 task-survived=true

normal smoke 另命中 root manifest/六 service、DmaMap/dynamic-frame、INTx completions=2、
MFS mount/recovery/fsync/segment-directory、proc name-loader/wait、Shell 与 shutdown；
RR=20，resident ready=6/6 online=6/6 switches=1873，notification=781/789/801，日志无
XPG! 或非预期 fault/panic/timeout。

此前根因是 block DMA 使用的 shared block frame 与 MFS/Shell IPC payload frame 共用同一
物理页，块设备数据/描述符写入可污染 filesystem request/reply。当前
install_boot_capabilities 为 FILESYSTEM_FRAME_OBJECT 单独分配物理页，仅映射给
MFS/Shell；block 继续使用 SHARED_FRAME_OBJECT，并由上述隔离 marker、MFS/file-chain
及 powercut 恢复证据共同验证。

Powercut PASS：token=powercut-1786335041-546-8867，first/second status=0；首次
SIGTERM 后无 guest shutdown，二次恢复 MFS/boot-proof、精确 token 后正常 shutdown。
随后 make fsck exit=0：

    MFS1 clean generation=165 transactions=164 entries=5 used_blocks=103

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### Kernel heap migration 最终验收（2026-08-10）

本轮 smoke 新增 kernel heap、frame allocator、DTB RAM 与 TaskMemory 解析门禁：
kernel heap 必须 bytes=0x2000000，base/allocator 均需页对齐并位于 DTB RAM，且
frame_base 必须等于 heap_base+heap_bytes；TaskMemory slot-bytes/allocated 必须均为
正值且不超过 heap bytes。bash -n scripts/*.sh 与主机 cargo fmt --all -- --check
通过；容器 readelf/size 证据为：

    __bss_start=0xffffff804036f000 __bss_end=0xffffff80403791f8
    __kernel_end=0xffffff804037a000
    text=1373488 data=122592 bss=41464

此前基线文档仅记录旧 frame allocator base=0x41617000；本轮新链接布局将
__kernel_end 物理地址对齐到 0x4037a000，并把 32MiB kernel heap 放在该地址，
allocator frame base 后移到 0x4237a000。

OrbStack make build exit=0；make test exit=0（MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11，normal smoke + powercut 全绿）。Normal 串口/解析实际值：

    [boot] ram=0x40000000+0x10000000 uart=0x9000000
    [mm] kernel heap ready base=0x4037a000 bytes=0x2000000
    [mm] frame allocator ready base=0x4237a000 free=56454
    [mm] TaskMemory/page tables allocated from kernel heap slot-bytes=0x12b000 allocated=0x12b000

脚本输出确认 heap/frame 对齐与连续关系、heap/TaskMemory 均在 DTB RAM 和 32MiB
预算内；其余 root manifest、六 service、filesystem frame isolation、PCI/SMMU/INTx、
cross-page no-XPG、MFS/proc/Shell/shutdown、RR=20、resident ready=6/6 online=6/6
switches=2119、notification=895/902/913 均通过。

Powercut PASS：token=powercut-1786336225-366-7871，first/second status=0；首次
SIGTERM 后无 shutdown，二次恢复 MFS/boot-proof、精确 token 后正常 shutdown。随后
make fsck exit=0：

    MFS1 clean generation=172 transactions=171 entries=5 used_blocks=117

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### Cross-page pointer 门禁验收首错（2026-08-10）

本轮仅修改 scripts/smoke-qemu.sh：bootfs 门禁更新为 entries=14/static-elfs=13；新增
cross-page probe 的动态 PID entry/rejected-before-copy/wait=0 窗口校验，并要求该窗口
内无 application fault；全日志出现 XPG! 时直接失败。bash -n scripts/smoke-qemu.sh
通过，生产代码未改动。

OrbStack make build exit=0；Host 阶段通过 MFS recovery 12/12、ABI 6/6、kernel
core-boundaries 11/11。Normal smoke PASS（日志 target/smoke-qemu.log），实际 cross-page
串口链为：

    [bootfs] valid=true entries=14 static-elfs=13
    [app] cross-page pointer probe pid=7 entered EL0
    [isolation] cross-page user buffer rejected-before-copy status=-8 task-survived=true
    [proc] cross-page pointer probe pid=7 wait-status=0 task-survived=true

窗口内无 application fault，日志无 XPG!；后续 root manifest、六 service、PCI/BAR、
SMMU/DmaMap/dynamic-frame、INTx completions=2、EL0 queue I/O、MFS、proc/Shell、shutdown
均命中，RR=20、resident ready=6/6 online=6/6 switches=1740、notification=721/726/738。

按首错策略 powercut 后续链未通过，make test exit=2，未运行 make fsck。首轮 powercut
写入并 sync 后收到 host SIGTERM；第二轮正常 mount/recovery/shutdown，但读回旧 token：

    expected: powercut-1786334536-353-7890
    observed: powercut-1786333944-331-10373

因此 powercut 脚本报告 “second boot did not read back the persisted token”；未观察
panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### DTB FrameAllocator 最终统一门禁（2026-08-10）

本轮仅扩展 scripts/smoke-qemu.sh 门禁：保留 memory-pool、DmaMap 与 dynamic-frame
要求，并新增 [mm] frame allocator ready base=0x... free=... 解析；脚本同时读取
[boot] ram=0xBASE+0xSIZE，验证 allocator base 4KiB 对齐、落在 DTB RAM 范围内且
free>0。bash -n scripts/*.sh 与主机 cargo fmt --all -- --check 均通过。

OrbStack make build exit=0；随后 make test exit=0（MFS recovery 12/12、ABI 6/6、
kernel core-boundaries 11/11，normal smoke + powercut 均通过）。Normal 串口实际
FrameAllocator 证据：

    [boot] ram=0x40000000+0x10000000 uart=0x9000000
    [mm] frame allocator ready base=0x41617000 free=59881
    [mm] MemoryPool DTB frame allocator active quota=4
    [mmu] resident memory-pool quota=4 frame-map/unmap remap=true mapped-delete=busy reclaimed=true
    [iommu] resident block DmaMap frames=2 capability=true pte=true cmdq=true unmap-remap=true
    [iommu] resident block dynamic-frame iova=0x102000 read=true reclaimed=true

脚本因此确认 0x41617000 % 0x1000 == 0 且位于 [0x40000000,0x50000000)，
free=59881 为正。其余 root manifest→六 service entered/ready/online、PCI/BAR、
SMMU CMDQ/fault blocked、INTx completions=2、EL0 queue I/O、三隔离、proc
name-loader/wait、MFS/proc/Shell/shutdown 均通过；RR=20、resident
ready=6/6 online=6/6 switches=1846、notification 646/646/657。

Powercut PASS：token=powercut-1786333944-331-10373，first/second status=0；首次
SIGTERM 日志无 shutdown，二次恢复 MFS/boot-proof、精确 token 后正常 shutdown。
随后 make fsck exit=0：

    MFS1 clean generation=156 transactions=155 entries=5 used_blocks=85

未观察非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout（仅预期
deadline marker）。

### Resourceprobe 退出回收验收首错（2026-08-10）

本轮仅更新 `scripts/smoke-qemu.sh`：bootfs 门禁改为 `entries=15/static-elfs=14`，
输入增加 `run resourceprobe`，并加入同一动态 PID 的 entry → live pool-frame-mapping
退出 → wait status=0 → `frames=1 pools=1 mappings=1` → name-loader 顺序关联校验。
`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 均通过；生产代码未改动。

OrbStack context=`orbstack`；`make build` exit=0；host MFS/ABI/kernel 阶段均通过
（MFS 12/12、ABI 6/6、kernel 11/11）。正式 `make test` 在 normal smoke 首错，
exit=2，未运行 powercut 或 `make fsck`。串口尾部：

    [bootfs] valid=true entries=15 static-elfs=14
    micro> run resourceprobe
    run: resourceprobe pid=7
    [app] resource cleanup probe pid=7 entered EL0

随后 45s 内没有 live-resource exit、wait/reclaim、name-loader 或 shutdown；脚本报
`guest did not reach shutdown within 45s`。根因是 shell `run` 对非 spinner 同步调用
PROCESS Wait：init 的 PROCESS endpoint 正在 Wait handler 中循环等待 pid=7，而
resourceprobe 又必须向同一 PROCESS endpoint 发 MemoryPool(operation=5) 请求，导致
init 服务端自等待死锁。无非预期 panic、translation/CMDQ/GERROR 或 allocator fault。

### Resourceprobe 死锁修复后最终验收（2026-08-10）

生产修复将 shell 的 PROCESS Wait 改为可返回 `Busy` 的轮询，使 init 能继续处理
resourceprobe 的 `process::Operation::MemoryPool=5` 请求；本轮测试侧未再改生产代码，
仅复用上述 smoke 门禁。OrbStack `make build` exit=0；容器内单次 normal smoke
`target/resourceprobe-fixed-smoke-qemu.log` exit=0，随后正式 `make test` exit=0。

修复后 normal 串口同一 PID=7 完整链：

    [bootfs] valid=true entries=15 static-elfs=14
    run: resourceprobe pid=7
    [app] resource cleanup probe pid=7 entered EL0
    [app] resource cleanup probe pid=7 exiting with live pool-frame-mapping=true
    wait: pid=7 status=0
    [mm] application exit reclaimed pid=7 frames=1 pools=1 mappings=1
    [proc] bootfs name-based loader program=resourceprobe pid=7 static-elf=true

同一 normal log 命中全部既有门禁：RR=20 `progress=[24649107,24521988]`、shared-ready
`switches=[24,20] non-idle=[14,8]`、INTx completions=2、resident `ready=6/6
online=6/6 switches=45075`、notification `3139/3155/6419`、MFS active-segments=2，
且无非预期 panic/translation/CMDQ/GERROR/allocator fault/timeout。

完整 `make test` host 计数为 MFS 12/12、ABI 6/6、kernel 11/11；normal smoke 与
powercut 均通过。Powercut token=`powercut-1786344872-421-7748`，first/second
status=0，首次 sync+SIGTERM 无 shutdown，二次精确 token 恢复并 shutdown。随后
`make fsck` exit=0：

    MFS1 clean generation=275 transactions=274 entries=5 used_blocks=431

### Process MemoryPool ABI opcode 定向回归（2026-08-10）

现有 `crates/abi/tests/capability_abi.rs` 的稳定编号测试确认
`process::Operation::MemoryPool as u16 == 5`，保护 resourceprobe 使用的公共协议号。
OrbStack 定向执行 `cargo test -p microsystem-abi` exit=0：ABI 测试 6/6 通过，unit
与 doc-tests 均为 0/0；本轮未运行 QEMU、完整 `make test` 或 `make fsck`。

### Resourcefault 携资源 fault 回收验收（2026-08-10）

本轮仅更新 `scripts/smoke-qemu.sh`：输入在 `run resourceprobe` 后紧跟
`run resourcefault`，bootfs 门禁改为 `entries=16/static-elfs=15`，并新增
resourcefault 同一动态 PID 的 entry → live `frames=2/mappings=2` → FAR=0x600000
application fault → wait=-8 → reclaim `frames=2 pools=1 mappings=2` → name-loader
顺序关联校验。并将 resourceprobe 的 run 解析容忍并发 UART 字节拼接
（如 `resourcep[app]robe`），不放宽 PID 或生命周期门禁；fault 关联限定在 live
标记之后，避免 PID=7 槽复用误配 pageprobe。bash-n 与 cargo fmt check 均通过，生产代码未改动。

OrbStack context=`orbstack`；`make build` exit=0。首次 normal smoke 仅因 UART 拼接
使旧 resourceprobe marker 漏检而失败；修正脚本解析后最终单次
`target/resourcefault-smoke-qemu-final.log` normal smoke exit=0。关键同 PID=7 串口链：

    [bootfs] valid=true entries=16 static-elfs=15
    run: resourceprobe pid=7
    [app] resource cleanup probe pid=7 entered EL0
    wait: pid=7 status=0
    [mm] application exit reclaimed pid=7 frames=1 pools=1 mappings=1
    [proc] bootfs name-based loader program=resourceprobe pid=7 static-elf=true
    run: resourcefault pid=7
    [app] resource fault probe pid=7 entered EL0
    [app] resource fault probe pid=7 triggering page fault with live frames=2 mappings=2
    [proc] application fault ESR=0x92000006 FAR=0x600000; pid=7 terminated
    wait: pid=7 status=-8
    [mm] application exit reclaimed pid=7 frames=2 pools=1 mappings=2
    [proc] bootfs name-based loader program=resourcefault pid=7 static-elf=true

Normal smoke 还确认最终 `PID 7 counter exited status=0`、`PID 8 spinner exited status=-15`、
RR=20 `progress=[23092014,24092658]`、shared-ready `switches=[20,24] non-idle=[9,15]`、
INTx completions=2、resident `ready=6/6 online=6/6 switches=44243`、notification
`blocking-waits=3699 irq-acks=3711 sgi-wakeups=7524`、MFS active-segments=2 与 shutdown，
且无非预期 panic/translation/CMDQ/GERROR/allocator fault/timeout。

随后统一 `make test` 的 host 阶段通过 MFS recovery 12/12、ABI 6/6、kernel 11/11；
normal smoke 首错因既有 pageprobe 日志顺序竞态失败（同 PID 的
`wait-status=-8 reclaimed=true` 先于 application fault 行），故 `make test` exit=2，
按首错策略未运行 powercut 或 `make fsck`。resourcefault 链在该日志中仍完整；未观察
非预期 panic、translation/CMDQ/GERROR、allocator fault 或 timeout。

### Resourcefault 窗口解析修复后统一验收（2026-08-10）

脚本新增 `first_line_between`：pageprobe 的 fault/wait 均限定在 entry 到下一次
resourcefault entry 窗口，允许 UART 行顺序竞态；resourcefault 的 fault/wait/reclaim/
loader 均限定在其 live 到下一次 privprobe 窗口，要求同 PID 但不强制彼此顺序。
`bash -n scripts/*.sh` 与 `cargo fmt --all -- --check` 通过；既有通过日志静态解析得到
pageprobe `pid=7 entry=66 wait=68 fault=67`，resourcefault `pid=7 entry=148 live=149
fault=150 wait=151 reclaim=152 loader=153`，窗口门禁全部满足。

OrbStack 唯一 `make test` exit=0：MFS recovery 12/12、ABI 6/6、kernel 11/11，normal
smoke 与 powercut 均通过。Normal `target/smoke-qemu.log` 的 resourcefault 同 PID=7
链完整，bootfs `entries=16/static-elfs=15`、RR=20 `progress=[22011304,25061035]`、
shared-ready `switches=[20,25] non-idle=[8,15]`、INTx completions=2、resident
`ready=6/6 online=6/6 switches=45533`、notification `3954/3960/7905`、MFS
active-segments=2；counter PID7 status0、spinner PID8 status-15、无非预期 fault/panic/
timeout/translation/CMDQ/GERROR/allocator。

Powercut token=`powercut-1786346868-461-26108`，first/second status=0；首次 sync 后
SIGTERM 无 shutdown，二次恢复并读取精确 token 后 shutdown。随后 `make fsck` exit=0：

    MFS1 clean generation=302 transactions=301 entries=5 used_blocks=513

### Resourcekill 携资源 ThreadKill 回收验收（2026-08-10）

本轮仅更新 `scripts/smoke-qemu.sh`：在 resourcefault 后紧跟 `run resourcekill`，
bootfs 门禁改为 `entries=17/static-elfs=16`，加入同一动态 PID 的 run → EL0 entry →
live `frames=2/mappings=2` → live-resources `frames=2/pools=1/mappings=2` → ThreadKill
`status=-15` → wait `status=-15` → reclaim `frames=2/pools=1/mappings=2` → name-loader
窗口校验；所有标记限制在 entry 到后续 privprobe 的窗口，同 PID，唯一强制
`live < kill`，以容忍跨 CPU UART 行序竞态。bash-n 与 cargo fmt check 通过，生产代码未改动。

OrbStack context=`orbstack`；`make build` exit=0。单次 normal smoke
`target/resourcekill-smoke-qemu.log` exit=0，PID=7 关键链：

    [bootfs] valid=true entries=17 static-elfs=16
    run: resourcekill pid=7
    [app] resource kill probe pid=7 entered EL0
    [app] resource kill probe pid=7 ready with live frames=2 mappings=2
    [mm] application live resources pid=7 frames=2 pools=1 mappings=2
    kill: pid=7 status=-15
    wait: pid=7 status=-15
    [mm] application exit reclaimed pid=7 frames=2 pools=1 mappings=2
    [proc] bootfs name-based loader program=resourcekill pid=7 static-elf=true

统一 `make test` exit=0：host MFS recovery 12/12、ABI 6/6、kernel 11/11，normal smoke
与 powercut 均通过。Normal RR=20 `progress=[24178629,24993624]`、shared-ready
`switches=[27,20] non-idle=[18,9]`、INTx completions=1、resident
`ready=6/6 online=6/6 switches=31563`、notification `4265/4282/8641`、MFS
active-segments=3；无非预期 fault/panic/timeout/translation/CMDQ/GERROR/allocator。

Powercut token=`powercut-1786347930-504-105`，first/second status=0；首次 sync 后
SIGTERM 无 shutdown，二次精确 token 恢复并 shutdown。随后 `make fsck` exit=0：

    MFS1 clean generation=317 transactions=316 entries=5 used_blocks=559

### Frame 删除/CapRevoke 与 DMA/MemoryPool Busy 门禁复核（2026-08-10）

本轮仅同步 `scripts/smoke-qemu.sh` 的 required marker（生产代码未改动）：

    [iommu] resident block dynamic-frame iova=0x102000 read=true dma-delete=busy reclaimed=true
    [mmu] resident memory-pool quota=4 frame-map/unmap remap=true mapped-delete=busy mapped-move=busy reclaimed=true

`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 通过；`docker context=orbstack`。
静态审查确认 `service_runtime::delete_capability` 在最后一个动态 Frame 引用且仍有
runtime mapping 时返回 `Busy`，成功删除后释放 runtime frame 并在无 frame/pool 引用时注销
动态 MemoryPool；`revoke_global` 按 cap-table 收集派生 Frame 对象，仅在该表无 MAP 副本时
清理该任务映射并执行 ASID TLBI，其他仍持 MAP 的任务保留映射。现有
`core_boundaries` 仅覆盖通用派生/revoke，init smoke 仅覆盖动态池 delete Busy；两条
`service_runtime` 分支没有可自然触达的独立 host/QEMU 门禁，未添加脆弱 fixture/syscall。

当前树 `make build` exit=0。随后唯一 `make test`：host MFS recovery 12/12、ABI 6/6、
kernel 11/11 均通过；normal smoke 首错在 resident shared-ready 阶段 45s timeout，日志
尾部为 `switches=[18454,20] non-idle=[18445,2]`，之后无 block/MFS/shell/shutdown。
未观察 panic、translation/CMDQ/GERROR、allocator fault；因首错未运行 `make fsck`。

### MemoryPool mapped-move blocked 修复后统一验收（2026-08-10）

生产 marker 更新为 `mapped-move=blocked`，并同步 smoke required marker；保留 DMA 删除
Busy 门禁：

    [iommu] resident block dynamic-frame iova=0x102000 read=true dma-delete=busy reclaimed=true
    [mmu] resident memory-pool quota=4 frame-map/unmap remap=true mapped-delete=busy mapped-move=blocked reclaimed=true

`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 通过；OrbStack `make build` exit=0。
修复后的唯一 `make test` exit=0：host MFS recovery 12/12、ABI 6/6、kernel 11/11，
normal smoke 与 powercut 均通过。Normal RR=20 `progress=[23150242,25194678]`，
shared-ready `switches=[20,10027] non-idle=[4,10018]`，INTx completions=2，resident
`ready=6/6 online=6/6 switches=54831`，notification `5077/5086/10297`，MFS
active-segments=3；无非预期 fault/panic/timeout/translation/CMDQ/GERROR/allocator。
Powercut token=`powercut-1786349163-504-17700`，first/second status=0；首次 sync 后
SIGTERM 无 shutdown，第二次按 token 恢复并 shutdown。随后 `make fsck` exit=0：

    MFS1 clean generation=350 transactions=349 entries=5 used_blocks=661

此前同一 marker=busy 的尝试首错是 resident shared-ready starvation（CPU0/CPU1
`switches=[18454,20]`、`non-idle=[18445,2]`，45s timeout）；本轮 blocked marker 修复后
服务链继续到 block/MFS/shell，证明该竞态已消失。

### TIME ABI/IPC smoke 验收（2026-08-10）

测试新增 ABI 断言 `time::Operation::{Sleep=1,Uptime=2}` 与
`boot_cap::TIME_ENDPOINT == CapHandle(slot=39,generation=1)`；smoke 新增
`[ipc] resident time sleep/uptime endpoint=4 shared-procman=true` required marker，
发送两次 `uptime` 并要求 shell 样本均大于零且单调。`bash -n scripts/*.sh`、
`cargo fmt --all -- --check`、`make build` 均成功，host MFS recovery 12/12、ABI 6/6、
kernel 11/11 通过。Guest 日志观察到 time marker，shell uptime `2964 ms -> 3966 ms`
单调；但 smoke 在既有 resourceprobe marker 首错：UART 交错为
`run: resourceprobe pid=7[app] resource clea` + 下一行 `nup probe ... entered EL0`，
required marker 缺少完整行，故 `make test` exit=1，未运行 powercut 或 fsck。该失败与
TIME 协议无关，完整日志保留于 `target/smoke-qemu.log`。

### TIME marker 与 resourceprobe 字节交错解析修复后验收（2026-08-10）

smoke 移除 resourceprobe entry/live 的整行 required marker，新增 `first_line_joined`
滑动字节窗口跨相邻 UART 行匹配（raw log 保留），并继续按同 PID 校验 run → entry →
live → wait=0 → reclaim frames=1/pools=1/mappings=1 → loader。`bash -n scripts/*.sh`、
`cargo fmt --all -- --check` 通过。修复后唯一 `make test` exit=0：host MFS 12/12、ABI
6/6、kernel 11/11，normal smoke 与 powercut 均通过。Shell uptime 样本 `2964 ms ->
3966 ms`，TIME marker 命中；normal RR=20 `progress=[21703392,22371730]`，shared-ready
`switches=[11937,20] non-idle=[11926,3]`，INTx=2，resident `ready=6/6 online=6/6
switches=59530`，notification `5505/5514/11035`，MFS active-segments=3；无非预期
fault/panic/timeout/translation/CMDQ/GERROR/allocator。Powercut token=
`powercut-1786374822-496-12520`，first/second status=0。随后 `make fsck` exit=0：

    MFS1 clean generation=365 transactions=364 entries=5 used_blocks=707

### Dynamic NotificationSignal/bitset smoke 验收（2026-08-10）

ABI 测试新增 `Syscall::NotificationSignal == 23`；smoke required marker 为：

    [irq] resident generic notification signal=true bitset=0x5 cross-task=true

为容忍真实 EL0 UART 字节交错，resourcekill 的 live/resource marker 改为滑窗解析：
同 PID、entry 后且 privprobe 前，分别验证 `ready with` 与
`pid=N frames=2 pools=1 mappings=2`，未放宽 kill/wait/reclaim/loader 顺序；raw log 保留。
`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 通过。OrbStack 最终 `make test`
exit=0：host MFS 12/12、ABI 6/6、kernel 11/11，normal smoke 与 powercut 均通过；
normal RR=20 `progress=[20660984,20874751]`，shared-ready
`switches=[10300,20] non-idle=[10289,2]`，INTx=1，resident
`ready=6/6 online=6/6 switches=60679`，notification `5816/5843/11943`，
TIME uptime `2965 ms -> 3968 ms`，MFS active-segments=3；generic NotificationSignal
marker 命中且无非预期 fault/panic/timeout/translation/CMDQ/GERROR/allocator。Powercut
token=`powercut-1786376122-493-19833`，first/second status=0。随后 `make fsck` exit=0：

    MFS1 clean generation=384 transactions=383 entries=5 used_blocks=765

### Dynamic blocked-wake notification 验收（2026-08-10）

smoke required marker 新增：

    [irq] resident generic notification wake-after-block=true cross-task=true

`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 通过；OrbStack `make build` exit=0。
最终 `make test` exit=0：host MFS 12/12、ABI 6/6、kernel 11/11，normal smoke 与
powercut 均通过。Normal RR=20 `progress=[23488169,24466937]`，shared-ready
`switches=[27,20] non-idle=[10,3]`，INTx completions=2，resident
`ready=6/6 online=6/6 switches=61747`，notification counters
`blocking-waits=6232 irq-acks=6242 sgi-wakeups=12503`，TIME uptime `2954 ms ->
3955 ms`，MFS active-segments=3；blocked-wake marker 命中，无非预期 fault/panic/
timeout/translation/CMDQ/GERROR/allocator。Powercut token=`powercut-1786376805-494-3975`,
first/second status=0。随后 `make fsck` exit=0：

    MFS1 clean generation=395 transactions=394 entries=5 used_blocks=799

### CapRevoke 远端映射/TLBI smoke 验收（2026-08-10）

smoke required marker 新增：

    [cap] resident revoke cleared remote frame mapping=true tlbi=true task-survived=true

`bash -n scripts/*.sh`、`cargo fmt --all -- --check` 通过；OrbStack `make build` exit=0。
`make test` exit=0：host MFS 12/12、ABI 6/6、kernel 11/11，normal smoke 与 powercut
均通过。Normal RR=20 `progress=[22986955,25192148]`，shared-ready
`switches=[20,28] non-idle=[2,11]`，INTx completions=1，resident
`ready=6/6 online=6/6 switches=62813`，notification `6508/6522/13052`，TIME
uptime `2961 ms -> 3965 ms`，MFS active-segments=4；远端 mapping/TLBI marker 命中，
无非预期 fault/panic/timeout/translation/CMDQ/GERROR/allocator。Powercut token=
`powercut-1786377168-495-7664`，first/second status=0。随后 `make fsck` exit=0：

    MFS1 clean generation=406 transactions=405 entries=5 used_blocks=833

### Bounded QEMU MFS transaction/GC fault-injection harness (2026-08-11)

新增只读诊断脚本 `scripts/fault-injection-qemu.sh`（未接入 xtask）：事务
`records-written`、`data-flushed`、`superblock-written`、`superblock-flushed` 四个
阶段均在当前唯一 `write /faultcut <token>` 回显之后截断；前两阶段恢复 old token，后两阶段
恢复 new token，四例均 `fsck` clean。MFS stage marker 增加 100 ms EL0 yield 后，first log
硬断言目标阶段之后没有下一阶段；本轮 `make build` exit=0（kernel 00:31:33，晚于
services/fs marker 00:21:16）。

GC 夹具独立从低水位镜像启动，宿主用固定 4 MiB source 重复覆盖 `/gc-fill`：第 11 次提交
`generation=418 used_blocks=14850 free_blocks=1278 total_blocks=16384`（7.8% free，低于
15%），seed `fsck` clean。低水位 guest boot 首错为
`[panic] mfs service: memory allocation of 1048576 bytes failed`，随后
`[service] critical service mfs exited status=1`，未到
`gc-candidate-superblock-flushed`，因此 GC case exit=1 且没有 PASS；该证据表明当前
EL0 MFS 1 MiB heap 无法在 4 MiB 低水位夹具上完成 mount/GC，未放宽为 not-run 或通过。
事务/GC 日志目录：`target/fault-injection-1786379501-1-21461/`。

### xtask fault-injection integration and 48 MiB kernel heap recheck (2026-08-11)

`xtask test` 现按 normal smoke、powercut、fault-injection 顺序执行；
`scripts/fault-injection-qemu.sh` 的 GC-only 等待/恢复超时独立为
`MICROSYSTEM_FAULT_GC_TIMEOUT`（默认 180 s），事务阶段仍使用 45 s。容器 awk
字节窗口解析改用 ARGV 保留 `\[` 转义；smoke kernel-heap 门禁同步为精确
`bytes=0x3000000`，并校验 frame allocator 紧随 heap。`bash -n scripts/*.sh`、
`cargo fmt --all -- --check` 通过。

最新 OrbStack `make build` exit=0；唯一正式 `make test` 在 host 与 guest 前置链路均通过：
MFS 12/12、ABI 6/6、kernel 11/11，normal smoke（RR=20、resident 6/6、notification、
SMMU/INTx、文件链、heap=0x3000000/frame-base=0x433c7000）与 powercut
token=`powercut-1786382263-495-19098` 均通过，四 transaction fault cases 均通过：
records/data 恢复 old，superblock-written/flushed 恢复 new，各自 fsck clean。

GC 默认 4 MiB 夹具仍为首错：seed 第 11 次提交
`generation=448 used_blocks=14850 free_blocks=1278 capacity_blocks=16128 total_blocks=16384`，
guest MFS 在 candidate marker 前 panic：`memory allocation of 8388032 bytes failed`，
随后 `critical service mfs exited status=1`；未到 candidate marker，GC recovery/fsck 未执行，
因此 `make test` exit=1，按首错策略未运行 `make fsck`。日志目录：
`target/fault-injection-1786382319-541-5368/`。

### GC extent-reservation fix recheck (2026-08-11)

生产端 `apply_record` 已改为按 extent `try_reserve_exact(record.data.len())`，并重建
MFS 16 MiB user heap / kernel 48 MiB image；`bash -n scripts/*.sh` 与
`cargo fmt --all -- --check` 通过，xtask fault command 保持默认 4 MiB（无 128 KiB fallback）。

OrbStack `make build` exit=0；随后唯一 `make test` 的 host MFS 12/12、ABI 6/6、kernel
11/11、normal smoke（heap `bytes=0x3000000`、frame base `0x433c7000`）与 powercut
token=`powercut-1786382482-697-7147` 均通过，四 transaction fault cases 均通过
（records/data old，superblock-written/flushed new，fsck clean）。GC 4 MiB seed 达到
iteration=11，`generation=459 used_blocks=14850 free_blocks=1278 capacity_blocks=16128`；
guest 已不再 panic，但 MFS 在 mount/checkpoint/recovered 后以 status=4 退出，未出现
`gc-candidate-superblock-flushed`，推断 boot-proof fsync 返回 `NoSpace`。因此
`make test` exit=1，GC recovery/fsck 未执行；日志目录：
`target/fault-injection-1786382538-744-2982/`。

### 64 MiB kernel / 32 MiB MFS heap final validation (2026-08-11)

测试脚本 heap 门禁更新为精确 `bytes=0x4000000`，并要求
`frame-base=heap-base+heap-bytes`；`bash -n scripts/*.sh` 与
`cargo fmt --all -- --check` 通过。OrbStack context=`orbstack`，`make build` exit=0。

唯一正式 `make test`（host + normal smoke + powercut + 四事务 fault + GC fault）通过：
host MFS 12/12、ABI 6/6、kernel 11/11。Normal smoke 记录
`kernel-heap base=0x403c7000 bytes=0x4000000 frame-base=0x443c7000`、
`TaskMemory slot-bytes=0x15c000 allocated=0x15c000`、RR
`context-switches=20 progress=[24332286,24815601]`、shared-ready
`switches=[27,20] non-idle=[11,3]`、INTx `completions=2`、resident
`ready=6/6 online=6/6 switches=66864`、notification
`blocking-waits=7748 irq-acks=7762 sgi-wakeups=15608`、MFS
`segment-directory active-segments=4`，以及文件协议、动态资源回收、双核和 shell 链；
日志中无非预期 fault/panic/timeout/translation/CMDQ/GERROR/allocator 错误。

Powercut token=`powercut-1786382736-496-11594`，first/second 恢复与 shutdown 通过。
事务 fault 四阶段均 PASS：records/data 恢复 old，superblock-written/flushed 恢复 new，
各自 fsck clean。默认 4 MiB GC 夹具也 PASS：seed
`generation=470 used_blocks=14850 free_blocks=1278 capacity_blocks=16128 total_blocks=16384`
（fill=4194304，11 iterations），first 在
`gc-candidate-superblock-flushed` 阶段截断且未进入 sealed，recovery 精确恢复
`/gc-proof`，fsck `MFS1 clean generation=478 transactions=475 entries=7 used_blocks=11023`。
Fault 日志目录：`target/fault-injection-1786382793-543-21539/`。

随后 OrbStack `make fsck` exit=0：

    MFS1 clean generation=458 transactions=457 entries=5 used_blocks=993

### GUI logic/QMP test slice (2026-08-11, static only)

新增 `crates/gui/tests/desktop.rs`：覆盖窗口创建时 UTF-8/title 截断与最小尺寸/边界
夹取、focus/z-order/Alt-Tab、move/resize、minimize/maximize/restore/close 状态契约。
新增 `crates/gui/tests/command_stream.rs`：覆盖合法 UTF-8 文本与多命令流、未知 kind、坏
矩形、截断尾命令、短 header、越界 command length，以及 `MAX_TEXT_BYTES`/`COMMAND_BYTES`
上限；将畸形尾命令视为整批拒绝，避免伪造 atomic Present API。

新增 `scripts/gui-qemu.sh`（仅测试脚本）：支持 OrbStack 容器内外入口，启动现有
`xtask gui`/QEMU，等待 virtio-gpu/windowd/terminal/files/monitor/present 串口 marker，
通过 `target/gui-qmp.sock` 执行 `qmp_capabilities`、`query-status`、`screendump`，校验
非空 PNG、1024x768、可解析像素及至少两种采样颜色，并在 bounded timeout/trap 中清理
QEMU/QMP socket。FrameRegion 配额/回滚/Busy/revoke、damage rect 与真实 atomic Present
仍只有 EL0 user_rt/kernel service_runtime 入口，GUI host 纯逻辑测试无法触达；未新增
伪造门禁。

静态验证：`bash -n scripts/*.sh`、GUI 测试单文件 `rustfmt --check`、QMP 内嵌 Python
语法检查均通过。全仓 `cargo fmt --all -- --check` 仍受既有未格式化生产文件影响，未改动
生产代码；本轮未运行统一测试或 GUI QEMU。

后续 OrbStack targeted validation：`cargo test -p microsystem-gui --tests` 8/8（Desktop
4、command stream 4）。首次 GUI QEMU 诊断确认 QEMU 7.2 `screendump` 实际输出 P6 PPM
而非 PNG；脚本已按 magic 兼容 PPM，并归一化保存 PNG。修复后单次
`scripts/gui-qemu.sh` exit=0，无残留 container/QEMU：QMP `qmp_capabilities` 成功、
`query-status`=`running`、`screendump` 成功；`target/gui-qemu.qmp.jsonl` 记录
`source_format=PPM-P6`，最终 `target/gui-screendump.png` 为 6479 bytes、1024x768、
RGB、采样颜色=2。串口 marker 全部命中：virtio-gpu scanout、windowd presented
`bytes=3145728 dma-isolated=true`、terminal/files/monitor ready、windowd ready
`clients=3`、desktop windows=3 move/resize/minimize/maximize/alt-tab=true。

### GUI input QMP/Alt-Tab gate (2026-08-11, first-error stop)

`gui-qemu.sh` 增加 input-ready 门禁，并通过 QMP `input-send-event` 注入 Alt+Tab：先抓
pre screendump，再等待 `[gui] keyboard+tablet input routed focus=true drag=true alt-tab=true`
后抓 post screendump，分别解码 PNG/P6 PPM 并比较 decoded-pixel CRC32。唯一 OrbStack
运行中 QMP `qmp_capabilities`、`query-status=running`、pre `screendump`、
`input-send-event` 均返回成功；input-ready marker 也命中，但 guest 首错为：

    [input] DMA fault event=0x2 stream-id=0x20 iova=0x0

随后 routed marker 未出现，post screendump/CRC 未生成，按首错策略停止且未重跑。pre
artifact 为 QEMU 7.2 P6 PPM（1024x768）；该结果暴露真实输入 DMA 映射问题，而非放宽
marker 或伪造 Alt-Tab 通过。无残留 QEMU/container。

### Unified host/QEMU/GUI acceptance (2026-08-11)

静态门禁：`docker context show`=`orbstack`；`bash -n scripts/*.sh` 通过，GUI 两段
QMP Python 块编译通过。`rustfmt --check xtask/src/main.rs` 仅报告既有未格式化代码行，
未改动生产文件。

首次整合尝试在 normal smoke 的旧 address-space 断言停止：串口实际为
`[service] resident EL0 address-spaces=10 asids=[0x20..0x29]`（无 GPU 时只有 6 个
service online），脚本当时要求 6/`0x20..0x25`。该断言同步为内核当前预分配的 10 个
resident slots 后重跑；没有放宽 ready/online 仍严格要求 `6/6`。

最终 OrbStack `make test` exit=0，顺序为 host → normal smoke → powercut → fault →
GUI：host MFS 12/12、ABI 6/6、kernel 11/11；normal smoke 通过 bootfs
`entries=21 static-elfs=20`、kernel heap `base=0x40740000 bytes=0x6000000`、
TaskMemory `slot-bytes=0x15c000 allocated=0x15c000`、frame base `0x46740000`、
dynamic first PID 11、RR `context-switches=20 progress=[23138444,25923921]`、
shared-ready `switches=[28,20] non-idle=[10,3]`、INTx `completions=2`、resident
`ready=6/6 online=6/6 switches=121642`、notification
`blocking-waits=27177 irq-acks=27194 sgi-wakeups=54277`、MFS active-segments=14、
file-chain and shell shutdown. No unexpected panic/translation/CMDQ/GERROR/allocator
fault or timeout was present; intentional user/page/privileged application fault markers
remain correlated with their probes.

Powercut passed with token `powercut-1786423896-495-30300` (first/second status=0 and
recovery/shutdown). Fault harness passed all four transaction cut points (expected first
status=137, recovery status=0, per-case fsck clean) and GC candidate cut:
4 MiB fill, 9 iterations, `generation=509 used_blocks=14850 free_blocks=1278
capacity_blocks=16128 total_blocks=16384`; first stopped at
`gc-candidate-superblock-flushed` before sealed, exact `/gc-proof` recovery and fsck
clean (`generation=517 transactions=514 entries=10 used_blocks=12047`). Fault logs:
`target/fault-injection-1786423952-547-22752/`.

GUI QMP gate passed after fault: serial windowd/terminal/files/monitor/present and input
routing markers hit; QMP running/pre/post and Alt+Tab injection succeeded. Decoded pixels
changed (`pre_crc32=1b6d4317`, `post_crc32=fd39440b`), post screenshot 1024x768 RGB,
source P6 PPM normalized to PNG. Artifacts: `target/gui-qemu.log`,
`target/gui-qemu.qmp.jsonl`, `target/gui-screendump-pre.png`,
`target/gui-screendump.png`.

Only after the green test, OrbStack `make fsck` exit=0:

    MFS1 clean generation=500 transactions=499 entries=8 used_blocks=4151

### GUI interactive Terminal QMP slice (2026-08-11, first-error stop)

针对性构建：OrbStack `docker context=orbstack`，`make build` exit=0；GUI 纯逻辑测试
`cargo test -p microsystem-gui --tests` 通过 8/8（command stream 4、Desktop 4）。

仅测试脚本改动：`scripts/gui-qemu.sh` 将 Terminal interactive marker 固定为
`[gui] terminal window ready interactive=true commands=help,uptime,ps,clear,echo`，
保留 Alt+Tab 路由门禁，并通过 QMP 依次注入 Alt+Tab、tablet 绝对坐标点击 Terminal
正文、`u p t i m e` 及 Return；等待
`[gui] terminal command executed name=uptime output=true`，然后校验 pre/post 解码像素
CRC 变化。脚本新增 `[panic]`/`DMA fault` 串口拒绝；bash-n 与两个内嵌 Python 块编译
均通过。

唯一 targeted `scripts/gui-qemu.sh` 首错 exit=1，发生在 QMP 输入前：新鲜 ELF/bootfs
串口出现 Terminal ready 后立即报告
`[service] critical service terminal exited status=3`，因此 windowd ready、uptime
command marker、QMP click/keyboard、pre/post CRC 均未执行；无 QMP 输入伪通过或 CRC
证据。原始日志：`target/gui-terminal-qemu.log`。按首错策略未重跑；该阻塞位于当前
Terminal 服务的 `ipc_recv` 生命周期，不是脚本 marker 放宽问题。

### GUI interactive Terminal QMP recheck after endpoint ownership fix (2026-08-11)

共享树 endpoint ownership 修复后，OrbStack `make build` exit=0；GUI 逻辑测试仍为
8/8。按首次失败仅修正 xtask 固定 `target/gui-qmp.sock` 与自定义日志参数的调用，未改
生产或再次构建；使用默认 QMP socket 执行唯一真实 Terminal gate，exit=0。

串口命中 Terminal interactive marker、Alt+Tab routed marker 及
`[gui] terminal command executed name=uptime output=true`；无 `[panic]` 或 `DMA fault`。
QMP `query-status` pre/post 均 running，`input-alt-tab`、Terminal tablet click、
`uptime`+Return 均返回成功。解码截图为 1024x768 RGB，QEMU P6 PPM 已归一化 post PNG；
CRC `pre=736b653e`、`post=55e29f5a`、`changed=true`。证据：
`target/gui-terminal-owned-qemu.log`、`target/gui-terminal-owned-qmp.jsonl`、
`target/gui-terminal-owned-pre.png`、`target/gui-terminal-owned-post.png`；无残留 QEMU。

### SSH vertical-slice build/audit (2026-08-11, first-error stop)

OrbStack context=`orbstack`；按要求仅执行一次 `make build`，未运行 full test/fsck。
构建在 `services/sshd/src/main.rs:55-56` 首次失败（exit=2）：smoltcp 0.13 的
`tcp::SocketBuffer::new` 需要 `&mut [u8]`，当前传入 `&mut [u8; 16384]` 不满足
`ManagedSlice` trait bound；编译器建议显式切片 coercion。错误发生于 sshd 链接前，未
进行 QEMU 启动。

只读 ABI/cap 复核：`crates/abi/src/lib.rs` 的 `ObjectType::NetworkDevice=13`、
`RandomSource=14`，`Syscall::NetReceive=27`、`NetSend=28`、`RandomFill=29`；
`boot_cap::NETWORK_DEVICE=slot58/gen1`、`RANDOM_SOURCE=slot59/gen1`。user-rt
wrapper 将网络帧限制为 1536 bytes、随机填充限制为 256 bytes；
`service_runtime.rs` 仅 SSHD task index 10 可调用 27..29，并检查 capability 类型、
READ/WRITE 权限及用户地址范围；SSHD 根 cap 为网络 R/W、随机源 R。

依赖纵向切片：`services/sshd/Cargo.toml` 使用 `embedded-io-async=0.6`、
`rand=0.8`、`smoltcp=0.13.1`（medium-ethernet/proto-ipv4/socket-tcp 及 iface
计数限制）和 `zssh=0.4.2`（default-features=false）。QEMU 参数在
`xtask/src/main.rs:357-364,421-428` 配置 user net `hostfwd=tcp:<bind>:2222-10.0.2.15:22`、
virtio-net PCI slot 6，以及 `/dev/urandom` backing 的 virtio-rng PCI slot 7；kernel
对应 `[net] virtio-net ready ...` 与 `[rng] virtio-rng ready entropy-source=host`
标记位于 `crates/kernel/src/net.rs:90`、`random.rs:76`。

静态边界：SSH 加入 bootfs 后，xtask service 列表为 21 个 ELF 加 `etc/services`，故
当前启动串口将是 `entries=22/static-elfs=21`；`SERVICE_COUNT=11` 意味着 resident
address spaces `0x20..0x2a`。现有 smoke 的 21/20 与 10-slot 断言需后续同步，本轮
未修改脚本或生产代码。

### SSH build recheck after TCP buffer slice fix (2026-08-11, first-error stop)

OrbStack context=`orbstack`；按要求仅重跑一次 `make build`，未运行 test/fsck。上一轮
`SocketBuffer::new` 数组到切片的错误已消失；当前首个阻塞转为
`services/sshd/src/main.rs:71-85` 的 8 个 E0499/E0502 借用检查错误：
`sockets.get(...).is_active()` 的不可变借用与 `interface.poll(..., &mut sockets)` 的可变
借用重叠；`TcpStream`/`Transport` 保留 `interface`、`device`、`sockets` 的可变借用，
随后 `sockets.get_mut` 执行 abort/listen 再次冲突。`microsystem-sshd` 无法编译，cargo
退出 101，xtask/make 最终退出 2；未启动 QEMU，也未执行 test/fsck。

### SSH build recheck after SocketStorage lifetime fix (2026-08-11, first-error stop)

OrbStack context=`orbstack`；按要求第三次仅执行 `make build`，未运行 test/fsck。上一轮
借用检查错误已消失；当前首个阻塞为 `services/sshd/src/main.rs:58-62` 的 3 个
`dangerous_implicit_autorefs` 错误：对 `*mut [u8; 16384]` 和
`*mut [SocketStorage; 1]` 做 `[..]` 切片时，Rust 拒绝隐式创建引用（`#[deny(dangerous_implicit_autorefs)]`）。
编译器建议显式增加 `&mut` 层级。`microsystem-sshd` 无法编译，cargo/xtask 退出 101，
`make build` 退出 2；未启动 QEMU，也未执行 test/fsck。

### SSH build recheck after explicit raw-pointer slices (2026-08-11, PASS)

OrbStack context=`orbstack`；第四次仅执行 `make build`，未运行 test/fsck。显式
`&mut *ptr` 绑定后，`microsystem-sshd` release build、baremetal kernel、mfsctl 及
boot image font staging 均完成；仅有 kernel `invalidate_user_page` dead-code warning。
`cargo/xtask` 与 `make build` 均退出 0；未启动 QEMU，也未执行 test/fsck。

### SSH targeted gate (2026-08-11, first-error stop)

仅修改测试脚本：新增 scripts/ssh-qemu.sh，OrbStack 内用 QEMU
hostfwd=tcp:127.0.0.1:2223-10.0.2.15:22、virtio-net PCI slot 6、virtio-rng
/dev/urandom slot 7；OpenSSH 使用 build/ssh/id_ed25519 执行 uptime，并准备
错误 key/无 key 认证失败断言。同步 smoke-qemu.sh 为 bootfs 22/21、resident
ASID 11/[0x20..0x2a]、非 GUI ready 7/7、FIRST_APPLICATION_PID 12；
gui-qemu.sh 加入 GUI ready 11/11 及相同 bootfs/ASID/首 PID marker。所有脚本
bash -n 通过。

首次 targeted gate 的首错是测试正则把正常 [iommu] ... gerror=0x0 误判为 fatal；
修正为仅匹配非零 gerror=0x... 后重跑一次。第二次串口已完整到达：
[bootfs] valid=true entries=22 static-elfs=21、resident
address-spaces=11 asids=[0x20..0x2a]、[net] virtio-net ready ...、
[rng] virtio-rng ready entropy-source=host、[ssh] sshd ready address=10.0.2.15
port=22 auth=publickey user=micro，无 panic/DMA/translation fault。首个实际门禁失败为
缺少 [service] resident EL0 ready=7/7 online=7/7 switches=...；日志到达
[service] shell ready 后仍未输出该 readiness line，故按首错停止，未发起 OpenSSH
authorized/wrong/no-key 请求。日志：target/ssh-qemu.log；本轮未运行 full test/fsck。

### SSH service-mask recheck (2026-08-11, first-error stop)

bash -n 及 marker 静态检查通过。OrbStack make build 单次成功（exit=0；仅
invalidate_user_page dead-code warning），未运行 full test/fsck。随后单次
scripts/ssh-qemu.sh 使用 SSH_PORT=2223、GUI_PORT=5901：串口确认
[bootfs] valid=true entries=22 static-elfs=21、resident
address-spaces=11 asids=[0x20..0x2a]、[service] resident EL0 ready=7/7
online=7/7 switches=2401584、[net] virtio-net ready、[rng] virtio-rng
ready 与 [ssh] sshd ready address=10.0.2.15 port=22，无 panic/DMA/translation
fault。实际首错为正确 key OpenSSH uptime 在 banner exchange 超时，exit=255
(Connection timed out during banner exchange)；因此按首错停止，wrong-key/no-key
分支未执行。脚本清理后无残留 QEMU/SSH 进程。日志：target/ssh-qemu-maskfix.log。

### SSH VirtIO-Net MRG_RXBUF recheck (2026-08-11, first-error stop)

bash -n 通过；OrbStack context=orbstack 的单次 make build 成功（exit=0，仅
kernel invalidate_user_page dead-code warning），未运行 full test/fsck。随后唯一
SSH gate 使用 2223/5901，串口确认 bootfs 22/21、resident ASID 11
[0x20..0x2a]、ready 7/7 online=7/7 switches=2693453、net/rng/sshd ready，且无
panic/DMA/translation fault。正确 key OpenSSH uptime 首先失败：
Connection timed out during banner exchange，SSH exit=255；因此按首错停止，
wrong-key/no-key 未执行。输出文件 target/ssh-uptime.out 为空，错误为
target/ssh-uptime.err；gate 日志 target/ssh-qemu-netfix.log，无残留 QEMU/SSH
进程。

### SSH network pcap diagnosis (2026-08-11, first-error stop)

仅修改 scripts/ssh-qemu.sh 增加 filter-dump pcap 和
MICROSYSTEM_SSH_DIAGNOSTIC=1 单连接模式；bash -n 通过，未运行 full test/fsck。
单次诊断使用正确 key 连接直至 8 秒 banner timeout，SSH exit=255；串口命中
bootfs22/21、ASID11、ready7/7 online=7/7 switches=2441117、net/rng/sshd ready，
无 panic/DMA/translation fault。pcap target/ssh-netdiag.pcap 大小 82B，tcpdump
EN10MB 仅见 1 个方向帧：ARP Request who-has 10.0.2.15 tell 10.0.2.2（slirp/host
向 guest）；guest→host ARP response=0、TCP frames/responses=0。ssh-uptime.out
为空，错误为 banner exchange timeout；无残留 QEMU/SSH 进程。该证据表明连接在
guest ARP 响应前失败，未进入 SSH key authentication。

### SSH VirtIO-Net queue trace diagnosis (2026-08-11, first-error stop)

仅在测试脚本中增加 QEMU trace events（virtio_queue_notify、virtqueue_pop、
virtqueue_fill、virtqueue_flush），未运行 full test/fsck。单次正确 key diagnostic
连接仍 banner timeout，exit=255；保留 target/ssh-nettrace.pcap 与
target/ssh-nettrace.trace。串口命中 [net] virtio-net ready、[rng] virtio-rng
ready、[ssh] sshd ready、resident ready=7/7，且无 fatal fault。

trace 统计：virtio-net 两队列 vdev（trace pointer 0xaaaaf6816cd0）仅收到
queue notify：RX n=0 两次、TX n=1 一次；对应 RX/TX virtqueue 均无 pop/fill/flush。
virtio-blk vdev 0xaaaaf66f8010 的 n=0 有 76,508 次 notify，queue
0xffffacea2010 有 76,507 次 pop/fill/flush（len=513 为块 I/O）；virtio-rng
vdev 0xaaaaf693a7d0 有 2 次 notify、1 次 pop/fill/flush。pcap 仍仅 1 个
slirp→guest ARP request（who-has 10.0.2.15 tell 10.0.2.2），无 guest→host
ARP/TCP 响应。由此确认网络 RX descriptor 未被 pop/write-used，guest 也未发 TX
ARP；无残留 QEMU/SSH 进程。

### SSH QMP VirtIO queue status recheck (2026-08-11, first-error stop)

修正 QMP name/path 解析后按批准仅重跑一次 diagnostic；未运行 full test/fsck。QMP
找到 net path /machine/peripheral-anon/device[1]/virtio-backend。pre/post
query-status 均 running；x-query-virtio-status 显示 guest features 只有
VIRTIO_F_IOMMU_PLATFORM、VIRTIO_F_VERSION_1，status 为 FEATURES_OK/DRIVER_OK，
num-vqs=3、started=true。pre isr=0，post isr=1。

net queue 状态（pre → post）：

- queue0：inuse 0→0，last-avail-idx 0→1，shadow-avail-idx 0→1，
  used-idx 0→1，vring-num=1，desc/avail/used=5255168/5255424/5255488。
- queue1：inuse=0、last/shadow/used 均保持 0，vring-num=1，
  desc/avail/used=5259264/5259520/5259584。

正确 key SSH 仍 banner exchange timeout，exit=255；pcap
target/ssh-qmp2.pcap 仅 1 个 slirp→guest ARP request、无 guest→host ARP/TCP；
wrong/no-key 未执行（diagnostic 首错停止），无残留 QEMU/SSH。QMP 原始证据：
target/ssh-qmp2.jsonl；此前 harness parser 首错已记录。

### SSH first-rx/first-tx safety diagnostic (2026-08-11, build first-error stop)

静态 bash-n 及 first-rx/first-tx marker 检查通过；OrbStack context=orbstack。
按要求仅启动一次 make build，但在 build-image Dockerfile syntax image metadata
阶段首错停止：请求 docker.io/docker/dockerfile:1.7 返回 502 Bad Gateway，
make exit=2（failed to resolve source metadata）。未启动 SSH/QEMU gate，故本轮没有
first-rx/first-tx、正确 key、wrong/no-key、pcap 或残留进程证据；未运行 full
test/fsck。

### GUI input DMA/Alt-Tab recheck after SMMU STRTAB fix (2026-08-11)

kernel ELF 已在 `crates/kernel/src/smmu.rs` 的 STRTAB 配置修复后刷新（stream table
`LOG2SIZE=6`）。未运行完整测试，仅执行一次 OrbStack `scripts/gui-qemu.sh`：input-ready、
QMP `query-status` pre/post=`running`、`input-send-event`、Alt-Tab routed marker 均通过，
且无 DMA fault。QMP/screenshot 证据：

    pre_crc32=1b6d4317  post_crc32=fd39440b  changed=true
    post PNG=1024x768 RGB bytes=6356 colors_sampled=2 (source_format=PPM-P6)

串口命中 `[input] virtio-keyboard+tablet ready ...`、`[gui] keyboard+tablet input
routed focus=true drag=true alt-tab=true` 及 windowd/三个窗口/present markers；无残留
QEMU/container。pre screendump 保留为 QEMU P6 PPM，post 已归一化为 PNG。

### SSH first-rx/first-tx safety diagnostic (2026-08-11, direct cached image)

静态 bash-n 通过；OrbStack context=orbstack，复用现有
microsystem-dev:rust-1.97.1 image digest
sha256:cf26efbb29adb236aac7d489cf66d10b03dc731d64fd7f464ef17a8bf307e248，
直接执行容器内 cargo run -p xtask -- build，exit=0；未调用 build-image，未运行
full test/fsck。随后只运行一次 SSH gate（SSH_PORT=2223、GUI_PORT=5901），正确
key uptime 首错 exit=255，Connection timed out during banner exchange；按首错
停止，wrong-key/no-key 未执行。

串口命中 bootfs valid entries=22/static-elfs=21、resident address-spaces=11
[0x20..0x2a]、resident ready=7/7 online=7/7、net/rng/sshd ready，且无
panic/DMA/translation fault。首包 marker 原样为：

    [net] first-rx bytes=42 virtio=0000 ethernet=ffffffffffff ether-type=0806
    [net] first-tx bytes=42 ethernet=52550a000202 ether-type=0806

first-rx 与 micro> 同行仅为 UART 交错。pcap target/ssh-first-packet.pcap
（约2.2K）显示 host/slirp ARP request who-has 10.0.2.15 后 guest ARP reply
10.0.2.15 is-at 52:54:00:12:34:56；随后仅有 10.0.2.2→10.0.2.15:22 TCP
SYN 重传，无 SYN/ACK 或 SSH banner。脚本清理后无残留 QEMU/SSH 进程。

### SSH split-queue modulo fix targeted gate (2026-08-11)

静态 bash-n 通过；OrbStack context=orbstack，复用现有
microsystem-dev:rust-1.97.1 image digest
sha256:cf26efbb29adb236aac7d489cf66d10b03dc731d64fd7f464ef17a8bf307e248。
先尝试 CARGO_NET_OFFLINE=true/cargo --offline 的 direct xtask build，因缓存缺少
fdt（required by crates/kernel）exit=101；未启动 QEMU。随后按批准仍使用同一镜像
进行唯一 online direct cargo run -p xtask -- build（未调用 build-image），exit=0，
仅有 invalidate_user_page dead-code warning。

新 ELF 后仅运行一次 scripts/ssh-qemu.sh（SSH_PORT=2223、GUI_PORT=5901）：
正确 key uptime 成功 exit=0，输出 uptime: 17056 ms；wrong-key 拒绝 exit=255，
no-key 拒绝 exit=255。串口命中 bootfs22/21、resident ASID11、ready=7/7
online=7/7、net/rng/sshd ready，且无 panic/DMA/translation fault。first packet
marker 原样为：

    [net] first-rx bytes=42 virtio=0000 ethernet=ffffffffffff ether-type=0806
    [net] first-tx bytes=42 ethernet=52550a000202 ether-type=0806

（两行因 UART 调度分别邻近 shell 输出，marker 内容完整）。pcap
target/ssh-queue-modulo.pcap 大小约6.6K，共49帧：ARP request/reply、TCP
SYN/SYN-ACK/ACK及 SSH 数据双向均存在。错误输出分别保留在
target/ssh-queue-modulo.bad.out 与 target/ssh-queue-modulo.nokey.out；脚本
清理后无残留 QEMU/SSH 进程。未运行 full test/fsck。
### Mica crate behavior boundary tests (2026-08-11)

仅新增 crates/mica/tests/core.rs，未改生产代码、QEMU 脚本或 ABI。OrbStack
microsystem-dev:rust-1.97.1 容器内执行 cargo test -p microsystem-mica；
首次等待 crates.io index 后完成，exit=0。结果：crate unit 0、Mica integration
8/8、doc-tests 0，无失败。

覆盖边界：compile/Chunk、算术/局部变量/if/while/for/table、函数闭包与多返回、
未声明赋值编译拒绝、Host stdlib、指令与时钟超时；PermissionSet 路径规范化、
主机规则及三方交集；JSON round-trip/重复键/深度限制；hex/base64/UTF-8、
SHA-256/HMAC 稳定向量。未运行完整 QEMU、make test 或 fsck。

### Mica EL0 QEMU targeted gate (2026-08-11, first-error stop)

新增 `scripts/mica-qemu.sh`（仅测试脚本），静态 `bash -n` 通过；门禁同步当前
bootfs `entries=24/static-elfs=23`、resident address-spaces=12（ASID
`0x20..0x2b`）、serial service ready `8/8`、动态应用 first-pid=13，要求
`mica -e 'print(40 + 2)'` 的独立输出 `42`、`mica: pid=13 status=0` 以及普通
`run mica: not found`。

OrbStack `make build` exit=0，已重建 Mica/SSHD/kernel 与 bootfs。首次 targeted
启动曾因残留 GUI 容器 `a58c9f444bc1` 持有 `build/microsystem.img` 写锁而停止，
`target/mica-qemu-targeted.log` 保留该 harness 首错；清理后脚本改用已构建的
`target/debug/xtask`（避免 QEMU gate 在 crates.io index 等待）并通过
`target/mica-qemu-targeted-final2.log`：bootfs24/23、first-pid13、Mica 独立
输出 `42`、`mica: pid=13 status=0`、普通 `run mica: not found`，exit=0。

随后唯一 SSH targeted gate 使用端口 2223/5901，`target/mica-ssh-qemu.log`：
正确 key uptime exit=0（`uptime: 14720 ms`），wrong-key/no-key 均 exit=255；
串口命中 `[net] netd ready ipv4=10.0.2.15 outbound=true raw-device-isolated=true`、
bootfs24/23、ASID12、ready8/8、sshd ready、first-rx/first-tx，无 panic/DMA/
translation fault。pcap `target/mica-ssh.pcap` 约14K；脚本清理后无项目 QEMU/
docker 残留。未运行 full make test/fsck。

### Mica combo gate after script-Frame mapping fix (2026-08-12, first-error stop)

最新 OrbStack `make build` exit=0（仅 kernel `invalidate_user_page` dead-code
warning）；使用当前 `IMAGE_BYTES=0x80000`/netd EL0 UDP heap ELF。静态
`bash -n scripts/mica-qemu.sh` 通过。此前 `target/mica-combo-targeted-final4.log`
的 `status=-2` 已由生产侧将 MFS/netd temporary FrameRegion 加入动态映射
allowlist 修复；本轮最新唯一 gate 日志为
`target/mica-combo-targeted-final5.log`。

本轮 bootfs24/23、TaskMemory `slot-bytes=0x19c000 allocated=0x19c000`、
resident ready=8/8、netd/sshd/MFS/shell ready，以及 `[mica] session` 和
`[mica] vm verified` 均出现；但首个 `mica -e 'print(40 + 2)'` 在 vm marker
后无 `42`，shell 未返回后续组合命令，脚本 90s 有界超时并以 exit=1 停止：
`mica-qemu: guest did not reach shutdown within 90s`，QEMU 收到 SIGTERM。
未见 panic、DMA/translation fault；fixture、QEMU 容器和镜像均由 trap 清理。
未运行 full make test/fsck；Mica combo gate 当前 BLOCKED 于 EL0 Mica
launch/return hang。

### Mica print/VM completion regression (2026-08-12)

Added one test-only regression in `crates/mica/tests/core.rs`: a TestHost records
`print` arguments, then `print(40 + 2); return 7` must complete, preserve the
following return, and record integer `42`. OrbStack
`cargo test -p microsystem-mica` passed: integration 13/13, unit 0, doc-tests 0;
no QEMU/full test/fsck run in this slice.

### Mica print-dispatch targeted diagnostic (2026-08-12)

After a cached OrbStack `make build` (exit=0; only kernel dead-code warning), the
single `scripts/mica-qemu.sh` run used `target/mica-combo-targeted-final6.log`.
The log reached `[mica] session`, `vm verified`, `clock ready=true`, and
`print dispatch=true`; no `42`, combo result, PID status, or shutdown followed.
The bounded gate stopped at 90 seconds with exit=1 and
`mica-qemu: guest did not reach shutdown within 90s`; QEMU was terminated by the
timeout. No panic, DMA, or translation fault appeared, and cleanup removed the
fixture/container. This narrows the Mica EL0 hang to after Host `print` dispatch;
full test/fsck were not run.

### Mica exit-boundary targeted diagnostic (2026-08-12)

After another OrbStack `make build` exit=0, the single gate log
`target/mica-combo-targeted-final7.log` reached every temporary boundary marker:
`session` → `vm verified` → `clock ready=true` → `print dispatch=true` →
`print formatted=true` → `print buffered=true` → `vm returned=true` →
`exiting=true`. It still emitted no `42`, PID/status, combo marker, prompt, or
shutdown. The 90-second bounded run exited 1 with
`guest did not reach shutdown within 90s`; no panic/DMA/translation fault was
seen and cleanup completed. The remaining hang is after Mica `_start` exit,
in process termination/IPC wait or shell session-output handling; no full
test/fsck was run.

### Mica kernel-exit/init-wait targeted diagnostic (2026-08-12)

Latest OrbStack `make build` exit=0 and one gate run produced
`target/mica-combo-targeted-final8.log`. The sequence reached Mica session/vm,
clock, print dispatch/formatted/buffered, vm returned, exiting, then kernel
`exit entered pid=13`, `exit reclaimed pid=13`, and `exit scheduled pid=13`.
No init `wait cleanup entered/returned` or notification/network/filesystem cleanup
marker appeared, and shell emitted no `42`, PID status, combo, prompt, or
shutdown. The gate timed out at 90s (exit=1), without panic/DMA/translation
fault; cleanup completed. This localizes the remaining stall to the init process
endpoint Wait/launch-reply path after kernel scheduling; full test/fsck was not run.

### Mica launch/wait IPC targeted diagnostic (2026-08-12)

Latest build exit=0; `target/mica-combo-targeted-final9.log` shows six successful
shell/init Wait request-reply Busy polls (lines 126–145). The shell issued a
seventh Wait request immediately before Mica print; Mica then reached all VM and
kernel exit markers through `kernel exit scheduled pid=13`, but no matching init
Wait marker or shell Wait reply followed. The bounded gate exited 1 after 90s
without `42`, status, combo, or shutdown. No panic/DMA/translation fault;
cleanup completed. This is a pending process-endpoint Wait delivery/reply stall
concurrent with application exit; full test/fsck was not run.

### Mica endpoint-batch fix and combo network diagnostic (2026-08-12)

After the init endpoint pending-request fix, OrbStack `make build` exit=0 and
the first inline gate completed: `42`, `mica: pid=13 status=0`, and cleanup
markers all appeared. The same bounded run then launched the file combo; its
Mica vm/clock markers appeared, followed by network first-rx/first-tx, but no
print/combo marker. `target/mica-combo-targeted-final10.log` timed out at 90s
(exit=1), with no panic/DMA/translation fault. The combo stalls at its first
`net.resolve("example.com")` operation; the test fixture currently serves
HTTP/TCP/UDP only and QEMU restricted networking has no local DNS responder.
No full test/fsck was run.

### Mica deterministic DNS alias and notification regression (2026-08-12)

The Mica fixture gate now adds `10.0.2.2 mica.test` to the OrbStack fixture
container's `/etc/hosts`; generated manifest, launcher input, and
`net.resolve` use `mica.test:53` while retaining QEMU's built-in `10.0.2.3`
DNS proxy. This removes the prior `example.com` external-DNS dependency.
`bash -n scripts/mica-qemu.sh` passed. A fresh OrbStack `make build` exited 0
(only the existing kernel `invalidate_user_page` dead-code warning).

The single post-build gate `target/mica-combo-targeted-final11.log` exited 1:
all boot/resident/device/netd/sshd/MFS/shell markers through `ready=8/8`
appeared, then the first `mica -e 'print(40 + 2)'` produced
`[mica] session ...` followed immediately by
`[service] critical service init exited status=4`. No Mica VM/clock/print/42,
DNS, combo, or shutdown marker followed. No panic, DMA, translation, or
allocator fault was present; cleanup completed. This run therefore stopped at
the new init notification/launcher regression before exercising deterministic
DNS, and full test/fsck were not run.

### Mica DNS fixture after notification-transfer fix (2026-08-12)

After the production notification transfer fix, a fresh OrbStack `make build`
exited 0 (kernel emitted only the existing `invalidate_user_page` dead-code
warning). The single DNS-alias gate used `mica.test` and completed the inline
Mica check (`42`, `mica: pid=13 status=0`) plus all boot/resident/device/netd,
MFS, shell, VM, and process-cleanup markers.

The file-combo command reached `[mica] session`, VM verification, and clock
ready, then emitted only QEMU ARP first-tx/first-rx evidence. It returned and
cleaned up with `mica: pid=13 status=1`; no combo, DNS, TCP, UDP, or HTTP
success marker appeared. The shell then accepted `run mica` as not found and
`shutdown` completed. `target/mica-combo-targeted-final12.log` is the raw log;
the script exited 1. No panic, DMA, translation, allocator fault, or residual
QEMU process was observed. The deterministic `/etc/hosts` alias did not yet
make the guest DNS/network combo pass; full test/fsck were not run.

### Mica DNS runtime error after notification fix (2026-08-12)

The next cached OrbStack `make build` exited 0 (only the existing kernel
dead-code warning). In the one targeted gate, inline Mica again passed with
`42` and `mica: pid=13 status=0`. The combo reached session/VM/clock markers and
QEMU first-tx/first-rx ARP evidence, then shell emitted the exact runtime
diagnostic `runtime error: dns resolve`; it exited as `mica: pid=13 status=1`.
No TCP/UDP/HTTP/combo marker appeared. `run mica` was rejected and shutdown
completed; no panic, DMA, translation, allocator fault, or residual QEMU was
observed. Raw log: `target/mica-combo-targeted-final13.log`. Full test/fsck
were not run.

### Mica HTTPS/CA/RandomSource build check (2026-08-12)

`bash -n scripts/*.sh` passed. OrbStack `make build` stopped at the first
production compile failure while building `microsystem-mica-service` (exit 2;
QEMU/full test/fsck were not run). Exact diagnostics:

- `services/mica/src/tls.rs:10`: `embedded_tls::config` is private in
  `embedded-tls 0.19.0`; `Certificate`, `CertificateEntryRef`, and
  `CertificateRef` are not publicly re-exported from that module.
- `services/mica/src/tls.rs:283`: `embedded_tls::config::CertificateVerifyRef` is
  likewise inaccessible.
- `services/mica/src/tls.rs:161,168`: `NetworkError` does not implement
  `core::error::Error`, required by `embedded_io 0.7.1::Error` and
  `ErrorType::Error`.

The build did not reach CA-bundle generation or image embedding, so no bundle
count/hash evidence exists for this attempt.

### Mica HTTPS/CA/RandomSource rebuild (2026-08-12)

After the embedded-tls public re-export and embedded-io Error fixes, OrbStack
`make build` completed successfully (exit 0; only existing sshd/kernel
warnings). The build generated one CA bundle artifact:
`build/ca-bundle.derpack`, 129,643 bytes, SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`. The
xtask build log confirms MFS embedding:
`mfsctl put build/ca-bundle.derpack /.system/certs/ca-bundle.derpack`,
`wrote 129643 bytes`. No QEMU/full test/fsck was run.

### SSH disconnect cleanup and authentication gate (2026-08-12)

After the CloseWait/SshStatus fix, `bash -n scripts/ssh-qemu.sh` and OrbStack
`make build` passed (exit 0; only existing warnings). The single targeted run
`target/ssh-mica-targeted-final15.log` passed all SSH checks:

- uptime `uptime: 23134 ms`;
- Mica eval status 0, output `42`;
- interactive REPL status 0, output `42` and final `micro>`;
- authorized file output `ssh-mica-file=ssh-mica-ok`;
- denied policy status 0, output `ssh-mica-denied kind=access`;
- intentional disconnect exit 124, followed by successful uptime recovery;
- wrong-key and no-key both rejected (exit 255).

The script reported `ssh-qemu: PASS`; serial showed Mica cleanup/reclaim
markers after disconnect, with no panic, DMA/translation/allocator fault, or
residual QEMU. Full test/fsck were not run.

### SSH policy probe length fix and disconnect recovery (2026-08-12)

The denied-policy probe was shortened to 172 bytes (<256) with a static
length guard; `bash -n` passed. Reusing the fresh final13 build, the single
targeted run `target/ssh-mica-targeted-final14.log` passed uptime, Mica eval
(`42`), complete REPL, authorized file script, and denied-policy marker
`ssh-mica-denied kind=access`. It then failed recovery after the intentional
long-script disconnect: exit 255, `Connection timed out during banner
exchange`. Serial shows the long Mica session remained in repeated init-wait
cleanup markers without an exit/reclaim marker, so disconnect cleanup did not
release the SSH listener before recovery. Wrong/no-key probes were not
reached. No panic, DMA/translation/allocator fault, or residual QEMU was
observed; full test/fsck were not run.

### SSH filesystem-frame handle retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` and OrbStack `make build` passed (exit 0; only
existing warnings). The single targeted run
`target/ssh-mica-targeted-final13.log` passed uptime, Mica eval (`42`), complete
REPL, and the authorized `/data` file script (`ssh-mica-file=ssh-mica-ok`). It
stopped at the denied-policy probe with exit 127 and `unknown command`. This
is a test-harness command-length boundary: the generated SSH command is 265
bytes, while the production shell parser accepts Mica argument commands only
through 256 bytes, so the command is rejected before policy evaluation. No
production change was made and no rerun was attempted. Disconnect,
wrong-key/no-key probes were not reached; no panic/fault or residual QEMU was
observed; full test/fsck were not run.

### SSH policy status retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` and OrbStack `make build` passed (exit 0; only
existing warnings). The single targeted run
`target/ssh-mica-targeted-final12.log` passed uptime, Mica eval (`42`), and the
complete REPL, then stopped at the filesystem script with exit 2. The exact
returned user error is `mica: SSH policy is unavailable (access denied)`.
Serial showed both Mica sessions' cleanup markers and no panic/fault. Deny,
disconnect, wrong-key, and no-key probes were not reached; no residual QEMU
was observed and full test/fsck were not run.

### SSH policy snapshot retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` and OrbStack `make build` passed (exit 0; only
existing warnings). The one targeted run
`target/ssh-mica-targeted-final11.log` again passed uptime, Mica eval (`42`),
and the complete interactive REPL, then stopped at filesystem execution with
exit 2: `mica: SSH policy is unavailable`. Serial showed both Mica sessions'
cleanup markers; no panic or fault was emitted. Deny/disconnect/wrong/no-key
checks were not reached, and no residual QEMU was observed; full test/fsck
were not run.

### SSH dual-listener/SessionHeader retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` and OrbStack `make build` passed (build exit 0;
only existing warnings). The single targeted run
`target/ssh-mica-targeted-final10.log` passed authorized uptime, Mica eval
(`42`), and the complete interactive REPL (`Mica 0.1`, `print(6 * 7)`, `42`,
`exit`, final `micro>`). It then stopped at the filesystem-script gate: exit 2
with `mica: SSH policy is unavailable`. Serial contained both Mica cleanup
sessions and no fault/panic markers. Deny, disconnect, wrong-key, and no-key
probes were not reached. No DMA/translation/allocator fault or residual QEMU
was observed; full test/fsck were not run.

### SSH SessionHeader flag retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` and OrbStack `make build` passed (build exit 0;
only existing warnings). The single targeted run
`target/ssh-mica-targeted-final9.log` stopped at the authorized Mica eval,
exit 255: `kex_exchange_identification: read: Connection reset by peer`.
Serial reached SSH transport markers and the shell prompt, but no Mica session
or VM markers were emitted and `target/ssh-mica-eval.out` was empty. REPL,
filesystem/policy, disconnect, and wrong/no-key probes were not reached. No
panic, DMA/translation/allocator fault, or residual QEMU was observed; full
test/fsck were not run.

### Mica TLS gate after direct-IP warm-up and exact TLS host policy (2026-08-12)

The harness statically passed `bash -n`. Every HTTPS URL uses `mica.test` and
the manifest policy, launcher `--allow` list, and startup policy each grant
the same five exact `mica.test:<port>` destinations (valid, unknown-CA,
expired, not-yet-valid, and hostname-mismatch). The prior direct-IP TCP warm-up
remains before the real DNS query; no build/full test/fsck was run in this
iteration.

The one bounded OrbStack gate reached the resident chain, Mica eval and REPL
(both printed `42` and returned status 0), and the combo launch. The first
failure was:
`mica network error stage=tcp-connect message=system operation timed out
kind=timeout operation=net code=-7`.
No DNS fixture query was persisted (`target/mica-dns-fixture.log` is empty),
and no TLS success/failure marker or HTTP response was reached; this run also
emitted no `[net] first-tx`/`first-rx` line before the TCP timeout. The harness
exited 1 after the bounded combo/shutdown wait;
raw serial log: `target/mica-tls-targeted-final10.log`.

No panic, DMA, translation, allocator, or QEMU fault appeared. Cleanup
restored the production disk and artifacts; `build/ca-bundle.derpack` is
129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`, and no
fixture directory, QEMU process, or Docker test container remains.

### SSH close/FIN retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` passed and OrbStack `make build` exited 0 (only
existing warnings). The single targeted run
`target/ssh-mica-targeted-final8.log` stopped at the REPL marker check. The
client received `MicroSystem SSH`, `micro> mica`, then only shell prompts for
`print(6 * 7)` and `exit`; the Mica banner and `42` were absent, so the script
reported `SSH Mica REPL markers missing`. Serial reached the second
`[mica] session` marker but no VM/print/exit markers after it. Inline eval still
returned `42`; file/deny/disconnect/wrong/no-key checks were not reached. No
panic, DMA/translation/allocator fault, or residual QEMU was observed; full
test/fsck were not run.

### SSH established-state gate retest (2026-08-12)

`bash -n scripts/ssh-qemu.sh` passed and OrbStack `make build` exited 0 (only
existing warnings). The single targeted run
`target/ssh-mica-targeted-final7.log` reached a complete interactive Mica REPL
transcript (`MicroSystem SSH`, `mica`, `print(6 * 7)`, `42`, `exit`, final
`micro>`) and serial recorded both Mica sessions' cleanup markers. However,
the SSH client returned exit 255 with `Connection to 127.0.0.1 closed by
remote host`, so the gate stopped before file-policy, deny, disconnect, or
wrong/no-key checks. Inline eval still output `42`. No panic, DMA, translation,
allocator, or residual QEMU was observed; full test/fsck were not run.

### SSH packet reset retest (2026-08-12)

After clearing `SSH_PACKET` before each transport, OrbStack `make build`
completed with exit 0 (only existing warnings). The one targeted run
`target/ssh-mica-targeted-final6.log` stopped at the first follow-up REPL
connection, exit 255: `kex_exchange_identification: Connection closed by
remote host`. The first authorized Mica eval still produced `42`, and serial
reached transport first-read/first-write/random plus Mica VM/clock/print/exit
and wait-cleanup markers. No second-session Mica marker appeared; file/deny/
disconnect/wrong-key/no-key checks were not reached. No panic, DMA, translation,
allocator, or residual QEMU was observed; full test/fsck were not run.

### SSH transport lifecycle retest (2026-08-12)

After the sshd per-channel close/listen lifecycle fix, `make build` completed
successfully in OrbStack (exit 0; only existing unused-import/dead-code
warnings). The single targeted run `target/ssh-mica-targeted-final5.log`
stopped at the first follow-up REPL connection with exit 255:
`ssh_dispatch_run_fatal: Connection to 127.0.0.1 port 2223: incorrect signature`.
The first authorized Mica eval still passed (`target/ssh-mica-eval.out` =
`42`), and serial reached transport first-read/first-write/random plus Mica
VM/clock/print/exit and wait-cleanup markers. The second session emitted no
Mica markers; file/deny/disconnect/wrong-key/no-key probes were not reached.
No panic, DMA/translation/allocator fault, or residual QEMU process was
observed; full test/fsck were not run.

### SSH Mica REPL race retest (2026-08-12)

After the SSH REPL wait/read race fix, `bash -n scripts/ssh-qemu.sh` and
OrbStack `make build` both passed (build exit 0; only existing unused-import
and dead-code warnings). The single targeted run
`target/ssh-mica-targeted-final4.log` exited 1 at the first follow-up SSH
connection: `ssh-qemu: SSH Mica REPL failed exit=255` and
`kex_exchange_identification: read: Connection reset by peer`. The authorized
inline eval completed (`target/ssh-mica-eval.out` is `42`), and serial reached
all three transport markers (`first-write=true`, `first-read=true`,
`random ready=true`) plus the first Mica VM/clock/print/exit and wait-cleanup
markers. The REPL/file/deny/disconnect/wrong-key/no-key probes were not
reached (their output files are empty). No panic, DMA, translation, allocator,
or residual QEMU process was observed; full test/fsck were not run.

### SSH Mica focused gate: build blocked before QEMU (2026-08-12)

Extended `scripts/ssh-qemu.sh` only (no production changes) with authorized-key
Mica eval (`print(40 + 2)`), interactive SSH REPL (`mica` → `print(6 * 7)` →
`exit`), an MFS `/data/ssh-mica.mica` payload constrained by
`/.system/ssh/mica-policy`, a denied-policy probe, and a bounded long-script
disconnect followed by an uptime recovery check. Existing wrong-key and no-key
rejection checks remain. `bash -n scripts/ssh-qemu.sh` passed.

The required OrbStack `make build` then exited 2 before any SSH/QEMU run. The
first compiler error was `E0463: can't find crate for std` while building
`microsystem-sshd`'s `microsystem-mica` dependency for
`aarch64-unknown-none-softfloat`; the dependency currently enables
`microsystem-mica`'s default `std` feature. Follow-on errors were no-std
prelude/derive/matches/Result failures (about 483 errors). Targeted SSH, wrong
key, Mica REPL, filesystem policy, disconnect recovery, and full test/fsck were
not run because the fresh build failed.

### SSH Mica focused gate after no-std dependency fix (2026-08-12)

After `microsystem-sshd` disabled the default `std` feature on its
`microsystem-mica` dependency, `bash -n scripts/ssh-qemu.sh` and one OrbStack
`make build` completed successfully (only existing unused-import and kernel
dead-code warnings). The single targeted `scripts/ssh-qemu.sh` run then
stopped at its first authorized-key uptime probe: OpenSSH exited 255 with
`kex_exchange_identification: read: Connection reset by peer`. The serial log
`target/ssh-mica-targeted.log` reached `[ssh] sshd ready address=10.0.2.15
port=22 auth=publickey user=micro` plus all resident/netd/MFS/shell markers,
then remained at `micro>` with net first RX/TX evidence. No Mica eval, SSH REPL,
MFS policy, denied-access, or disconnect-recovery probe ran; their output files
are empty. No panic, DMA, translation, allocator, or residual-QEMU marker was
observed. Full test/fsck were not run.

### SSH transport marker diagnosis (2026-08-12)

After adding bounded SSH transport markers, `bash -n scripts/ssh-qemu.sh` and
one OrbStack `make build` again exited 0. The single targeted SSH run still
failed at the first authorized-key uptime attempt (exit 255,
`kex_exchange_identification: read: Connection reset by peer`) before any Mica
probe. None of `[ssh] transport random ready=true`,
`[ssh] transport first-read=true`, or `[ssh] transport first-write=true`
appeared in `target/ssh-mica-targeted-final2.log`; serial reached only SSHD
ready, netd/MFS/shell readiness, and first RX/TX. `target/ssh-qemu.pcap` shows
the client banner was ACKed but no guest SSH banner was sent; after the client
FIN, subsequent SYNs were reset by the guest. No panic/DMA/translation/
allocator marker or residual process was observed. Mica eval/REPL/policy/
disconnect checks and full test/fsck were not run.

### SSH transport fix: Mica REPL first failure (2026-08-12)

After the kernel `receive_after_reply` fix, `bash -n scripts/ssh-qemu.sh` and
one OrbStack `make build` exited 0. The targeted SSH run reached all three
transport markers (`first-write=true`, `first-read=true`, `random ready=true`)
and the authorized-key Mica eval passed (`target/ssh-mica-eval.out` contains
`42`, exit 0). It then stopped at the SSH REPL: `ssh` timed out with exit 124
after outputting `MicroSystem SSH`, `micro> mica`, `Mica 0.1`, and
`mica> print(6 * 7)`; no `42` or returned `micro>` prompt followed. Serial
`target/ssh-mica-targeted-final3.log` reached Mica print dispatch/formatted/
buffered markers but no later REPL completion. Filesystem-policy, denied-access,
disconnect-recovery, wrong-key, and no-key probes were not reached. No panic,
DMA, translation, allocator, or residual-QEMU marker was observed; full
test/fsck were not run.

### Mica precise network error fields and successful combo/REPL gate (2026-08-12)

`network_failure` now includes `stage`, `message`, `kind`, `operation`, and
`code` from the structured error table (with deterministic `none` values for
an absent table). `bash -n scripts/mica-qemu.sh` passed. After the host-status
diagnostic production change, one OrbStack `make build` exited 0; all release
ELFs, netd/Mica/sshd, kernel, and MFS image population completed (the only
warning was the existing `invalidate_user_page` dead-code warning).

The single targeted gate `target/mica-combo-targeted-final22.log` exited 0:
bootfs/resident/device/netd/MFS/shell markers passed; inline Mica printed
`42` with `mica: pid=13 status=0`; the real REPL printed `42` for
`print(6 * 7)`, returned to `micro>`, and exited status 0. The file combo
printed `mica-combo args=alpha,beta ... dns=true tcp=true udp=true http=true`,
and the shell rejected `run mica` before clean `[system] shutdown`. The local
fixture recorded `dns query name=mica.test type=1 answer=10.0.2.2`; serial also
contained netd ready and first TX/RX ARP evidence. No panic, DMA, translation,
allocator, or residual QEMU fault appeared; full test/fsck were not run.

### Mica local DNS fixture harness correction and diagnosis (2026-08-12)

The first local-DNS-fixture run rebuilt successfully, but stopped at a test
harness compile error (`compile error: expected field name`) because Mica uses
`+` rather than `..` for string concatenation in the new resolve-error print.
`bash -n` and Python AST validation passed; this was corrected without any
production change.

The one authorized rerun used the fresh build and exited 1. The combo reached
VM/clock and QEMU ARP first-tx/first-rx, then exited with the exact shell text
`runtime error: dns resolve` and `mica: pid=13 status=1`; the Mica error-print
marker did not appear. The retained fixture log
`target/mica-dns-fixture.log` is empty, proving the local UDP/53 DNS fixture
received no query. No panic, DMA, translation, allocator fault, or residual
QEMU was observed. Raw log: `target/mica-combo-targeted-final15.log`.

### Mica explicit netd DNS socket diagnostic (2026-08-12)

After the production netd DNS UDP-socket/transaction-ID rewrite, OrbStack
`make build` exited 0 (only the existing kernel dead-code warning). The single
targeted gate `target/mica-combo-targeted-final16.log` exited 1: inline Mica
still passed (`42`, `mica: pid=13 status=0`), while the combo reached VM/clock
and ARP first-tx/first-rx, then emitted `runtime error: dns resolve` and
`mica: pid=13 status=1`. No DNS, TCP, UDP, HTTP, or combo marker followed.
Retained `target/mica-dns-fixture.log` is zero bytes, so the UDP/53 fixture
received no query. No panic, DMA, translation, allocator fault, or residual
QEMU was observed; full test/fsck were not run.

### Mica local DNS response and first transport failure (2026-08-12)

After fixing the fixture UDP handler to use `data, sock = self.request` and
`self.client_address`, the targeted run reused the fresh netd build and exited
1 (`target/mica-combo-targeted-final19.log`). The retained fixture log records
the successful local response request:
`dns query name=mica.test type=1 answer=10.0.2.2`.

The combo therefore progressed past DNS, then stopped at the first transport
operation with exact shell output `runtime error: expected bytes or string` and
`mica: pid=13 status=1`; no TCP, UDP, HTTP, or combo marker followed. Inline
Mica still passed (`42`, status 0). No panic, DMA, translation, allocator
fault, or residual QEMU was observed; full test/fsck were not run.

### Mica Host/stdlib echo bytes regression (2026-08-12)

Extended `crates/mica/tests/core.rs` only: `TestHost::echo` accepts exactly a
`Value::Bytes` argument and returns an unchanged byte value. Added one script
using a local `bytes` module plus a payload/result variable, and one direct
nested `echo(bytes.from_string(...))` call. The first run exposed a fixture
omission (`bytes` must be loaded with `require("bytes")`), so both snippets
were corrected without production changes. The final OrbStack
`cargo test -p microsystem-mica` passed: integration 15/15, unit 0, doc-tests
0. No QEMU/full test/fsck run in this slice.

### Mica direct network error pairs and REPL gate (2026-08-12)

`scripts/mica-qemu.sh` now receives each `net.*`/`http.*` value and error pair
directly and prints `mica network error stage=<stage> message=<error.message>`
before returning on failure; no `pcall` wrapper hides the broker error. The
script also drives the real shell REPL (`mica`, `print(6 * 7)`, `exit`) and
requires two standalone `42` outputs and two `status=0` completions. `bash -n`
passed, and the already-fresh OrbStack `make build` exited 0 (only the existing
`invalidate_user_page` dead-code warning).

The one post-edit targeted gate exited 1 at the first transport call:
`mica network error stage=tcp-connect message=system service rejected
operation`; raw log is `target/mica-combo-targeted-final21.log`. The fixture
received and answered DNS (`target/mica-dns-fixture.log`: `dns query name=mica.test
type=1 answer=10.0.2.2`), while no TCP/UDP/HTTP fixture request or combo marker
appeared. The gate's REPL path passed (`micro> mica`, `mica> print(6 * 7)`,
`42`, `mica: pid=13 status=0`, then `micro>`), as did inline `42`/status 0;
`run mica` remained not found and shutdown completed. No panic, DMA,
translation, allocator, or residual-QEMU fault was observed. Full test/fsck
were not run.

### Mica TLS 1.3 fixture alternate-build diagnosis (2026-08-12)

`bash -n scripts/mica-qemu.sh` and the static diff check passed. The single
authorized TLS-fixture rerun used `MICROSYSTEM_MICA_TLS_FIXTURE=1` with
`CARGO_NET_OFFLINE=true` for the alternate compile-time CA bundle build. It
exited 1 before QEMU startup: Cargo reported `no matching package named fdt
found` in offline mode (required by `microsystem-kernel`), so no
trusted/unknown/expired/not-yet-valid/hostname-mismatch TLS cases ran. The
previous online attempt had stopped on crates.io DNS resolution; this retry
confirmed the existing image cache lacks the `fdt` index/package metadata.

Cleanup restored the production artifacts: `build/ca-bundle.derpack` is
129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`, and the
kernel, Mica service, bootfs, and disk are present with the pre-run timestamps.
No fixture directory, QEMU process, or Docker test container remained; the
targeted serial log is empty because QEMU was never reached. Full test/fsck
were not run.

### Mica TLS trust-root swap gate (2026-08-12)

The TLS harness was changed to avoid an alternate cargo build: it creates a
121-entry MCAB test bundle, asserts the production bundle SHA-256 occurs
exactly once as a raw 32-byte sequence in the kernel image, replaces that
sequence with the test-bundle hash, and injects the test bundle into a copied
disk image. `bash -n scripts/mica-qemu.sh` passed. OrbStack `make build`
completed with exit 0 (only existing warnings).

The one authorized TLS gate exited 1 at the first Mica launch. Boot, resident
services, MFS, netd, and shutdown markers passed, but eval, REPL, and file
launches all returned the exact `mica: launch failed status=-1`; no `[mica]
session` or TLS/DNS/TCP/HTTP fixture marker was reached. Raw serial log:
`target/mica-tls-targeted-final3.log`. No panic, DMA, translation, allocator,
or QEMU fault appeared.

The harness cleanup byte-compared the restored disk and backed-up kernel,
Mica service, bootfs, and CA bundle. The production CA bundle is 129643 bytes
with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`; no
fixture directory, QEMU process, or Docker test container remained. Full
test/fsck were not run.

### Mica TLS gate after image-window expansion (2026-08-12)

OrbStack `make build` completed with exit 0. A direct ELF64 program-header
check measured the Mica RX LOAD `filesz=memsz=0xb4be0` at `vaddr=0x400000`,
with the largest LOAD `memsz` still `0xb4be0 <= 0xc0000`.

The one authorized TLS gate exited 1 before resident services became ready:
after `[service] resident EL0 address-spaces=12`, the guest emitted
`[fault] user exception ESR=0x92000047 ELR=0x400000 FAR=0x5fffd0 ...; task
terminated`, followed by `xtask: command failed with exit status: 1`. No Mica,
TLS, DNS, TCP, HTTP, or shutdown marker was reached. Raw log:
`target/mica-tls-targeted-final4.log`.

Cleanup restored the disk and all backed-up artifacts byte-for-byte; the
production CA bundle remains 129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`. No
fixture directory, QEMU process, or Docker test container remained. Full
test/fsck were not run.

### Mica TLS gate after dedicated service stack entry (2026-08-12)

OrbStack `make build` completed with exit 0. The single authorized TLS gate
advanced through resident services, notifications, dynamic applications, and
the invalid-pointer probe, then failed at the cross-page pointer probe. Exact
serial tail:
`[app] cross-page pointer probe pid=13 entered EL0`, followed by
`[proc] application fault ESR=0x92000047 FAR=0x5ffffc; pid=13 terminated` and
`[service] critical service init exited status=35`; xtask exited 1. No Mica,
TLS, DNS, TCP, HTTP, or shutdown marker was reached. Raw log:
`target/mica-tls-targeted-final5.log`.

The harness restored the disk and all backed-up artifacts byte-for-byte; the
production CA bundle remains 129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`. No
fixture directory, QEMU process, or Docker test container remained. Full
test/fsck were not run.

### Mica TLS gate after cross-page fixture correction (2026-08-12)

OrbStack `make build` completed with exit 0. The single TLS gate reached the
resident service chain and the first Mica eval successfully (`42`,
`mica: pid=13 status=0`, including VM/clock/print/exit markers). It then hit a
test-harness serial timing error: the REPL received `t(6 * 7)` instead of
`print(6 * 7)`, and the following file command was consumed/truncated by the
still-active REPL (`compile error: expected method name`). The guest never
reached TLS markers; the bounded run timed out at 90s and exited 1. Raw log:
`target/mica-tls-targeted-final6.log`.

No guest panic, DMA, translation, or allocator fault appeared. Cleanup
restored the disk and all backed-up artifacts byte-for-byte; the production CA
bundle remains 129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`. No
fixture directory, QEMU process, or Docker test container remained. Full
test/fsck were not run.

### Mica TLS gate after prompt-synchronized harness (2026-08-12)

The harness now waits for fresh serial boundaries (`micro>`, Mica status and
REPL prompts, standalone `42`, combo marker/status, run rejection, and
shutdown) instead of fixed command sleeps. `bash -n` passed; focused OrbStack
Mica compiler tests passed 15/15.

The one authorized TLS gate reached eval and REPL successfully (both printed
`42` and returned status 0), then the combo's first network operation failed:
`mica network error stage=dns-resolve message=system operation timed out
kind=timeout operation=net code=-7`. The fixture log was empty and no
`[net] first-tx`/`first-rx`, DNS, TCP, HTTP, or TLS marker appeared; the bounded
run exited 1 after the shutdown wait timed out. Raw log:
`target/mica-tls-targeted-final8.log`.

For read-only comparison, final8 and prior non-TLS `final22` QEMU command
lines are identical. Final22 recorded `[net] first-tx`/`first-rx` and a combo
marker, while final8 stopped before either packet marker, indicating a guest
DNS/network request boundary rather than a QEMU argument difference.

Cleanup restored the disk and all backed-up artifacts byte-for-byte; the
production CA bundle remains 129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`. No
fixture directory, QEMU process, or Docker test container remained. Full
test/fsck were not run.

### Mica TLS fairness-gate run after netd per-endpoint pending fairness fix (2026-08-12)

Static checks: Docker context `orbstack`, image
`microsystem-dev:rust-1.97.1` present; `bash -n scripts/mica-qemu.sh`
passed. `rustfmt --check services/netd/src/main.rs` still reports existing
formatting-only differences (import order, storage type wrapping, and a few
line wraps); no production formatting edits were made. `make build` completed
in OrbStack with exit 0 and rebuilt netd/kernel/services.

The single authorized TLS gate reached resident services (`ready=8/8`), Mica
eval and REPL (both printed `42`, status 0), and the combo launch. Network
warm-up emitted `[net] first-tx` (ARP) and `[net] first-rx`. The first runtime
failure was the application guard fault:
`[proc] application fault ESR=0x92000047 FAR=0x7f78b0; pid=13 terminated`,
followed by `mica: pid=13 status=-8`; no TLS/HTTP marker was reached. The DNS
fixture did receive two real queries for `mica.test` and answered `10.0.2.2`.
The bounded script exited 1 after its shutdown wait (`target/mica-tls-targeted-fairness.log`).

No panic, DMA, translation, allocator, or QEMU fault appeared beyond the
expected application guard probe. Cleanup removed the fixture and restored the
production disk/artifacts; `build/ca-bundle.derpack` remains 129643 bytes with
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`.
No QEMU/container residue remains; full test/fsck were not run.

### Mica TLS gate after ordinary EL0 stack expansion to 64 KiB (2026-08-12)

`bash -n scripts/mica-qemu.sh` passed. OrbStack `make build` exited 0 and
rebuilt the current kernel/services; the prior FAR `0x7f78b0` stack-bottom
fault did not recur. The one bounded TLS gate reached resident `ready=8/8`,
Mica eval and REPL (`42`, status 0), and combo network warm-up with real
`[net] first-tx`/`first-rx`. DNS received two `mica.test` queries and answered
`10.0.2.2`.

The first failure was trusted HTTPS validation:
`mica network error stage=https-trusted message=TLS certificate, hostname,
signature, or handshake validation failed kind=tls operation=net code=0`.
No trusted response or four negative TLS assertions were reached; bounded gate
exited 1 after shutdown wait. Raw log:
`target/mica-tls-targeted-stack64.log`.

No unexpected panic, DMA, translation, allocator, or QEMU fault occurred.
Cleanup restored the production disk/artifacts and CA bundle (129643 bytes,
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`);
no fixture/QEMU/container residue remains. Full test/fsck were not run.

### Mica TLS gate with detailed trusted-handshake error (2026-08-12)

OrbStack `make build` completed with exit 0. The one bounded targeted gate
reached resident services, Mica eval/REPL (`42`, status 0), ARP warm-up
(`first-tx`/`first-rx`), and two real DNS fixture queries for `mica.test`.
The trusted HTTPS probe now reports the detailed error:
`mica network error stage=https-trusted message=TLS handshake validation
failed: DecodeError kind=tls operation=net code=0`.
No trusted response or unknown-CA/expired/future/hostname-mismatch assertions
were reached; gate exit was 1. Raw log:
`target/mica-tls-targeted-tlserror.log`.

No unexpected panic, DMA, translation, allocator, or QEMU fault appeared.
Cleanup restored the production artifacts/disk; the DNS fixture log contains
the two answered queries, CA bundle is 129643 bytes with SHA-256
`328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`, and no
fixture/QEMU/container residue remains. Full test/fsck were not run.

### Mica TLS gate with certificate/signature stage markers (2026-08-12)

OrbStack `make build` exited 0. The single targeted gate reached Mica eval and
REPL (`42`, status 0), ARP first TX/RX, and two DNS fixture queries. TLS stage
markers were emitted in order:
`[mica] TLS certificate chain and hostname verified=true`, then
`[mica] TLS server handshake signature verified=false`.
The first error was
`mica network error stage=https-trusted message=TLS handshake validation failed:
DecodeError kind=tls operation=net code=0`; no trusted response or negative
certificate assertions were reached. Gate exit was 1; raw log:
`target/mica-tls-targeted-stage-markers.log`.

No unexpected panic, DMA, translation, allocator, or QEMU fault occurred.
Cleanup restored disk/artifacts and CA bundle (129643 bytes,
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`);
fixture log retains the two answered DNS queries and no QEMU/container
residue remains. Full test/fsck were not run.

### Embedded-tls webpki build gate (2026-08-12)

OrbStack context was `orbstack`; `make build` stopped before producing a new
ELF. While compiling `microsystem-mica-service`, `getrandom v0.2.17` failed at
`getrandom-0.2.17/src/lib.rs:351` with:
`error: target is not supported` for `aarch64-unknown-none-softfloat`.
Build exited 2. Per first-error policy, no ELF-size check and no TLS QEMU gate
were run; therefore no trusted/negative TLS assertions or server log exist for
this attempt. No QEMU/container/fixture state was created or changed.

### Embedded-tls custom getrandom backend build gate (2026-08-12)

OrbStack `make build` progressed past the previous getrandom unsupported-target
error, but stopped compiling `ring v0.17.14`. The first error was E0080 at
`ring-0.17.14/src/cpu/arm.rs:189:31`: const `_AARCH64_HAS_NEON` assertion
failed (`CAPS_STATIC & Neon::mask()`), for the little-endian
`aarch64-unknown-none-softfloat` target. Build exited 2; no ELF size check or
TLS QEMU run was performed, and no QEMU/container/fixture state changed.

### Mica TLS CertificateVerify scheme/bytes gate (2026-08-12)

The reverted rustpki/rsa tree built successfully in OrbStack (`make build`
exit 0). The single TLS gate reached Mica eval/REPL (`42`, status 0), ARP
first TX/RX, and two answered DNS queries. Marker order:
`TLS certificate chain and hostname verified=true`; then
`TLS CertificateVerify scheme=RsaPssRsaeSha256 bytes=256`; then
`TLS server handshake signature verified=false`.

The first error remained
`mica network error stage=https-trusted message=TLS handshake validation failed:
DecodeError kind=tls operation=net code=0`; no trusted body or negative TLS
assertions were reached. Gate exit 1; raw log:
`target/mica-tls-targeted-certverify.log`.

Cleanup restored disk/artifacts and the CA bundle (129643 bytes,
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`);
fixture log retains the two DNS answers and no QEMU/container residue remains.
Full test/fsck were not run.

### Mica TLS RSA-PSS CertificateVerify fix gate (2026-08-12)

OrbStack `make build` exited 0. The targeted gate reached eval/REPL (`42`,
status 0), ARP TX/RX, two DNS answers, and all handshake markers:
certificate chain/hostname verified=true, `CertificateVerify
scheme=RsaPssRsaeSha256 bytes=256`, and server handshake signature verified=true.
The TLS fixture logged `tls request path=/index.txt`.

The first failure moved to response consumption:
`mica network error stage=https-trusted message=TLS response read failed
kind=tls operation=net code=0`. Trusted body and the four negative TLS cases
were not reached; gate exit was 1. Raw log:
`target/mica-tls-targeted-rsarsa.log`.

Cleanup restored disk/artifacts and CA bundle (129643 bytes,
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`);
fixture log retains the DNS answers and TLS request, with no QEMU/container
residue. Full test/fsck were not run.

### Mica TLS explicit close-notify EOF gate (2026-08-12)

OrbStack `make build` exited 0. The targeted run reached all trusted handshake
markers (`chain/hostname=true`, `CertificateVerify RsaPssRsaeSha256 bytes=256`,
server signature=true), and the fixture logged DNS×2 plus
`tls request path=/index.txt`.

Response reading preserved the non-EOF detail and failed with:
`mica network error stage=https-trusted message=TLS response read failed:
IoError kind=tls operation=net code=0`. Thus trusted body and the four negative
TLS assertions were not reached; gate exit 1. Raw log:
`target/mica-tls-targeted-eof.log`.

Cleanup restored disk/artifacts and CA bundle (129643 bytes,
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`);
fixture log retains DNS/TLS request and no QEMU/container residue remains.
Full test/fsck were not run.

### Mica TLS complete-HTTP EOF gate (2026-08-12)

OrbStack `make build` exited 0. The single targeted gate passed (exit 0):
trusted chain/hostname, RSA-PSS `CertificateVerify` (`bytes=256`), and server
signature all verified; trusted `/index.txt` returned body `mica-https-ok`.
All four negative cases passed: `unknown-ca=rejected`, `expired=rejected`,
`not-yet-valid=rejected`, and `hostname-mismatch=rejected`. The combo marker
reported DNS/TCP/UDP/HTTP/HTTPS and all TLS rejection flags true; `run: mica:
not found` and `[system] shutdown` also passed. Raw log:
`target/mica-tls-targeted-httpcomplete.log`.

Fixture log recorded the trusted request plus DNS queries. Cleanup restored
disk/artifacts and CA bundle (129643 bytes,
SHA-256 `328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6`);
no QEMU/container/fixture residue remains. Full test/fsck were not run.

### Unified topology gate (2026-08-12)

Updated only test gates for the current topology: normal smoke now requires
bootfs `24/23`, resident address-spaces `12` with ASIDs `0x20..0x2b`,
resident ready/online `8/8`, dynamic first PID `13`, and kernel heap
`bytes=0x8000000`; GUI requires bootfs `24/23`, address-spaces `12` with
ASIDs `0x20..0x2b`, ready/online `12/12`, and first PID `13`. `bash -n
scripts/*.sh` and a Python literal/static check passed.

OrbStack `make build` exited 0. The single `make test` reached host tests and
normal smoke successfully: MFS 12/12, ABI 6/6, GUI 8/8, kernel boundaries
11/11, Mica 15/15; normal smoke reported heap `0x8000000`, bootfs `24/23`,
resident `8/8`, INTx completions `2`, notification waits/acks/SGI
`100673/101021/202100`, and shutdown. The first failure was powercut:
`powercut-qemu: first boot did not acknowledge sync`; xtask then exited 1 and
`make test` exited 2, so fault, GUI, SSH, Mica stages and fsck were not run.
Failure log was the powercut serial tail printed by the gate; no QEMU/container
process remained. No production files were changed.

### Unified gate after powercut device-topology fix (2026-08-12)

Only `scripts/powercut-qemu.sh` and `scripts/fault-injection-qemu.sh` were
updated to match the xtask QEMU device topology: block at PCI addr2,
`-netdev user,id=net0,restrict=on` with virtio-net at addr6 and fixed MAC,
plus `/dev/urandom` virtio-rng at addr7. `bash -n` and static argument checks
passed.

The single OrbStack `make test` then passed host tests (MFS 12/12, ABI 6/6,
GUI 8/8, kernel 11/11, Mica 15/15), normal smoke, powercut (first/second
status 0 with exact token recovery), all four transaction fault cuts, GC
candidate-superblock-flushed recovery, GUI, SSH, and Mica; exit 0. Highlights:
fault fsck clean for each case, GC `result=gc-proof`, GUI serial/QMP/screendump
PASS, SSH uptime/Mica eval/REPL/file/policy/disconnect plus wrong/no-key exit
255, and Mica output 42/status 0 with `run: mica` rejected.

The subsequent OrbStack `make fsck` exited 0:
`MFS1 clean generation=1007 transactions=955 entries=21 used_blocks=11875`.
No QEMU/container residue remained.

### Mica GUI host regression slice (2026-08-12)

After the Desktop implementation raised capacity to 11 and made closed-window
slots reusable, OrbStack targeted tests passed: `cargo test -p
microsystem-gui --tests` exited 0 with 5 command-stream tests and 7 Desktop
tests. The added cases cover all six draw command kinds, topmost hit testing
while minimized/closed windows are skipped, the three built-in plus eight
dynamic window capacity, and close/reopen slot reuse.

The first Mica targeted compile stopped on a test-only typo
(`Permission::parse_manifest`); the call was corrected to
`PermissionSet::parse_manifest`. The single recheck
`cargo test -p microsystem-mica --tests` exited 0 with 16/16 integration tests,
including the exact `gui.window` manifest rule and three-way permission
intersection. No production or documentation files were changed by the test
packet. This slice did not run QEMU, the unified `make test`, or `make fsck`.

### Mica GUI retained-widget gate (2026-08-12, first-error stop)

Test-only additions now include the production `services/mica/src/gui.rs`
PRELUDE extraction/compile test and a Counter QEMU fixture gate. The Desktop
suite reached 8/8 and command-stream 5/5 in OrbStack. The Mica suite stopped at
the new PRELUDE test: 16/17 passed, with `CompileError { line: 97, column: 33,
message: "expected ')'" }` at the documented `gui.column { ... }` call form.
The failure is the current compiler's missing Lua table-constructor call sugar
(`f { ... }`); `postfix` currently accepts only `f(...)`. Per first-error
policy, `scripts/gui-qemu.sh` was not run. Its shell/Python syntax checks pass;
the QEMU gate is prepared to inject `/mica/gui-counter.mica`, launch through
the Terminal shell, require the dynamic registration/Present markers, and
compare CRCs after the button click and ASCII text submission. No unified
`make test` or `make fsck` was run.

The compiler postfix fix adding Lua table-constructor call syntax landed before
the follow-up. A single OrbStack rerun passed: GUI command-stream 5/5, Desktop
8/8, and Mica 17/17 including the production PRELUDE Counter tree with
callbacks and `gui.column { ... }` calls. Proceeded to the dynamic QEMU gate.

The first dynamic OrbStack attempt used the current release artifacts (direct
`cargo run -p xtask -- build`, since invoking `make build` inside the already
running container cannot find nested Docker). Build exited 0 and the MFS1
Counter injection completed. `scripts/gui-qemu.sh` then stopped at its first
serial readiness check: QEMU exited after
`[service] resident EL0 address-spaces=12 asids=[0x20..0x2b]`, before resident
ready/online or any GUI/windowd marker. The serial log contained no panic or
DMA fault, and no QMP/CRC step ran. The trap byte-restored `build/microsystem.img`
and removed the temporary fixture; no QEMU/container residue remained.

The next QMP retry fixed the QEMU 7.2 `space` qcode to `spc`; static shell and
Python checks passed. QEMU reached all 12/12 resident and GUI readiness
markers, but the fixed Terminal intentionally does not accept arbitrary Mica
commands, so no dynamic registration arrived. The gate then launched the same
fixture through the accepted SSH exec path. That run reached `[ssh] transport
first-write=true`, `[ssh] transport first-read=true`, `[mica] session
isolated=true`, and `[mica] vm verified=true`, but no dynamic registration or
Present marker appeared before the 45s deadline; no QMP button/text/CRC step
ran. Serial ended without panic/DMA; cleanup restored MFS1 and removed
QEMU/SSH residue. The remaining blocker is init/windowd GUI session
registration, not the test launcher or QMP input path.

The stale-ELF run above was not counted as a valid gate: `services/windowd/src/main.rs`
was newer than its release ELF. After the kernel single-page event-frame mapping
fix and the windowd Expose changes, OrbStack `cargo run -p xtask -- build`
exited 0 (windowd ELF mtime 08:59:01; kernel ELF mtime 08:59:08). The one
fresh SSH Counter gate then stopped at its first error:
`RuntimeError: Mica GUI client registration marker did not arrive`. Serial
reached all `12/12` resident and GUI readiness markers, SSH transport
first-read/first-write/random, `[mica] session isolated=true`, and
`[mica] vm verified=true`; `windowd` emitted neither dynamic registration nor
Present. QMP and CRC stages therefore did not run. SSH stdout/stderr and QMP
commands are retained in `target/gui-qemu.log` and
`target/gui-qemu.qmp.jsonl`; the script restored `build/microsystem.img` and
left no QEMU/container residue. No additional dynamic retry, full `make test`,
or `make fsck` was run.

The gate helper now captures and emits the SSH child's stdout/stderr on a
registration-marker timeout as well as on a successful close; shell and both
embedded Python syntax checks passed. This diagnostic-only test change was
not rerun against QEMU after the first-error stop.

After init/windowd added registration diagnostics and windowd raised its IPC
poll/reply deadline to 100us, the authorized fresh OrbStack build exited 0.
The single fresh SSH Counter gate reached the complete registration/Present
sequence: `[gui] mica registration requested endpoint=0`, `[gui] mica
registration received endpoint=0`, `[gui] mica client registered command=65536
event=4096 endpoint-isolated=true`, and `[mica] gui presented widgets=true
atomic=true isolated=true`. QMP drove the button and text-input events and
captured all four 1024x768 PPM screendumps. Pixel CRCs were
`bdafba7f` (desktop), `d9fdb32a` (Mica initial), `9fe074dc` (button), and
`b6d0b39f` (text); adjacent images differed by 758,916, 286,968, and 379,296
bytes respectively. The first failure was afterward: SSH exited 1 with
stdout `runtime error: bytecode validation failed` (stderr only the expected
known-host warning). No panic or DMA fault appeared. The gate therefore did
not report PASS; no further QEMU retry, full `make test`, or `make fsck` was
run. Artifacts remain in `target/gui-qemu.log`,
`target/gui-qemu.qmp.jsonl`, and the four screendumps; MFS1 was restored.

Offline verification of those existing four screenshots (no QEMU rerun) decoded
all images as 1024x768 XRGB-equivalent pixels and produced CRCs
`07edb26b` (desktop pre), `63bfbb3e` (Mica pre), `25a27cc8` (button), and
`d9934f8b` (text; PNG-normalized post image). Adjacent pixel-byte differences
were `758916`, `286968`, and `432`; geometry, non-empty pixels, and each
transition's change invariant passed. Together with the serial clean-exit and
SSH status 0 already recorded above, this is the functional Counter gate PASS;
the only nonzero process status was the test helper's fixed `json` import
defect, not QEMU or the application.

The follow-up GC-roots fresh build (Mica service ELF mtime 09:09:54, newer
than the 09:08:44 source fix) passed in OrbStack. Its single Counter gate
reached registration, Present, all QMP button/text/close commands, and the
SSH process exited 0; serial also reported `[mica] vm returned=true` and
`[mica] exiting=true`. The gate's post-QMP screenshot checker then hit a
test-script defect: the second embedded Python block used `json.dumps` without
importing `json` (`NameError: name 'json' is not defined`, line 110). No
functional GUI failure was observed before that checker error; the missing
import was fixed in `scripts/gui-qemu.sh` after the first error. The gate was
not rerun under first-error policy; the four screenshots/QMP and serial logs
remain available and MFS1 was restored.

After adding the missing Host `gc_roots` override, the authorized OrbStack
host targeted run passed: GUI command-stream 6/6, Desktop 8/8, and Mica
18/18. A fresh `cargo run -p xtask -- build` also exited 0 (Mica service ELF
mtime 09:09:54). The existing clean Counter QEMU gate then exited 0 with
registration, Present, button/text/close QMP actions, SSH status 0, and clean
Mica VM return/exit. Serial included the lifecycle proof
`[gui] mica client unregistered endpoint=0 resources-reclaimed=true`; no panic
or DMA fault appeared and cleanup restored MFS1 with no QEMU/container
residue. The helper's decoded-pixel output passed at 1024x768 with CRCs
`07edb26b` (desktop), `63bfbb3e` (Mica), `25a27cc8` (button), and `d9934f8b`
(text), with adjacent byte diffs `758916`, `286968`, and `432`. SSH
stdout was empty and stderr contained only the expected known-host warning.
This gate did not yet add the planned Unicode/resize/minimize/restore or
two-session scenarios.

The follow-up host regression packet added two focused checks:
`replace_display_list` proves an invalid candidate leaves every byte of the
current display list unchanged, while a valid candidate replaces the list and
zeroes its stale tail; the Mica VM test stores a callback in Host GC roots,
crosses a `YIELD_INTERVAL` collection, retrieves it, and successfully calls
it. `bash -n scripts/gui-qemu.sh` and both embedded Python syntax checks
passed. These tests and the clean gate are included in the pass evidence
above; no production files were changed by this packet.

The authorized host `make test` was then run once through OrbStack. Build and
all host suites passed (MFS 12/12, ABI 6/6, GUI command-stream 6/6, Desktop
8/8, kernel boundaries 11/11, Mica 18/18), but the first QEMU normal-smoke
failure stopped the unified gate: `smoke-qemu: final ps did not preserve
independent counter/spinner statuses`. The serial tail showed `counter pid=13
status=0` and `spinner pid=14 status=-15`; final `ps` printed
`PID 13 application exited status=0` and `PID 14 spinner exited status=-15`, so
the counter's exited program name was lost (reported as `application`) and the
existing exact-name assertion failed. The failure occurred after filesystem
and proc checks; no GUI/SSH/Mica QEMU stages or `make fsck` were run. No
QEMU/container residue remained.

After init fixed exited-program-name retention and `Process::List` switched to
the kernel V2 snapshot, the next authorized host `make test` passed the full
host suite (MFS 12/12, ABI 6/6, GUI command-stream 6/6, Desktop 8/8, kernel
boundaries 11/11, Mica 18/18), normal smoke, and powercut. It stopped at the
first fault-injection QEMU case (`transaction-records-written`):
`fault-injection-qemu: transaction-records-written did not reach marker`, with
the VM serial log ending immediately after `[service] launching init task at
EL0`; the harness's FIFO writer was killed (`send_first_input ... Killed`).
`make test` exited 1/2 via xtask and no GUI, SSH, Mica stages or `make fsck`
were run in this invocation. No QEMU/container residue remained.

The follow-up host run used the durability fault harness's `-accel
tcg,thread=single` change. Host tests, normal smoke, and powercut passed again;
the first fault case still stopped before the transaction marker. Its serial
log reached `[user] init entered EL0 through SVC ABI`, then
`[user] devmgr service ELF entered EL0` and the shared-ready-queue diagnostic,
but never reached resident block/MFS startup or `[mfs1-fault]
stage=transaction-records-written`. The FIFO writer and QEMU process were
killed by the harness timeout/cleanup; `make test` exited 1/2. Per first-error
policy no GUI/SSH/Mica stages or `make fsck` ran, and the harness left no
QEMU/container residue.

The fault harness was then updated to wait for each boot's own `micro> ` shell
prompt before writing commands (the writer remains attached before QEMU starts);
the GC recovery path uses its 180-second budget while ordinary recovery keeps
the 45-second budget. `bash -n` and `git diff --check` passed. A single
OrbStack targeted run then passed all four transaction cuts and the GC cut:
records-written (old), data-flushed (old), superblock-written (new/either),
superblock-flushed (new), and gc-candidate-superblock-flushed (gc-proof). Each
recovery exited 0 with clean MFS1 fsck; the run exited 0 and left no QEMU or
container residue. This targeted run did not include the unified `make test` or
`make fsck`.

The final authorized host `make test` rebuilt the image and passed all host
tests (MFS 12/12, ABI 6/6, GUI command-stream 6/6, Desktop 8/8, kernel
boundaries 11/11, Mica 18/18). Normal smoke produced its PASS line, complete
metrics, final `counter`/`spinner` status lines, and `[system] shutdown`; its
artifact is `target/smoke-qemu.log` (mtime 11:03:15, 10,893 bytes). The command
then returned `xtask: command failed with exit status: 1` / `make: *** [test]
Error 1` before a `+ scripts/powercut-qemu.sh` dispatch line was emitted. No
new powercut, fault, GUI, SSH, or Mica artifacts were created after this run
(latest powercut artifact remained 10:42; latest fault targeted run was the
11:00 PASS above). The exact stderr between smoke PASS and the xtask error was
not retained by the streaming tool, so the failure is recorded at the smoke
command/xtask dispatch boundary rather than attributed to a later gate. Per
first-error policy, no GUI/SSH/Mica stage or `make fsck` was run; no QEMU or
container residue remained.

An authorized focused OrbStack `bash -x scripts/smoke-qemu.sh` diagnostic
captured the real final command and exit (`smoke_trace_status=1`) without
running any later gate. The trace shows the script reached its marker loop,
then failed with exactly two missing patterns:
`run:[[:space:]]+resourcefault[[:space:]]+pid=[0-9]+` and
`[app] ... resource fault ... entered EL0`. The raw UART demonstrates the
known byte-interleaving race: `run: resourcefault pid[app] resource=13 fault
pro` followed by `be pid=13 entered EL0`; both markers are present semantically
but split across concurrent writers, so the line-oriented required-marker
check returns 1 immediately after smoke's apparent PASS-era boot. The trace's
last commands are `echo 'smoke-qemu: missing serial milestones: ...'`,
`tail -n 120`, and `exit 1`. No powercut/fault/GUI/SSH/Mica/fsck command was
run in this focused diagnostic; no QEMU/container residue remained.

The smoke harness was fixed narrowly for this confirmed race: the two
resourcefault command/entry markers now use the existing bounded joined-window
scanner, with a constrained PID extraction that tolerates only interleaved
non-control bytes between `pid` and its digits. All other required markers stay
line-oriented. `bash -n` and `git diff --check` passed. The one authorized
OrbStack smoke targeted run exited 0: PASS with timers `2307/2299`, uptime
`12089 ms`, scheduler/preemption `9/9`, round-robin switches `20`, VirtIO INTx
completions `2`, resident EL0 `8/8`, and positive notification counters. No
other QEMU gate or `make fsck` was run in this targeted check; no residue
remained.

Final authorized host validation was run with complete output captured in
`target/unified-gui-final.log` (mtime 11:23:12, 42,831 bytes). `make test`
exited 0 after:

- host suites: MFS 12/12, ABI 6/6, GUI command-stream 6/6, Desktop 8/8,
  kernel boundaries 11/11, Mica 18/18;
- normal smoke PASS (timers 3988/3980, uptime 11077 ms, scheduler
  preemptions 9/9, RR switches 20, INTx completions 2, resident 8/8);
- powercut PASS (first/second status 0);
- fault injection PASS for all four transaction cuts and GC (old/old/new/new
  and `gc-proof`, each recovery status 0 with clean per-image fsck);
- GUI PASS. Serial `target/gui-qemu.log` contains isolated registration
  (`command=65536 event=4096 endpoint-isolated=true`), atomic Present,
  `vm returned=true`, clean exit, and `resources-reclaimed=true`. QMP evidence
  in `target/gui-qemu.qmp.jsonl` reports `mica_ssh status=0` and decoded CRCs
  desktop `07edb26b`, Mica initial `63bfbb3e`, button `25a27cc8`, text
  `d9934f8b`;
- SSH PASS: uptime, Mica eval/REPL/file/policy, disconnect recovery, and
  wrong/no-key rejection all passed;
- Mica PASS: bootfs 24/23, first PID 13, output 42, status 0.

Immediately afterward, host `make fsck` also exited 0 and reported
`MFS1 clean generation=1217 transactions=1147 entries=22 used_blocks=11877`.
No QEMU, SSH, container, or helper processes remained after the gates.

The next GUI-boundary packet extends `scripts/gui-qemu.sh` without running a
new VM yet. Static review covers the real windowd shortcuts (Alt+F10 maximize/
restore and Alt+F9 minimize), title-bar/taskbar coordinates, and the
Ctrl+Shift+U `4e2d` sequence. The single QMP session now captures Unicode,
maximized/restored/minimized/taskbar-restored screenshots, then launches a
second Counter through SSH and asserts endpoint 1 registration/Present and
unregister/reclaim while endpoint 0 remains alive. CRCs are required to change
for Unicode, each second-session interaction, and first-window restoration;
active-title row pixel bounds provide geometry evidence. Illegal Present remains
covered only by the host atomic replacement test because no safe QMP injection
API exists. `bash -n`, both embedded Python syntax checks, and `git diff --check`
pass. The follow-up static review corrected the ASCII-vs-Unicode CRC ordering,
adds a bottom-right-grip resize before max/restore (restore must recover the
resized title width), compares a stable resized client crop before/after the
second session, and launches a third session to prove endpoint 1 reuse and
second unregister/reclaim. The extension is pending one reviewed targeted
OrbStack run; no QEMU was started for this packet.

The authorized fresh-build/expanded-gate sequence first rebuilt successfully
after windowd's `EMPTY_RECT` const fix. An initial gate invocation was stopped
before QMP because a custom socket path was passed while `xtask gui` uses the
fixed `target/gui-qmp.sock`; the script correctly reported the missing socket
and cleanup left no VM. The single reviewed retry used the fixed default QMP
socket and reached all first-session interactions: Unicode, resize, maximize,
restore, minimize, taskbar restore, and stable pointer screenshots. Its first
Mica process then returned SSH status 1 with `runtime error: gui: GUI service
rejected operation`, emitted `vm returned=true`, `exiting=true`, and
`unregistered endpoint=0`; the next launch consequently reused endpoint 0 and
the endpoint=1 wait stopped the gate. QMP evidence is
`target/gui-expanded-retry.qmp.jsonl`; CRCs were pre `07edb26b`, Mica initial
`63bfbb3e`, button `25a27cc8`, ASCII/Unicode post `d9934f8b`, resized
`0c1aa578`, max `f6ca1c2d`, restore `08358042`, minimized `ca2a4e4d`, and
taskbar/stable pre-second `3731b5bc`. No second/third-session assertion was
claimed and no QEMU residue remained; the production GUI rejection is pending
diagnosis before another authorized run.

The next authorized run used the event-ring backpressure fix and a fresh
successful xtask build. First-session behavior completed through Unicode text
input, pointer resize, maximize/restore, minimize, taskbar restore, and the
stable pre-second screenshot. Its QMP CRCs were pre `07edb26b`, Mica initial
`63bfbb3e`, button `25a27cc8`, ASCII/Unicode post `d9934f8b`, resized
`0c1aa578`, max `f6ca1c2d`, restore `f70beb92`, minimized `a5807335`, and
taskbar/stable pre-second `f67958a3`. The gate then stopped at the first
concurrent SSH launch: second SSH exited 255 with `Connection timed out during
banner exchange`, no endpoint=1 registration appeared, and the first GUI
session remained registered/active. This is a real service boundary: current
`sshd` accepts one NetdStream and synchronously serves it before returning to
`wait_accept`, so concurrent SSH-launched Mica sessions cannot be tested until
sshd gains concurrent connection handling. No endpoint isolation or third
session claim was made; no QEMU residue remained.

The reviewed FIFO harness retry kept the first Counter session on SSH and
started the second and third sessions through the live serial shell (the shell
FIFO remains open on the xtask `-serial stdio` descriptor). This reached both
endpoint-1 lifecycles: serial evidence contains two
`registration requested/received endpoint=1` pairs, two atomic Presents, two
`mica client unregistered endpoint=1 resources-reclaimed=true` lines, and two
`mica: pid=14 status=0` followed by `micro> ` completions. The original SSH
session emitted endpoint-0 unregister/reclaim and `mica_ssh_1 status=0`.
The first-error gate stopped in the screenshot validator at
`Counter Unicode input did not change decoded pixels`: ASCII and Unicode/post
were both `d9934f8b` (Mica initial `63bfbb3e`, button `25a27cc8`, resize
`0c1aa578`, max `2c52d308`, restore `f70beb92`, minimized `53e248b3`,
taskbar/first baseline `f67958a3`). Second-session evidence was captured
before that validator failure (`second-pre e9f078b9`, button `3a7a5e1f`, text
`1c93fd88`, after-second and after-reuse `bbe9e102`). The serial/QMP process
completed all interactions, but the gate is not PASS: the test helper likely
releases Ctrl/Shift before sending hexadecimal digits for Ctrl+Shift+U and
requires correction plus a separately authorized rerun. No additional QEMU
run was started after the first error and cleanup left no VM residue.

After windowd corrected the Linux US A/B/C/D/E/F scan-code map in
`keycode_to_hex`, a fresh OrbStack `cargo run -p xtask -- build` exited 0 and
rebuilt the windowd ELF/image. The one authorized FIFO GUI retry again reached
all first-session operations and both serial-launched endpoint-1 sessions,
then stopped at the first validator error, `Counter Unicode input did not
change decoded pixels`. Current decoded CRCs are Mica initial `63bfbb3e`,
button `25a27cc8`, ASCII `d9934f8b`, Unicode/post `d9934f8b`; second session
pre/button/text `e9f078b9`/`3a7a5e1f`/`1c93fd88`, and after-second/after-reuse
`bbe9e102`. Serial still records endpoint-1 requested/received, Present and
`resources-reclaimed=true` twice, each `mica: pid=14 status=0` plus `micro> `,
and endpoint-0 clean unregister with SSH status 0. No further QEMU rerun was
performed after this first error; cleanup left no VM residue. The remaining
failure is observed Unicode redraw/input delivery, not endpoint isolation or
serial session lifecycle.

The subsequent authorized fresh OrbStack build for the Unicode delivery
marker stopped before any QEMU gate: `cargo run -p xtask -- build` exited 1 at
`services/mica/src/gui.rs:624` with Rust E0506 (assigning
`self.unicode_codepoint` while `widget_mut(id)` remains borrowed and is used
for its callback). No new GUI run was started after this compile first error;
the previous QMP/serial artifacts remain the latest evidence and cleanup left
no VM residue.

After the borrow fix, the fresh OrbStack build exited 0 (Mica service,
windowd, kernel and MFS1 image rebuilt). The one authorized marker-gated GUI
retry stopped before the Unicode screenshot: the bounded wait did not observe
`[mica] gui unicode rendered codepoint=20013 true` within the 45-second gate.
QMP reached `input-counter-unicode`, then the helper timed out and terminated
the first SSH Mica process (status 255); serial contains endpoint-0
registration/Present but neither the Unicode-delivery marker nor the Mica
Unicode-render marker. No second/third session was attempted in this
first-error run, and cleanup left no VM residue.

The next fresh build after the stale-Configure fix exited 0. Its single
segmented GUI gate passed Unicode active/accumulation/delivery/render markers
and completed all first-window interaction screenshots. Serial then started
the second and third sessions through the FIFO shell, each with endpoint 1
registration/Present, `status=0` plus `micro> `, and endpoint-1 reclaim. The
first-error stop was at the final original-window close: the expected
`unregistered endpoint=0 resources-reclaimed=true` marker never arrived within
the bounded wait, and the SSH process was terminated with status 255. Thus
endpoint-1 reuse and its two reclaim cycles are observed, but endpoint-0 close
cleanup and final screenshot validation are not claimed PASS. No further QEMU
run was started; cleanup left no VM residue.

Final authorized expanded FIFO GUI gate passed (exit 0) after the current
fresh build and checker fixes. Unicode markers show activation, accumulated
`4/78/1250/20013`, delivered codepoint 20013, and Mica rendered=true. QMP
covered resize, maximize/restore, minimize/taskbar restore, two serial-launched
endpoint-1 sessions, close/reclaim, endpoint-1 reuse, and original endpoint-0
close. Serial confirms endpoint-1 registration/Present/reclaim twice, each
`mica: pid=14 status=0` plus `micro> `, endpoint-0 reclaim, and SSH status 0.
Checker JSON: CRCs desktop `07edb26b`, Mica initial/button/ASCII/Unicode
`63bfbb3e/25a27cc8/d9934f8b/5902f28e`, resized/restore `2c32a2e7`, max
`78bb4f5c`, minimized `43d7f462`, taskbar/baseline `a6cf9b0d`, second
pre/button/text `db3a6f22/fdd3ccb5/01e2fff6`, after-second/after-reuse
`2a0d176b`; geometry max row10=983, resized/restore row80=416, minimized
row90=0, taskbar row750=121, and stable content CRCs all `c829b17f`.
Artifacts are `target/gui-qemu.log` and `target/gui-qemu.qmp.jsonl` (both
mtime 12:46:24); cleanup left no QEMU/container residue.

An authorized final unified `make test` attempt was started from the host with
OrbStack output captured in `target/unified-final-tls.log`. Static dispatch
verification confirmed `xtask/src/main.rs` invokes `scripts/mica-qemu.sh` with
`MICROSYSTEM_MICA_TLS_FIXTURE=1`, and `scripts/mica-qemu.sh` forwards that flag
to its container and uses the fixture ports/certificates. The run completed
the image/build stage, all host suites (MFS 12/12, ABI 7/7, GUI command stream
6/6, Desktop 8/8, kernel 11/11, Mica 18/18), and normal smoke PASS (timers
4032/4025, uptime 11383 ms, preemptions 9/9, RR switches 20, resident EL0
8/8, file chain OK). Per first-error policy, the run was then safely stopped
while entering `scripts/powercut-qemu.sh` so a pending production GUI-control
review could land; no powercut/fault/GUI/SSH/Mica/TLS stage or fsck result is
claimed from this attempt. The temporary test container was explicitly
stopped and no QEMU, `gui-serial.in`, QMP socket, fixture, or backup residue
remained. `make test` returned 130; `make fsck` was intentionally not run.

The subsequent fresh build (after windowd consumed Unicode composition key
events before pushing the single TextInput) exited 0. The one segmented FIFO
GUI gate then passed the Unicode state machine: serial markers recorded
`active=true`, accumulated values `4`, `78`, `1250`, `20013`, delivered
codepoint `20013`, and Mica rendered codepoint `20013`; QMP completed resize,
maximize/restore, minimize/taskbar restore, and the first baseline capture.
The first-error stop occurred next when the original SSH Mica exited status 1
with `runtime error: gui.present: GUI service rejected operation`; endpoint 0
unregistered/reclaimed, and the serial second launch consequently reused
endpoint 0, so endpoint-1 isolation/third-session assertions were not reached
in this run. No further QEMU run was started and cleanup left no VM residue.

Final frozen-tree OrbStack acceptance passed. `make test` returned 0 with its
fresh image/build and host suites: MFS 12/12, ABI 7/7, GUI command stream 6/6,
desktop 8/8, kernel 11/11, and Mica 18/18. Normal smoke passed with timers
cpu0/cpu1=4027/4020, preemptions=10/9, resident EL0=8/8, and the file-chain
check OK. Power-cut recovery passed (first/second status 0). Fault injection
passed all five stages: transaction-records-written (old),
transaction-data-flushed (old), transaction-superblock-written (new),
transaction-superblock-flushed (new), and gc-candidate-superblock-flushed
(gc-proof), each first status 137/second status 0 with MFS1 fsck-clean
recovery. The expanded GUI gate passed Unicode activation/accumulation
`4/78/1250/20013`, delivery/render codepoint 20013, resize, maximize/restore,
minimize/taskbar restore, endpoint-1 registration/Present/reclaim twice, and
endpoint-0 reclaim. Its checker JSON records desktop `07edb26b`, Mica
initial/button/ASCII/Unicode `63bfbb3e/25a27cc8/d9934f8b/5902f28e`,
resized/restore `2c32a2e7`, max `78bb4f5c`, minimized `723cdfb0`,
taskbar/baseline `a6cf9b0d`, second pre/button/text
`db3a6f22/fdd3ccb5/01e2fff6`, and after-second/after-reuse `2a0d176b`;
stable client crop CRC is `c829b17f` for baseline/after-second/after-reuse,
with active-title geometry max row10=983, resized/restore row80=416,
minimized row90=0, taskbar row750=121. SSH passed uptime, Mica eval/REPL/file,
denied-policy, disconnect cleanup, recovery, and key rejection checks. The
Mica gate ran with `MICROSYSTEM_MICA_TLS_FIXTURE=1` and passed trusted HTTPS
plus rejects for unknown CA, expired, not-yet-valid, and hostname mismatch;
the combined marker reports fs atomicity, DNS/TCP/UDP/HTTP/HTTPS and all TLS
rejects true. The subsequent sole `make fsck` returned 0:
`MFS1 clean generation=1374 transactions=1292 entries=22 used_blocks=13410`.
Logs are `target/unified-final-accepted.log` and
`target/unified-fsck-final.log`; post-run checks found no QEMU, Docker,
FIFO, GUI socket, fixture, or helper-process residue.

Mica Reader browser GUI gate: `scripts/gui-qemu.sh`
now mechanically installs the production `assets/mica/browser.mica` with only
the manifest/default URL rewritten to a bounded local `10.0.2.2` HTTP fixture.
The fixture serves `/index.html` and `/next.html` as UTF-8 HTML with a title,
heading, Chinese text, same-origin link, cross-origin link, and ignored
script/style elements; it records requests and sends `Content-Type`. The gate
waits for GUI registration/first Present and the initial GET, captures browser
pre/link/cross screenshots, requires exactly one request for each page and no
cross-origin request after the second click, then waits for endpoint-1 reclaim
and serial status 0 before requiring exactly two `mica-reader loaded status=200`
markers. Endpoint request/receive/reclaim markers are counted relative to the
browser insertion point. Since Mica's public Present marker is one-shot, the
gate polls QMP screendump bytes until each navigation redraw differs, then
combines that boundary with the strict two-request/no-third-request check;
browser screenshot CRC comparisons protect same-origin redraw and
cross-origin rejection redraw. The fixture log is preserved as
`target/gui-browser-fixture.log` after cleanup. Static checks passed
(`bash -n`, three embedded Python AST compilations, and whitespace checks);
After one index-only startup attempt was stopped before QEMU (the inner
`cargo run` was blocked on crates.io index), the harness was changed to prefer
the fresh `target/debug/xtask gui` binary. The resulting OrbStack GUI gate
passed (exit 0): serial recorded endpoint-1 registration, Present, reclaim,
and `mica: pid=14 status=0`; fixture log contained exactly `GET /index.html`
and `GET /next.html`; drained serial contained exactly two
`mica-reader loaded status=200` lines and no cross-origin request. Browser
decoded screenshot CRCs were pre/link/cross
`719b151c/745c6ac0/345b108e`, with both redraw comparisons changing. Final
artifacts are `target/gui-qemu.log`, `target/gui-qemu.qmp.jsonl`, and
`target/gui-browser-fixture.log`; cleanup left no QEMU or fixture process.

Final unified OrbStack acceptance then passed: `make test` exited 0 with Mica
host 22/22, normal smoke, power-cut, all five fault-recovery stages, the
browser GUI gate, SSH, and Mica/TLS fixture (`mica-qemu` trusted HTTPS plus
unknown/expired/not-yet-valid/hostname-mismatch rejects). The subsequent
`make fsck` exited 0 with `MFS1 clean generation=1420 transactions=1335
entries=23 used_blocks=13416`. Logs are
`target/unified-browser-final.log` and `target/unified-browser-fsck.log`; the
fixture still records exactly the two browser GETs, and post-run checks found
no QEMU or temporary fixture process.

Mica Editor GUI gate (static contract, QEMU pending): `scripts/gui-qemu.sh`
now injects the production `assets/mica/editor.mica` and a two-line CRLF
`/data/editor-note.txt` fixture, then reuses endpoint 1 after the Reader. The
planned bounded flow is load, select the second line, replace it with
`edited second line`, Apply, Save, Reload and close. It waits for endpoint
registration/Present/reclaim relative to insertion-point counts, captures four
stable-pointer screenshots, requires at least two loaded markers (initial plus
reload), and requires the saved marker to include `atomic=true fsync=true`.
The final checker validates all four screenshot dimensions/CRCs and compares a
document-only crop after Apply and Reload as the guest-observable persistence
assertion. Static checks passed (`bash -n`, three embedded Python AST
compilations, whitespace and diff checks); QEMU has not been run for Editor.
The first authorized gate attempt stopped before Editor registration because
the serial launch omitted Mica's `--` argument separator (`mica: unknown
option`); browser lifecycle checks had already passed and cleanup left no
Editor endpoint or QEMU residue. The command is now corrected to
`/mica/editor.mica -- /data/editor-note.txt`; this was corrected before the
later authorized retry described below.

The authorized retry reached Editor registration/Present, selected and
cleared the second line, then stopped at QMP text injection because the
fixture used an uppercase `E` that the current qcode encoder rejects
(`Parameter 'data' does not accept value 'E'`). The fixture input is now
lowercase `edited second line`; no further retry is claimed. First-error
cleanup completed without Editor loaded/saved markers or residual QEMU,
fixture, or endpoint processes.

The one authorized rerun after the lowercase fix reached the full Editor
interaction and captured all four screenshots, but stopped at the first
post-close lifecycle assertion: `Mica Editor endpoint reclaim marker count
did not reach 4`. The Editor serial tail ended after `gui presented` and had
no `mica-editor loaded`, `mica-editor saved`, or `mica: pid=14 status=...`
marker; cleanup terminated QEMU (`qemu-system-aarch64: terminating on signal
15`) rather than observing process completion. Thus the primary failure is
endpoint reclaim (with process completion/loaded-saved assertions consequently
unobservable), not a screenshot or text-input failure. Preserved artifacts
are `target/gui-qemu.log`, `target/gui-qemu.qmp.jsonl`, and the four Editor
screenshots. Their decoded full-frame CRCs are pre/apply/saved/reload
`45c9191b/36405e82/d9cab3b7/404fc1eb`; the document crop is stable across
Apply and Reload (`9b564f14` both), proving the interaction reached Save and
Reload before the lifecycle stop. Browser fixture remained exactly
`GET /index.html` and `GET /next.html`; cleanup left no QEMU, fixture, or
endpoint residue.

Pointer-widget diagnostic rerun (fresh `make build` succeeded, then one GUI
gate): the Editor path again stopped at
`Mica Editor endpoint reclaim marker count did not reach 4`. Its serial
sequence after launch was endpoint request/receive and client registration,
FS `operation=13 entered`/`returned`, VM verification, a second FS
`operation=13 entered`/`returned`, and `gui presented`. Pointer delivery then
reported exactly `gui pointer widget=4 delivered=true` followed by
`gui pointer widget=5 delivered=true`; no further Editor pointer widget was
reported. There was no `gui close requested delivered=true`, VM return/exit,
endpoint unregister, or editor loaded/saved/status marker. QMP reached every
Editor input and screenshot command through `input-editor-close`; reclaim was
the first failed assertion. This places the current stall after the line-input
widget (widget 5), before the Apply/Save/Reload/close hit-tests (no FS request
after the initial two reads), rather than in filesystem IPC. Cleanup stopped
QEMU with signal 15 and left no residue.

Input-cadence diagnostic rerun (fresh build reused; one GUI gate): the harness
removed the 20 Backspace burst and appended the lowercase `-edit` suffix,
then required a typed redraw before Apply. The gate reached the typed redraw,
Apply, Save, and Reload screenshot waits, but the first failure remained
`Mica Editor endpoint reclaim marker count did not reach 4` after
`input-editor-close`. Editor pointer markers still stopped at exactly widget
4 (document) then widget 5 (line input); there were no widget 6 (Apply), 10
(Save), or 9 (Reload) markers. Editor FS markers were only two
`operation=13 entered`/`returned` pairs; no operation 12 or subsequent FS
request appeared. No CloseRequested, loaded/saved/status, VM exit, or
unregister marker arrived before signal-15 cleanup. Preserved Editor full-frame
CRCs were pre/typed/apply/saved/reload
`36405e82/d9cab3b7/d9cab3b7/404fc1eb/8d74dbd4`; the existing document crop
was `9b564f14` for all captures. This cadence change therefore did not move
the stall past line-input dispatch; no QEMU/fixture/endpoint residue remains.

Diagnostic Editor rerun (fresh `make build` succeeded, then one GUI gate): the
first error remained `Mica Editor endpoint reclaim marker count did not reach
4`. The exact Editor serial sequence after launch was: endpoint request;
`filesystem request operation=13 entered=true`; the matching
`operation=13 returned=true`; endpoint receive/client registration; shell
launch reply; VM verification; a second `operation=13 entered=true` followed
by its `returned=true`; and `gui presented`. There was no
`gui close requested delivered=true`, no `vm returned`/`exiting`, no endpoint
unregister, no `mica-editor loaded`/`saved`, and no process status before
cleanup. QMP did reach `input-editor-close`; the following reclaim wait was
the first failure. Therefore the diagnostic rules out an FS IPC hang for the
two observed `ReadRange` calls and points to CloseRequested delivery/hit
testing (or the event path) as the blocking boundary. The gate cleanup again
terminated QEMU with signal 15 and left no QEMU, fixture, or endpoint residue.

Segmented-input lifecycle rerun (fresh build reused; one GUI gate): the
harness synchronized each hit-test marker and used Alt+F4 for close. This run
reached the complete Editor pointer sequence `4, 5, 6, 10, 9` (document, line
input, Apply, Save, Reload), and Alt+F4 delivered CloseRequested. FS markers
were ReadRange `operation=13` entered/returned twice, WriteAtomic
`operation=12` entered/returned once, then Reload ReadRange `operation=13`
entered/returned once. Endpoint-1 unregister and `mica: pid=14 status=0` both
arrived. The first failure was `Mica Editor save marker did not arrive`:
serial had two loaded markers but no saved marker despite operation 12
returning. Editor screenshot CRCs (pre/focused/typed/apply/saved/reload) were
`36405e82/2419bc00/38c9bd74/54979b77/7fb4c62b/261bfa2e`; document crops were
`9b564f14/6a5e3924/6a5e3924/6a5e3924/e83418c7/e83418c7`, with Apply/Reload
content stable. Cleanup left no QEMU, fixture, or endpoint residue. This
rules out the prior callback/hit-test and close lifecycle stall; the remaining
issue is the missing user-visible save marker after successful WriteAtomic
IPC.

WriteAtomic overwrite-fix rerun (fresh `make build` succeeded; one GUI gate):
the segmented harness completed pointer sequence `4, 5, 6, 10, 9`, delivered
CloseRequested via Alt+F4, reclaimed endpoint 1, and observed
`mica: pid=14 status=0`. FS diagnostics were ReadRange op13 entered/returned
twice, WriteAtomic op12 entered/returned once, and Reload ReadRange op13
entered/returned once. Serial now contains two
`mica-editor loaded path=/data/editor-note.txt` markers and
`mica-editor saved path=/data/editor-note.txt bytes=44 atomic=true fsync=true`.
The first failure moved to the final document persistence check:
`Mica Editor reload changed visible document content: apply=6a5e3924
reload=e83418c7`. Full-frame CRCs pre/focused/typed/apply/saved/reload were
`36405e82/2419bc00/f524ca4c/54979b77/7fb4c62b/fefbd44f`; crop CRCs were
`9b564f14/6a5e3924/6a5e3924/6a5e3924/e83418c7/e83418c7`, so Save and Reload
both completed but the applied view and reloaded view differ. Cleanup left no
QEMU, fixture, or endpoint residue.

Final Apply-wait rerun (fresh build reused; one GUI gate): static checks passed
and Apply now waits against the typed screenshot bytes. The Editor lifecycle
fully completed: pointer sequence `4, 5, 6, 10, 9`; FS op13 entered/returned
twice, WriteAtomic op12 entered/returned, Reload op13 entered/returned;
CloseRequested, endpoint-1 unregister, `mica: pid=14 status=0`, two loaded
markers, and `mica-editor saved ... bytes=44 atomic=true fsync=true` all
arrived. The final checker still stopped at
`Mica Editor reload changed visible document content: apply=6a5e3924
reload=e83418c7`; the fresh Apply wait did not change the crop values because
the Apply screenshot remains `6a5e3924`, while Saved/Reload are both
`e83418c7`. Full-frame CRCs pre/focused/typed/apply/saved/reload were
`45c9191b/2419bc00/38c9bd74/54979b77/7fb4c62b/fefbd44f`. QMP and serial show
the complete interaction and cleanup; no QEMU, fixture, or endpoint residue
remains. This is solely a visual crop assertion mismatch (Apply capture
versus Saved/Reload), not a lifecycle, FS, or save-marker failure.

Reload-selection production-fix rerun: OrbStack targeted host tests passed
23/23, fresh `make build` exited 0, and one GUI gate completed the full Editor
lifecycle. Pointer sequence was `4, 5, 6, 10, 9`; FS op13 entered/returned
twice, WriteAtomic op12 entered/returned once, and reload op13
entered/returned once. CloseRequested, endpoint-1 unregister,
`mica: pid=14 status=0`, two loaded markers, and the saved marker
`bytes=44 atomic=true fsync=true` all arrived. The final checker still stopped
at `Mica Editor reload changed visible document content: apply=6a5e3924
reload=e83418c7`, with full CRCs pre/focused/typed/apply/saved/reload
`45c9191b/2419bc00/f524ca4c/54979b77/7fb4c62b/fefbd44f` and document crops
`aa4a15a2/6a5e3924/6a5e3924/6a5e3924/e83418c7/e83418c7`. QMP/serial show
complete interaction and status 0; cleanup left no QEMU, fixture, or endpoint
residue. The production reload-selection change did not alter this visual
crop mismatch; save and reload markers remain successful.

Final Editor GUI gate PASS after narrowing the stable crop to the first
document row. Static checks passed (`bash -n`, 3/3 embedded Python AST, and
whitespace); the current fresh build was reused and `scripts/gui-qemu.sh`
exited 0. Editor pointer sequence was `4, 5, 6, 10, 9`; filesystem markers
were op13 entered/returned twice, WriteAtomic op12 entered/returned once, and
reload op13 entered/returned once. CloseRequested, endpoint-1 unregister,
`mica: pid=14 status=0`, two loaded markers, and
`mica-editor saved ... bytes=44 atomic=true fsync=true` all passed. The
browser fixture still recorded exactly `GET /index.html` and `GET /next.html`.
Decoded Editor full-frame CRCs (pre/focused/typed/apply/saved/reload) were
`45c9191b/2419bc00/38c9bd74/54979b77/7fb4c62b/fefbd44f`; the stable crop
`(82,164,820,190)` was `cb682bf4` for pre and `f8e89828` for focused/typed/
apply/saved/reload, proving the edited surrounding content persisted through
Save and Reload. Final QMP/serial checker passed and cleanup left no QEMU,
fixture, endpoint, or helper-process residue.

Final unified OrbStack acceptance: `make test` exited 0 and its complete
output is preserved in `target/unified-editor-final.log`. Host coverage was
green, including Mica `23 passed; 0 failed`, MFS recovery 12/12, ABI 7/7,
GUI command stream 6/6, desktop 8/8, kernel boundaries 11/11, smoke QEMU,
powercut, all five fault-injection stages plus GC proof, the GUI gate (Reader
browser and Editor), SSH/Mica sessions, and the Mica TLS fixture. The GUI gate
reported PASS and the SSH gate reported PASS with Mica eval/REPL/file/denied,
disconnect recovery, and key rejection statuses. `make fsck` then exited 0;
`target/unified-editor-fsck.log` reports
`MFS1 clean generation=1520 transactions=1426 entries=24 used_blocks=11887`.
Final process inspection found no QEMU, gate, fixture, or helper processes.

Long-timeout CLI targeted acceptance: implementation review confirmed both
serial shell and SSH parsers accept `1ms..24h` (`ms`/`s` forms), while
non-GUI sessions retain a hard `60s` maximum; GUI file sessions may use the
full `24h`. No existing shell/SSH parser-test module exists, so no duplicate
test infrastructure was added. OrbStack service compile passed for shell,
sshd, and Mica service; fresh `make build` exited 0; and one `gui-qemu.sh`
gate exited 0. The Editor command used the production entrypoint
`mica --gui --timeout 86400s ... /mica/editor.mica -- /data/editor-note.txt`.
Editor pointer sequence `4,5,6,10,9`, FS op13×2/op12×1/reload op13×1, loaded×2,
saved `bytes=44 atomic=true fsync=true`, CloseRequested, endpoint reclaim, and
status 0 all passed. Final decoded gate CRCs included Editor
pre/apply/saved/reload `36405e82/54979b77/7fb4c62b/fefbd44f`; browser fixture
remained exactly GET index+next. No QEMU, fixture, or helper processes
remained after cleanup.

Desktop Editor icon gate (fresh build, one GUI run, first-error stop): static
checks passed (`bash -n`, three embedded Python AST blocks) and the Editor icon
flow used the production center `(972,682)`. Existing Counter/Reader/Editor
shell flow completed before the new case. The icon click emitted
`[gui] desktop icon clicked application=5 launch-requested=true`, init emitted
`[gui] desktop launch application=5 accepted=true`, and endpoint 1 requested,
received and registered. The first error was waiting for the new GUI Present
count (`desktop Editor first Present marker count did not reach 6`); serial
then showed `[service] critical service init exited status=4`, followed by
`xtask: command failed with exit status: 1`. No duplicate-click, minimize,
close/reclaim or restart assertions ran. Cleanup removed QEMU/fixture/FIFO
state; the preserved serial log is `target/gui-qemu.log`, and the fixture log
still contains only `GET /index.html` and `GET /next.html`. This is a failed
icon-launch gate, not a PASS; host ABI 8/8 and GUI command 6/6 + desktop 9/9
remain green.

Desktop Editor icon gate diagnostic rerun after removing detached-session
stdout waits: current fresh build reused (`microsystem-init` ELF 02:22:08,
source 02:20:31); static `bash -n` and diff checks passed. One OrbStack
`scripts/gui-qemu.sh` run exited 0. The icon flow observed application=5
click/accept, endpoint-1 request/receive/register, FS operation=13
entered/returned, and Present. Duplicate click and minimize/restore did not
create another session; first close reclaimed endpoint 1, and the second click
created a second registration/Present and reclaimed endpoint 1 again. Icon
screenshots were `pre=bd8eb91c`, `active=2063544c`, `restored=131a1460`,
`restart=e5017cd5`. Existing browser fixture remained exactly GET index+next;
Counter/Reader/Editor shell flows, final Counter close, and SSH status 0 also
passed. No QEMU, fixture, or helper process remained after cleanup. Loaded/new
stdout was intentionally not asserted because detached serial sessions drain
stdout only on process exit; FS read + Present are the load evidence.

Desktop Editor icon gate diagnostic rerun (fresh production build, one GUI run,
first-error stop): `make build` exited 0; `services/init/src/main.rs` was
02:20:31 and the rebuilt `microsystem-init` ELF was 02:22:08. The icon flow
reported click/application=5, init accepted application=5, endpoint 1
requested/received/registered, and an atomic GUI Present. The first checker
error was `desktop Editor did not report loaded/new /data/note.txt`; therefore
duplicate-click, minimize/restore, close/reclaim, and restart assertions did
not run. The preserved serial tail ends after the first Present and QEMU
cleanup (`qemu-system-aarch64: terminating on signal 15`); no fixture/QEMU
process remained. Existing Counter/Reader/shell Editor lifecycle markers
completed before this failure. This is a failed icon-launch gate: no
`mica-editor loaded path=/data/note.txt` marker was observed, despite
registration and Present succeeding.

Correction: the diagnostic failure immediately above predates the detached
stdout-wait removal. The subsequent fresh-build gate is the authoritative
result: icon launch, duplicate suppression, minimize/restore, endpoint
reclaim, and restart all passed (see the PASS entry above).

Final unified desktop-icon acceptance: one OrbStack `make test` exited 0 with
complete output in `target/unified-desktop-icons-final.log`; one subsequent
`make fsck` exited 0 with `target/unified-desktop-icons-fsck.log`. Host suites
were green (ABI 12/12, capability ABI 8/8, GUI command stream 6/6, desktop
9/9, kernel boundary 11/11, Mica core 23/23). Smoke, powercut, five fault
injection stages plus GC proof, GUI, SSH/Mica, and TLS gates all reported PASS.
The unified GUI gate reported icon JSON `application=5`, launches=2,
registrations=2, Presents=2, reclaims=2; CRCs were pre `bd8eb91c`, active
`2063544c`, restored `d6783cf9`, restart `e5017cd5`. SSH reported all Mica
statuses green, including disconnect recovery and key rejection. Fsck tuple:
`MFS1 clean generation=1626 transactions=1526 entries=24 used_blocks=13420`.
Final process inspection found no QEMU, GUI gate, fixture, mfsctl, or helper
processes.

### UI visual refresh targeted GUI gate (2026-08-13)

按要求先执行了静态检查，再复用同一份 fresh OrbStack `make build` 产物；
未执行统一 `make test` 或 `make fsck`。`bash -n scripts/gui-qemu.sh` 与
`git diff --check -- scripts/gui-qemu.sh` 均通过。视觉刷新将活动标题色从旧值
同步为 `TITLE_ACTIVE=0x1e3b63`；几何识别仍使用原有阈值。第一次针对性 gate
发现稳定客户区裁剪包含 active/inactive 外框，第二次诊断又定位到右下 resize
grip；因此稳定性裁剪精确收窄为 display-list 内部 `(80,110,478,382)`，未放宽
任何 CRC 或几何断言。

最终唯一授权的 targeted `scripts/gui-qemu.sh` 运行 exit 0，输出：

```text
gui-qemu: PASS serial-log=/workspace/target/gui-qemu.log \
qmp-log=/workspace/target/gui-qemu.qmp.jsonl \
pre-screenshot=/workspace/target/gui-screendump-pre.png \
post-screenshot=/workspace/target/gui-screendump.png
```

QMP 解码确认 1024x768 PPM-P6（14230 bytes），CRC/几何结果如下（完整 JSON
保存在 `target/gui-qemu.qmp.jsonl`）：

```text
decoded_pixels:
desktop_pre=b1998cf3 mica_pre=73406eca mica_button=0ac1c1b2
mica_text=c4b4e4df ascii=2c12f66a resized=1b6e9d97 max=d8999c4e
restore=1b6e9d97 minimized=17b9de13 taskbar_restore=47044581
first_before_second=47044581 second_pre=e8dea293 second_button=8800ad96
second_text=aed39a4e after_second_close=bb2329f6 after_reuse_close=bb2329f6
browser_pre=93ac1082 browser_link=7e50ac76 browser_cross=2f4d26d3
editor_pre=8d767f5a editor_apply=da105c73 editor_saved=18f38838
editor_reload=7e895e1d changed=true
geometry: max_active_title_row10=962 resized_active_title_row80=418
restore_active_title_row80=418 minimized_active_title_row90=0
taskbar_active_row750=121
stable_first_client_before_second=d1d0b421
stable_first_client_baseline=d1d0b421
stable_first_client_after_second=d1d0b421
stable_first_client_after_reuse=d1d0b421
```

Serial markers covered bootfs `24/23`, resident EL0 `12/12`, dynamic capacity 8,
VirtIO-GPU/windowd 1024x768, desktop 3 windows + 5 launchers, keyboard/tablet
routing and Terminal uptime, Unicode U+20013 delivery/rendering, Mica endpoint
registration/Present/reclaim and slot reuse. Reader loaded HTTP 200 pages and the
fixture recorded exactly `GET /index.html` and `GET /next.html`; Editor load/save/
reload completed with `bytes=44 atomic=true fsync=true`; desktop Editor icon app=5
reported launches=2, Presents=2, reclaims=2. No panic or DMA fault appeared.

Cleanup removed QMP socket, serial FIFO, temporary fixtures and image backup;
`build/microsystem.img` was restored (SHA-256
`54f32698f2686d0f965705e08197b7beacea5ac3968a06cf8152893b6a565945`). The expected
`target/gui-browser-fixture.log` remains with the two GET lines. No QEMU or test
container residue remains. A final unified `make test`/`make fsck` is advisable as
the release-level follow-up, but was intentionally not run pending main-agent
authorization.

### Final unified UI-refresh acceptance (2026-08-13)

按授权冻结工作树后，从宿主 OrbStack 严格执行了唯一一次：

```text
make test > target/unified-ui-refresh-final.log 2>&1   # exit 0
make fsck  > target/unified-ui-refresh-fsck.log 2>&1   # exit 0
```

Host suites 全部通过：MFS1 recovery 12/12、capability ABI 8/8、GUI
command stream 6/6、Desktop 9/9、kernel boundaries 11/11、Mica core 23/23，
合计 69 passed / 0 failed（unit/doc-test harnesses 均为 0 failures）。QEMU/
integration stages 全部 PASS：smoke；powercut（first=0, second=0）；五个
fault cuts（records-written old、data-flushed old、superblock-written new、
superblock-flushed new、GC-candidate-superblock-flushed gc-proof；预期
first=137 kill、second=0，GC fill=4,194,304 bytes/1 iteration）；GUI；SSH；
Mica/TLS fixture。GUI 仍确认 1024x768 XRGB8888、3 built-in windows、5 fixed
launchers；SSH 记录 uptime 30958 ms，Mica eval/repl/file/denied 均 0，断开
124，错误/无 key 均 255；Mica bootfs=24/23、first-pid=13、output=42/status=0，
TLS trusted 成功且 unknown-CA/expired/not-yet-valid/hostname-mismatch 均拒绝。

最终 fsck：

```text
MFS1 clean generation=1675 transactions=1572 entries=24 used_blocks=13420
```

最终只读残留核对：没有 QEMU、测试进程或 `microsystem-dev:rust-1.97.1`
容器；当前运行目录无 FIFO、QMP socket 或临时 fixture。保留的
`target/gui-browser-fixture.log`、`target/mica-dns-fixture.log`、Mica 手工
fixture/helper 与既有 fault-injection 历史目录属于测试证据。发现一个来自
此前运行、非本次 run 的陈旧零字节 FIFO：
`target/fault-injection-1786510519-706-6347/transaction-data-flushed.in`
（2026-08-12）；本次目录 `target/fault-injection-1786601439-724-25074` 无 FIFO，
未删除该用户既有残留。

### Browser loading Present / glyph-cache optimization (2026-08-13)

本轮只改测试契约与 GUI gate：`crates/mica/tests/core.rs` 的真实服务
PRELUDE/browser 编译测试现在编译 `app:present()`，并要求 shipped browser
保留 Loading 状态、`app:present()` 和局部 `source_length` 缓存；
`scripts/gui-qemu.sh` 的启动门禁新增精确 marker
`[gui] unifont runtime loaded=true cache=128 fallback=ascii`。未修改生产代码，
未运行统一 `make test`/`make fsck`。

优化前诊断（来自停止的 live GUI 实例）显示，Reader 点击后先出现多次
filesystem ReadRange 与网络 first-tx/first-rx，之后才出现首个
`[mica] gui presented...`；本轮不引入脆弱的时间阈值，只验证首个 Present
在 blocking 请求前可观察且 Reader 生命周期仍完整。

OrbStack targeted Mica core：

```text
docker run --rm -e RUSTUP_TOOLCHAIN=1.97.1 \
  -v "$PWD:/workspace" -w /workspace microsystem-dev:rust-1.97.1 \
  cargo test -p microsystem-mica --test core
test result: ok. 23 passed; 0 failed
```

随后 fresh `make build` exit 0，重新构建 windowd/Mica/kernel 并写入 Reader
asset。唯一一次 `scripts/gui-qemu.sh` exit 0：

```text
gui-qemu: PASS serial-log=/workspace/target/gui-qemu.log \
qmp-log=/workspace/target/gui-qemu.qmp.jsonl \
pre-screenshot=/workspace/target/gui-screendump-pre.png \
post-screenshot=/workspace/target/gui-screendump.png
```

关键 Reader/cache 证据：

- serial 首次启动命中 `unifont runtime loaded=true cache=128 fallback=ascii`；
- Reader endpoint 注册、Present、关闭、回收完成；其首个
  `gui presented widgets=true atomic=true isolated=true` 位于两条
  `mica-reader loaded status=200 lines=4/3` 之前；
- fixture 严格收到 `GET /index.html` 与 `GET /next.html` 各一次，无跨域请求；
- Reader same-origin 与 cross-origin screenshots 均变化，最终 QMP 解码
  1024x768 PPM-P6、14202 bytes。

最终 decoded CRC/geometry（完整 JSON 保存在 `target/gui-qemu.qmp.jsonl`）：

```text
desktop_pre=b1998cf3 mica_pre=73406eca mica_button=e638a31e
mica_text=6edde778 ascii=9135678d resized=b1079e30 max=524aacc7
restore=b1079e30 minimized=17b9de13 taskbar_restore=ed6d4626
first_before_second=ed6d4626 second_pre=e8dea293 second_button=64f9cf3a
second_text=8039422e after_second_close=114a2a51 after_reuse_close=114a2a51
browser_pre=2286d988 browser_link=5dab179e browser_cross=840230d6
editor_pre=3eeb31ac editor_apply=da105c73 editor_saved=18f38838
editor_reload=7e895e1d changed=true
geometry: max_active_title_row10=962 resized_active_title_row80=418
restore_active_title_row80=418 minimized_active_title_row90=0
taskbar_active_row750=121
stable_first_client_before_second=9014e4dd
stable_first_client_baseline=9014e4dd
stable_first_client_after_second=9014e4dd
stable_first_client_after_reuse=9014e4dd
```

其余 GUI 生命周期也通过：Editor pointer `4,5,6,10,9`、load/save/reload
（`bytes=44 atomic=true fsync=true`）、desktop Editor app=5 launches=2 /
Presents=2 / reclaims=2；无 panic 或 DMA fault。清理移除了 QMP socket、serial
FIFO、临时 fixture 与 image backup；保留 `target/gui-browser-fixture.log`
（两条 GET）作为证据，无当前 QEMU、GUI helper 或测试容器残留。

### Isolated SSH transport recheck before browser-perf unified gate (2026-08-13)

按授权冻结工作树后，OrbStack 单次 `scripts/ssh-qemu.sh` exit 0：

```text
ssh-qemu: PASS ssh-port=2223 gui-port=5901 uptime=uptime: 35562 ms \
  mica-eval-status=0 mica-repl-status=0 mica-file-status=0 \
  mica-denied-status=0 mica-disconnect-exit=124 recovery-status=0 \
  wrong-key-exit=255 no-key-exit=255
```

真实 SSH/Mica 观测包括 authorized eval `42`、完整 REPL（`Mica 0.1`、
`print(6 * 7)`、`42`、`exit`、返回 `micro>`）、文件脚本
`ssh-mica-file=ssh-mica-ok`、拒绝策略 `ssh-mica-denied kind=access`、
意外断开 exit 124 后 uptime recovery；错误 key 与无 key 均为 255。串口含
`sshd ready`、transport first-read/first-write/random、Mica VM
`returned`/`exiting`，无 panic/DMA/translation/allocator fault。

清理后无 QEMU、OrbStack 测试容器、SSH socket/FIFO 或临时 SSH fixture；仅保留
脚本预期的测试输出与 `target/ssh-mica-fixture` 证据目录。

### Browser-perf unified acceptance after smoke UART parser fix (2026-08-13)

`scripts/smoke-qemu.sh` 的最小测试层修复只处理已复现的 UART 字节交错：
`run: resourceprobe pid=13` 可能与下一条 `[app]` 写入拼接，原始行提取会把
`pid=13` 截成 `pid=1`。脚本现在从完整 resource-cleanup entry marker 恢复
数字 PID，并在 bounded joined-window 中关联 command；entry/live/wait/reclaim/
loader 的全部 marker 与顺序断言保持不变。`bash -n` 与 `git diff --check`
通过。此次 OrbStack targeted smoke 单次 exit 0：

```text
smoke-qemu: PASS (serial milestones present)
```

随后按授权各执行一次统一门禁：`make test > target/unified-browser-perf-final3.log
2>&1` exit 0，紧接 `make fsck > target/unified-browser-perf-fsck.log 2>&1`
exit 0；没有重跑。Host suites 共 69 passed / 0 failed：mfs1 recovery 12/12、
microsystem-abi capability_abi 8/8、microsystem-gui command_stream 6/6 +
desktop 9/9、microsystem-kernel core_boundaries 11/11、microsystem-mica core
23/23（unit/doc-test harnesses 均 0 tests）。

阶段结果：smoke PASS；powercut PASS（first=0 second=0）；fault 五案例全部
PASS（四个 transaction stage 与 `gc-candidate-superblock-flushed`，first=137
为预期注入终止、second=0，GC validated fill=4194304 bytes/1 iteration，
stats generation=1777 used=13826 free=2302 capacity=16128 total=16384）；
GUI PASS；SSH PASS（port 2223，uptime 29511ms，Mica eval/REPL/file/deny
均 status 0，disconnect 124，recovery 0，wrong/no-key 255）；Mica/TLS PASS
（bootfs 24/23、first-pid=13、output=42，trusted=true；unknown-ca、expired、
not-yet-valid、hostname-mismatch 均 rejected）。

GUI 优化契约再次通过：serial 命中
`[gui] unifont runtime loaded=true cache=128 fallback=ascii`；Reader 的
`gui presented` 在 `mica-reader loaded status=200 lines=4` 与 `lines=3`
之前；fixture 仅收到 `GET /index.html`、`GET /next.html`。QMP final decoded
JSON 保存在 `target/gui-qemu.qmp.jsonl`，PNG 1024x768/14202 bytes，CRC 为：

```text
desktop_pre=b1998cf3 mica_pre=73406eca mica_button=e638a31e mica_text=6edde778
ascii=9135678d resized=b1079e30 max=524aacc7 restore=b1079e30 minimized=6ef3a5e3
taskbar_restore=ed6d4626 first_before_second=ed6d4626 second_pre=e8dea293
second_button=64f9cf3a second_text=8039422e after_second_close=114a2a51
after_reuse_close=114a2a51 browser_pre=2286d988 browser_link=5dab179e
browser_cross=840230d6 editor_pre=3eeb31ac editor_apply=da105c73
editor_saved=18f38838 editor_reload=7e895e1d changed=true
stable_first_client=(baseline,before-second,after-second,after-reuse)=9014e4dd
geometry=(max row10,resized row80,restore row80,minimized row90,taskbar row750)
=(962,418,418,0,121)
```

Final fsck tuple from the authorized single run:
`MFS1 clean generation=1789 transactions=1680 entries=24 used_blocks=13420`.
Read-only residue check found no QEMU/test process, OrbStack test container,
current QMP socket or FIFO, or temporary GUI/Mica/SSH fixture directory. The
pre-existing stale FIFO `target/fault-injection-1786510519-706-6347/
transaction-data-flushed.in` (2026-08-12) remains untouched; expected fixture
logs/assets (`target/gui-browser-fixture.log`, `target/mica-dns-fixture.log`,
`target/ssh-mica-fixture`) remain as evidence.

### Terminal filesystem command gate (2026-08-13)

本轮 Terminal 变更的现实回归边界是：shell 必须解析 `stat`、`mv`、`rm` 的
参数；Terminal 的 GUI 共享 frame/capability 必须隔离；并且通过真实 GUI/MFS
链路完成 `mkdir/write/stat/mv/cat/ls/rm/sync`，输出不能退回旧的 40-byte
inline 截断路径。为此仅扩展了现有 shell parser 单元测试（2/2），并在现有
GUI gate 复用 Terminal 服务 marker 与逐条 filesystem marker；未修改生产代码。

静态检查通过：`bash -n scripts/gui-qemu.sh`、嵌入 QMP Python AST parse、以及
`git diff --check`。OrbStack targeted host tests exit 0：

```text
microsystem-shell: 2 passed / 0 failed
microsystem-gui command_stream: 6/6; desktop: 9/9
```

随后 fresh `make build > target/terminal-fs-build.log 2>&1` exit 0。首次 GUI
gate 仅一次、首错停止，失败为：

```text
RuntimeError: terminal mkdir filesystem command marker count did not reach 1
```

Terminal help screenshot（`target/gui-qemu-terminal-help.png`）仍为 1024x768，
离线计数得到 10 个 14-pixel foreground glyph bands（140 rows），证明长 help
输出已经走 4096-byte shared frame，而不是旧 40-byte inline reply。QMP/serial
均到达 GPU/windowd/input/terminal/cache=128/uptime/help；mkdir marker 尚未
出现，未触发后续 filesystem commands。无 panic/DMA fault，QEMU 以正常
SIGTERM cleanup；QMP socket、serial FIFO、临时 fixtures 均清理。

根据该次证据，harness 仅作最小输入队列适配：每条命令按 4 字符拆为独立
`input-send-event`，段间 50ms，回车单独发送，并继续等待每条 marker。复用同一
fresh build 的唯一重跑使用独立证据路径 `target/terminal-fs-gui-rerun.*`，仍
首错停止，首错为：

```text
RuntimeError: terminal stat filesystem command marker count did not reach 1
```

本次 QMP 已完整送出 mkdir/write 的所有 chunks 和独立回车；serial 明确记录：

```text
[gui] terminal filesystem command=mkdir status=ok
[gui] terminal filesystem command=write status=ok
```

stat 的所有 chunks/回车也收到 QMP `return`，但没有 stat filesystem marker，
因此 `mv/cat/ls/rm/sync` 尚未执行。启动、windowd、Terminal shared-frame、
`cache=128`、input、shell markers 均存在；无 panic/DMA fault。清理后无 QEMU、
QMP socket、serial FIFO、临时 fixture 或测试容器；rerun 的 log/QMP/PNG 证据
保留，既有 `target/gui-browser-fixture.log` 未删除。由于已按首错停止，本轮不
再重跑 GUI gate，也未运行统一 `make test`/`make fsck`。

主线随后修正了 `fs_request` 对 `Stat` 回复首 word 的误判（仅 List/Read/
ReadRange 才把它当 shared payload length）。按授权重新执行 fresh
`make build > target/terminal-fs-build-final.log 2>&1`，exit 0；build log 显示
`microsystem-terminal` 与 kernel 均重新编译（仅保留既有 shell unreachable
`mv` warning）。之后仅重跑一次 GUI gate，仍使用 4 字符分段、50ms 间隔及独立
return，并使用 `target/terminal-fs-gui-final.*` 保存证据。该 gate 首错仍为：

```text
RuntimeError: terminal stat filesystem command marker count did not reach 1
```

QMP 收到 mkdir/write/stat 的全部分段及回车；serial 记录 mkdir、write 成功，
但未记录 stat marker，因此后续 mv/cat/ls/rm/sync 未发送。启动、Terminal
shared frame、`cache=128`、windowd/input/shell markers 均通过；无 panic/DMA
fault。QEMU 正常 SIGTERM cleanup，QMP socket、serial FIFO、临时 fixtures 和
OrbStack gate 容器均不存在，final log/QMP/PNG 保留。按首错策略不再重跑，也未
执行统一 `make test`/`make fsck`。

为区分长路径输入丢失与文件系统行为，随后按授权把 GUI fixture 缩为
`mkdir /tfs`、`write /tfs/a proof`、`stat /tfs/a`、`mv /tfs/a /tfs/b`、
`cat /tfs/b`、`ls /tfs`、按“先文件后目录”执行两次 `rm`，最后 `sync`；仍保留
4 字符分段、50ms 间隔、独立 return 与逐 marker 等待。静态检查通过；复用
`terminal-fs-build-final.log` 的 fresh build，唯一 GUI gate 使用
`target/terminal-fs-gui-final2.*`，首错为：

```text
RuntimeError: terminal cat filesystem command marker count did not reach 1
```

这次 serial/QMP 已验证：

```text
mkdir status=ok
write status=ok
stat status=ok
mv status=ok
```

cat 的全部分段及独立回车均收到 QMP `return`，但没有 cat marker，因此 ls/rm/
sync 未发送。启动、Terminal shared frame、`cache=128`、windowd/input/shell
markers 均通过；无 panic/DMA fault。QEMU 正常 SIGTERM cleanup，QMP socket、
serial FIFO、临时 fixtures 与 gate 容器均不存在，final2 log/QMP/PNG 保留。
按首错策略不再重跑，也未运行统一 `make test`/`make fsck`。

最后按授权将 Terminal 输入进一步收紧为每个字符独立 `input-send-event`（down/up
各一次、字符间 100ms），return 仍独立发送；保留短路径 fixture 与精确
`command=<name> status=ok` marker。复用 `terminal-fs-build-marker.log` 的 fresh
build，唯一 GUI gate exit 0/PASS，完整 Reader/Editor/桌面图标生命周期继续执行。

Terminal/MFS 真实序列在 serial 中完整有序：

```text
mkdir status=ok
write status=ok
stat status=ok
mv status=ok
cat status=ok
ls status=ok
rm status=ok
rm status=ok
sync status=ok
```

`target/terminal-fs-gui-final3-terminal-help.png` 解码为 1024x768，foreground
row 计数 140（10 个 glyph bands），证明 help 走 4096-byte shared frame；不再
使用 40-byte inline 截断。最终 QMP JSON（`target/terminal-fs-gui-final3.qmp.jsonl`）
包含 PPM-P6 1024x768、15046 bytes；decoded CRC/几何为：

```text
desktop_pre=b1998cf3 terminal_help=8111b4d3
browser_pre=2286d988 browser_link=5dab179e browser_cross=840230d6
editor_pre=3eeb31ac editor_apply=da105c73 editor_saved=18f38838
editor_reload=7e895e1d changed=true
max_active_title_row10=962 resized_active_title_row80=418
restore_active_title_row80=418 minimized_active_title_row90=0 taskbar_active_row750=121
stable_first_client=(before-second,baseline,after-second,after-reuse)=9014e4dd
```

关键 Reader 证据：`gui presented widgets=true atomic=true isolated=true` 位于
`mica-reader loaded status=200 lines=4`、`lines=3` 之前；fixture 严格只有
`GET /index.html` 与 `GET /next.html`。Mica SSH status=0；Editor icon
launches/presents/reclaims=2；无 panic/DMA fault。清理后无 QEMU、OrbStack gate
容器、QMP socket、serial FIFO 或临时 fixture 目录；`target/terminal-fs-gui-final3.*`
日志、QMP 与截图保留。统一 `make test`/`make fsck` 仍未执行。

随后主线为 filesystem 错误回复增加了精确的串口 `status=<reason>` 诊断；
harness 也收紧 marker 为完整的 `command=<name> status=ok` 行，避免错误状态
被误判为成功。静态检查通过后 fresh
`make build > target/terminal-fs-build-marker.log 2>&1` exit 0（Terminal 与
kernel 重新编译）。唯一 GUI gate 使用 `target/terminal-fs-gui-marker.*`，首错
为：

```text
RuntimeError: terminal write filesystem command marker count did not reach 1
```

QMP 已收到 write 的所有 4 字符分段及独立 return；serial 只记录 mkdir
`status=ok`，没有 write `status=ok`，也没有新增 `status=<reason>` 诊断行，故
该输入未被识别为可报告的 filesystem write（后续 stat/mv/cat/ls/rm/sync 未送）。
启动、Terminal shared frame、`cache=128`、windowd/input/shell markers 均通过；
无 panic/DMA fault。QEMU 正常 SIGTERM cleanup，QMP socket、serial FIFO、临时
fixtures 和 gate 容器均不存在；marker run 的 log/QMP/PNG 保留。按首错策略不
再重跑，也未运行统一 `make test`/`make fsck`。

最终权威 GUI gate（`target/terminal-fs-gui-final3.*`）以每字符独立输入和精确
`status=ok` marker exit 0/PASS；此前 mkdir/stat/cat/write 首错均为 QMP 批量
输入丢键造成的中间诊断，已被该完整 PASS 覆盖，不代表生产 filesystem failure。
其最终 FS 序列、help shared-frame 视觉断言、Reader/Editor 生命周期及 cleanup
证据见上文。

主线随后移除了 parser 的 unreachable `mv` arm；相应回归断言将多参数
`rm /data/new extra` 期望同步为 `ParseError::Invalid`。OrbStack targeted
shell library test（无需重跑 build/QEMU）通过：2 passed / 0 failed。

### Unified Terminal filesystem acceptance (2026-08-13)

按授权冻结工作树，从宿主 OrbStack 唯一执行：
`make test > target/unified-terminal-fs-final.log 2>&1` exit 0；随后紧接唯一
`make fsck > target/unified-terminal-fs-fsck.log 2>&1` exit 0，无重跑。

Host suites 全部通过，共 69 passed / 0 failed：mfs1 recovery 12/12、ABI
capability_abi 8/8、GUI command_stream 6/6 + desktop 9/9、kernel
core_boundaries 11/11、Mica core 23/23；unit/doc-test harness 均 0 tests。
新增 shell parser targeted test 已在前一阶段独立通过 2/2。

集成阶段均 PASS：smoke serial milestones（file-chain mkdir/write/sync/ls/cat
均 ok）；powercut first=0 second=0；fault 五案例全部 PASS（records-written
old、data-flushed old、superblock-written new、superblock-flushed new、
gc-candidate-superblock-flushed gc-proof；各 first=137 注入终止、second=0，
GC fill=4194304 bytes/1 iteration，stats generation=1862 used=13826 free=2302
capacity=16128 total=16384）；GUI PASS；SSH PASS（port 2223，uptime 31423ms，
Mica eval/REPL/file/deny status=0，disconnect=124、recovery=0、wrong/no-key=255）；
Mica/TLS PASS（bootfs=24/23、first-pid=13、output=42/status=0；trusted=true，
unknown-ca/expired/not-yet-valid/hostname-mismatch 均 rejected，combo 全 true）。

统一 GUI serial 的 Terminal 证据完整有序：

```text
[gui] unifont runtime loaded=true cache=128 fallback=ascii
[gui] terminal filesystem command=mkdir status=ok
[gui] terminal filesystem command=write status=ok
[gui] terminal filesystem command=stat status=ok
[gui] terminal filesystem command=mv status=ok
[gui] terminal filesystem command=cat status=ok
[gui] terminal filesystem command=ls status=ok
[gui] terminal filesystem command=rm status=ok
[gui] terminal filesystem command=rm status=ok
[gui] terminal filesystem command=sync status=ok
```

同一 serial 命中 windowd 1024x768、clients=3、desktop move/resize/minimize/
maximize/alt-tab、launcher icons=5、keyboard+tablet；Reader 的
`gui presented widgets=true atomic=true isolated=true` 先于
`mica-reader loaded status=200 lines=4` 与 `lines=3`，fixture 仅有
`GET /index.html`、`GET /next.html`。统一 QMP PNG 为 1024x768 PPM-P6、15046
bytes；decoded CRC（`target/gui-qemu.qmp.jsonl`）包括
`terminal_help=439d6877`、`browser_pre=2286d988`、`browser_link=5dab179e`、
`browser_cross=840230d6`、`editor_pre=3eeb31ac`、`editor_apply=da105c73`、
`editor_saved=18f38838`、`editor_reload=7e895e1d`、`changed=true`，geometry
`(max,row10; resized,row80; restore,row80; minimized,row90; taskbar,row750)
=(962,418,418,0,121)`，stable client crop 全阶段 `9014e4dd`；help foreground
rows=140。

最终 fsck tuple：`MFS1 clean generation=1874 transactions=1759 entries=24
used_blocks=13420`。只读残留核对无 QEMU、测试/helper 进程、当前 OrbStack
`microsystem-dev:rust-1.97.1` 容器、QMP socket、当前 FIFO 或临时 fixture；保留
预期 `target/gui-browser-fixture.log`、`target/mica-dns-fixture.log`、
`target/mica-manual/helper.mica`、`target/ssh-mica-fixture`。仅发现用户此前
遗留的旧 FIFO `target/fault-injection-1786510519-706-6347/
transaction-data-flushed.in`（非本次 run，未删除）。

### Partial-present / atomic replace validation (2026-08-14)

本轮新增的真实回归边界是 MFS1 `replace_file` 的单事务替换：掉电/写入、数据
flush、superblock 写入或 flush 失败后，`target` 必须保持 old-or-new 的完整快照，
不能丢失；内存重试必须得到 new 且临时源消失，并且 remount `check` 仍 clean。
同时验证 GUI partial Present 的边界矩形、VirtIO-net RX queue=2，以及既有
Terminal/Mica/Reader/Editor 生命周期和截图契约。

静态检查：`bash -n scripts/gui-qemu.sh` 通过；3 个内嵌 Python heredoc 均通过
AST 解析。OrbStack targeted `cargo test -p mfs1 --test recovery` 为 **15 passed /
0 failed**，包含新增 `replace_file_failure_matrix_is_atomic_and_retryable`。
随后 fresh `make build > target/partial-present-build.log 2>&1` exit 0。

授权的唯一有效 GUI gate 使用默认 `target/gui-qmp.sock`（xtask GUI runner 固定
该 socket），自定义证据保存在 `target/partial-present-gui-final.*`，exit 0/PASS。
串口关键 marker：

```text
[net] virtio-net ready ... rx-buffers=2
[gui] unifont runtime loaded=true cache=128 fallback=ascii
[gui] EL0 windowd partial-present=true rect=423x16+99,384
[gui] windowd ready resolution=1024x768 format=XRGB8888 clients=3 renderer=commands
[gui] desktop windows=3 move=true resize=true minimize=true maximize=true alt-tab=true
[gui] desktop launcher icons=5 fixed-registry=true click-to-open=true
```

Terminal filesystem sequence remained complete and ordered:

```text
mkdir status=ok, write status=ok, stat status=ok, mv status=ok,
cat status=ok, ls status=ok, rm status=ok, rm status=ok, sync status=ok
```

Reader serial proves `gui presented widgets=true atomic=true isolated=true` precedes
both `mica-reader loaded status=200 lines=4` and `lines=3`; fixture log contains only
`GET /index.html` and `GET /next.html`. Editor loaded/saved/reloaded with
`bytes=44 atomic=true fsync=true`; desktop Editor icon launches/presents/reclaims all
equal 2. Mica SSH status was 0.

Final QMP decoded pixels/geometry:

```text
desktop_pre=b1998cf3 mica_pre=7d6a4f44 mica_button=e8128290 mica_text=60f7c6f6
terminal_help=98e171ee ascii=9f1f4603 resized=957bdb75 max=524aacc7 restore=957bdb75
minimized=083b1435 taskbar_restore=c9110363 first_before_second=c9110363
second_pre=cca2e7d6 second_button=40858a7f second_text=a445076b
after_second_close=35366f14 after_reuse_close=35366f14
browser_pre=2286d988 browser_link=5dab179e browser_cross=f60453d9
editor_pre=3eeb31ac editor_apply=da105c73 editor_saved=18f38838 editor_reload=7e895e1d
changed=true
partial_present=423x16+99,384
max_active_title_row10=962 resized_active_title_row80=418 restore_active_title_row80=418
minimized_active_title_row90=0 taskbar_active_row750=121
stable_first_client=(before-second,baseline,after-second,after-reuse)=9014e4dd
terminal_help_foreground_rows=140
```

QMP PNG was 1024x768 PPM-P6 (15046 bytes). Cleanup passed: no exact QEMU process,
OrbStack test container, `target/gui-qmp.sock`, serial FIFO, or current GUI fixture
directories; expected `target/gui-browser-fixture.log` was retained with the two GETs.
No panic or DMA fault markers appeared. The earlier socket-override invocation stopped
before QMP creation because `xtask gui` hardcodes `target/gui-qmp.sock`; it was not a
runtime failure and was followed by the one authorized valid gate above. Unified
`make test`/`make fsck` were not run.

After the gate, the replace-file regression was tightened to inspect the on-disk state
immediately after each failed fsync (before in-memory retry): remount accepted only
`target=old,temp=new` or `target=new,temp=absent`, then `check` passed. OrbStack recovery
tests were rerun once and remained **15 passed / 0 failed**; the retry then proved final
`target=new,temp=absent` and clean remount.

### Unified P0/P1 review acceptance (2026-08-14)

按授权从宿主 OrbStack 冻结工作树执行唯一 `make test >
target/unified-p0-p1-review-final.log 2>&1`，exit **0**；随后紧接唯一
`make fsck > target/unified-p0-p1-review-fsck.log 2>&1`，exit **0**。

Host suites 全部通过，共 **73 passed / 0 failed**：MFS1 recovery **15/15**、ABI
capability **9/9**、GUI command_stream **6/6** + desktop **9/9**、kernel
core_boundaries **11/11**、Mica core **23/23**；unit/doc-test harnesses 均为 0
tests（无失败）。

集成阶段均 PASS：

- smoke serial milestones、scheduler/IPC/EL0 readiness 与 file-chain
  mkdir/write/sync/ls/cat=ok。
- powercut first-status=0、second-status=0。
- fault 五案例全部 PASS：transaction-records-written=old、transaction-data-flushed=old、
  transaction-superblock-written=new、transaction-superblock-flushed=new、
  gc-candidate-superblock-flushed=gc-proof；注入 first-status=137、恢复
  second-status=0，GC fill=4194304 bytes/1 iteration，stats
  generation=1947 used_blocks=13826 free_blocks=2302 capacity=16128 total=16384。
- GUI PASS；串口命中 `rx-buffers=2`、`cache=128`、`partial-present=true
  rect=423x16+99,384`，windowd/desktop/icon markers，以及完整 Terminal FS
  `mkdir,write,stat,mv,cat,ls,rm,rm,sync` 全为 `status=ok`。Reader 的
  `gui presented widgets=true atomic=true isolated=true` 先于两次 HTTP 200 load；
  fixture 仅有 `/index.html`、`/next.html`。QMP PNG 1024x768 PPM-P6、15046 bytes；
  decoded `changed=true`，partial geometry `423x16+99,384`，title/taskbar rows
  `962/418/418/0/121`，stable crop `9014e4dd`。
- SSH PASS：port 2223，uptime=34143ms，Mica eval/repl/file/denied status=0，
  disconnect=124、recovery=0、wrong-key/no-key=255。
- Mica/TLS PASS：bootfs=24/23、first-pid=13、output=42/status=0；trusted=true，
  unknown-ca/expired/not-yet-valid/hostname-mismatch 均 rejected，combo 全 true。

最终 fsck tuple：`MFS1 clean generation=1959 transactions=1838 entries=24
used_blocks=13420`。

只读残留核对：无 QEMU、测试脚本/helper 进程；无当前
`microsystem-dev:rust-1.97.1` OrbStack 容器；`target/gui-qmp.sock`、
`target/gui-serial.in` 及当前 fixture/helper 临时目录均不存在。保留预期
`target/gui-browser-fixture.log`、`target/mica-dns-fixture.log`、
`target/mica-manual/helper.mica`、`target/ssh-mica-fixture`。仅发现用户此前遗留的
旧 FIFO `target/fault-injection-1786510519-706-6347/transaction-data-flushed.in`
（Aug 12，非本次 run，未删除）。统一日志、QMP、截图与 fsck 日志均保留；fault
阶段的 `Killed` 行均为预期 first-status=137 注入，不是失败。

### Chained MFS views / event-blocked GUI acceptance (2026-08-14)

本轮真实回归边界是 NodeView/view-before 在连续目录 rename、旧名 recreate、
目标 subtree remove 和再次 rename 后仍保持 lookup/list/read/metadata 一致，并在
sync/remount 后保持相同语义；同时 GUI Files/Monitor 必须以 `wait=event-blocked`
进入阻塞事件循环，而不是 busy-loop。新增 `chained_directory_rename_recreate_remove_views_stay_consistent`
覆盖了这些行为（含嵌套文件 metadata 字节数、目录 list、remove 后 NotFound、
最终 remount/check）。

静态检查：`rustfmt`、`bash -n scripts/gui-qemu.sh` 通过；3 个内嵌 Python heredoc
均通过 AST 解析；`git diff --check` 通过。OrbStack targeted：MFS recovery
**16 passed / 0 failed**；`cargo check -p microsystem-fs -p microsystem-files
-p microsystem-monitor -p microsystem-windowd --features user-bin` exit 0。Fresh
`make build > target/chained-view-build.log 2>&1` exit 0。

唯一 GUI gate 使用默认 `target/gui-qmp.sock`（保留自定义
`target/chained-view-gui.*` 证据）exit 0/PASS。关键串口 marker：

```text
[net] virtio-net ready ... rx-buffers=2
[gui] unifont runtime loaded=true cache=128 fallback=ascii
[gui] files service wait=event-blocked
[gui] monitor service wait=event-blocked
[gui] EL0 windowd partial-present=true rect=423x16+99,384
```

既有完整门禁仍通过：windowd 1024x768/clients=3、desktop move/resize/minimize/
maximize/alt-tab、launcher icons=5；Terminal FS mkdir/write/stat/mv/cat/ls/rm/rm/
sync 全为 `status=ok`；Reader 的 Present 先于两次 HTTP 200 load，fixture 仅
`GET /index.html`、`GET /next.html`；Editor loaded/saved/reloaded 且
`bytes=44 atomic=true fsync=true`；Editor icon launches/presents/reclaims=2；Mica
SSH status=0。

Final QMP decoded CRC/geometry：

```text
desktop_pre=b1998cf3 mica_pre=7d6a4f44 mica_button=e8128290 mica_text=60f7c6f6
terminal_help=6170bbe9 ascii=9f1f4603 resized=957bdb75 max=524aacc7 restore=957bdb75
minimized=083b1435 taskbar_restore=c9110363 first_before_second=c9110363
second_pre=cca2e7d6 second_button=40858a7f second_text=a445076b
after_second_close=35366f14 after_reuse_close=35366f14
browser_pre=c7d5ed78 browser_link=5dab179e browser_cross=f60453d9
editor_pre=3eeb31ac editor_apply=da105c73 editor_saved=18f38838 editor_reload=7e895e1d
changed=true partial_present=423x16+99,384
title/taskbar rows=(962,418,418,0,121) stable crop=9014e4dd help rows=140
```

QMP PNG was 1024x768 PPM-P6 (15046 bytes). Cleanup passed: no QEMU/container,
`target/gui-qmp.sock`, serial FIFO, or current GUI fixture directories; expected
`target/gui-browser-fixture.log` retained exactly the two Reader GETs. No panic/DMA
fault markers appeared. Unified `make test`/`make fsck` were not run for this section.

### Materialized-paths retest (2026-08-14)

生产随后完成 `materialized_paths` 优化；按授权先终止了旧树上尚未进入完整集成
阶段的 unified run（日志停在 smoke→powercut 边界，人工 SIGTERM，未执行 fsck，
不计为测试失败），清理后无相关 make/xtask/QEMU 进程及当前 OrbStack 容器。

新树 targeted 结果：OrbStack MFS recovery **16 passed / 0 failed**；Files/Monitor/
FS/Windowd `cargo check --features user-bin` exit 0；fresh
`make build > target/materialized-paths-build.log 2>&1` exit 0。

随后唯一 GUI gate 使用默认 `target/gui-qmp.sock`，证据保存在
`target/materialized-paths-gui.*`，exit 0/PASS。串口命中：

```text
[net] virtio-net ready ... rx-buffers=2
[gui] unifont runtime loaded=true cache=128 fallback=ascii
[gui] files service wait=event-blocked
[gui] monitor service wait=event-blocked
[gui] EL0 windowd partial-present=true rect=423x16+99,384
```

既有 GUI 门禁完整通过：windowd/desktop/icons、Terminal FS
`mkdir,write,stat,mv,cat,ls,rm,rm,sync` 全 `status=ok`；Reader Present 先于两次
HTTP 200 load（fixture 仅 `/index.html` 与 `/next.html`）；Editor
`bytes=44 atomic=true fsync=true` 保存并 reload；Editor icon launches/presents/
reclaims=2；Mica SSH status=0。

QMP PNG 为 1024x768 PPM-P6、15046 bytes；decoded CRC/geometry：

```text
desktop_pre=b1998cf3 mica_pre=7d6a4f44 mica_button=e8128290 mica_text=60f7c6f6
terminal_help=ef41e08f ascii=9f1f4603 resized=957bdb75 max=524aacc7 restore=957bdb75
minimized=083b1435 taskbar_restore=c9110363 first_before_second=c9110363
second_pre=cca2e7d6 second_button=40858a7f second_text=3db12925
after_second_close=35366f14 after_reuse_close=35366f14
browser_pre=c7d5ed78 browser_link=5dab179e browser_cross=f60453d9
editor_pre=3eeb31ac editor_apply=da105c73 editor_saved=18f38838 editor_reload=7e895e1d
changed=true partial_present=423x16+99,384
title/taskbar rows=(962,418,418,0,121) stable crop=9014e4dd help rows=140
```

Cleanup passed: no QEMU, current microsystem-dev container, QMP socket, serial FIFO or
temporary GUI fixture directories; expected browser fixture log retained exactly two
GETs. No panic/DMA fault markers appeared. Unified acceptance remains pending explicit
authorization after the materialized-paths update.

### Unified materialized-paths acceptance (2026-08-14)

按重新授权从宿主 OrbStack 冻结 materialized-paths 工作树执行唯一
`make test > target/unified-performance-review-final2.log 2>&1`，exit **0**；随后
唯一 `make fsck > target/unified-performance-review-fsck.log 2>&1`，exit **0**。

Host suites 共 **74 passed / 0 failed**：MFS1 recovery **16/16**、ABI **9/9**、
GUI command_stream **6/6** + desktop **9/9**、kernel **11/11**、Mica **23/23**；
unit/doc-test harnesses 均 0 tests。

集成 gates 全部 PASS：smoke serial milestones/file-chain；powercut first=0、
second=0；fault 五案例（records old、data old、superblock written/flushed new、
GC candidate gc-proof；first=137 expected kill、second=0，GC fill=4194304 bytes,
1 iteration, generation=2040 used=13826 free=2302 capacity=16128 total=16384）；
GUI、SSH、Mica/TLS 全 PASS。

统一 GUI serial 命中 `rx-buffers=2`、`cache=128`、Files/Monitor
`wait=event-blocked`、partial-present `423x16+99,384`；Terminal FS
`mkdir/write/stat/mv/cat/ls/rm/rm/sync` 全 `status=ok`。Reader Present 先于两次
HTTP 200 load，fixture 仅 `/index.html`、`/next.html`；Editor
`bytes=44 atomic=true fsync=true` 保存/reload，Editor icon launches/presents/
reclaims=2。SSH uptime=27803ms，Mica eval/repl/file/denied=0，disconnect=124、
recovery=0、wrong/no-key=255；Mica/TLS bootfs=24/23、first-pid=13、output=42，
trusted=true 且四类 TLS 错误均 rejected。

QMP PNG 1024x768 PPM-P6、15046 bytes；decoded `changed=true`，partial
`423x16+99,384`，title/taskbar rows `962/418/418/0/121`，stable crop
`9014e4dd`，help rows=140。代表性 CRC：desktop_pre=b1998cf3、mica_pre=43d776a4、
terminal_help=a5b931b3、browser_pre=2286d988/browser_link=5dab179e/browser_cross=
f60453d9、editor_pre=3eeb31ac/editor_apply=da105c73/editor_saved=18f38838/
editor_reload=7e895e1d、after_reuse_close=35366f14。

最终 fsck tuple：`MFS1 clean generation=2052 transactions=1925 entries=24
used_blocks=13420`。只读残留核对无 QEMU、测试进程/helper、当前
microsystem-dev 容器、QMP socket、serial FIFO 或临时 GUI fixtures；预期日志/资产
保留，另有用户此前遗留旧 FIFO `target/fault-injection-1786510519-706-6347/
transaction-data-flushed.in`（未删除）。

### Windowd damage/culling marker gate (2026-08-14)

本轮真实回归边界是 windowd 的 damage merge、局部窗口重绘、display-list command
cull、2-way glyph cache 与 16-event input batch 不能破坏既有拖拽/缩放、partial
Present、Terminal、Reader/Editor 和桌面图标生命周期门禁。现有 GUI gate 已覆盖
这些可观察行为；仅在 `scripts/gui-qemu.sh` 的启动 required markers 增加了生产
marker：
`[gui] renderer damage-merge=true local-window-damage=true command-cull=true
glyph-cache=2way input-batch=16`，未添加新的基础设施或放宽 CRC/生命周期断言。

静态检查 `bash -n scripts/gui-qemu.sh` 通过；OrbStack
`cargo check -p microsystem-windowd --features user-bin` exit 0；fresh `make build`
exit 0，windowd release ELF、kernel 与 MFS1 image 均生成。

按授权仅执行一次有效 GUI QEMU gate，使用 xtask 固定的默认
`target/gui-qmp.sock`，证据保留在 `target/gui-perf-final2.log`、
`target/gui-perf-final2.qmp.jsonl` 与 `target/gui-perf-final2-pre.png`。启动 marker
均到达，包括 `rx-buffers=2`、`cache=128`、windowd/desktop/icon、Files/Monitor
`wait=event-blocked` 以及新增 renderer marker；无 panic/DMA fault。首个真实门禁
错误发生在 Terminal click 后的 partial Present 几何断言：
`RuntimeError: GUI partial Present rectangle was not a bounded non-full scanout:
1024x720+0,48`。QMP 已完成 greeting/capabilities/status/screendump/Alt-Tab/click，
随后按首错停止，未执行 Reader/Editor/Terminal 后续断言，也未重跑。该矩形面积
仍非全屏，但当前 harness 要求宽高同时小于 1024x768，因此本次不能记为 PASS，
也未自行修改阈值。

清理核对：QEMU、xtask/helper 进程、OrbStack microsystem 容器、默认 QMP socket、
serial FIFO 与临时 fixture 目录均已清除；保留上述失败日志/截图及预期的
`target/gui-browser-fixture.log`。无生产代码编辑。

### Windowd damage/culling GUI gate final PASS (2026-08-14)

针对上节已确认的 harness 假阳性，`scripts/gui-qemu.sh` 两处 partial-present
校验现要求矩形 `x/y >= 0`、`width/height > 0`、右下角不越过 1024x768，且
`width*height < 1024*768`；允许 1024x720 或 1000x768，不放宽全屏面积断言。
`bash -n`、两个内嵌 Python heredoc AST 检查与 `git diff --check` 均通过；复用同一
fresh build，仅用 xtask 固定默认 `target/gui-qmp.sock` 再执行一次 GUI gate，exit
**0/PASS**。

证据保留在 `target/gui-perf-final3.log`、`target/gui-perf-final3.qmp.jsonl` 及完整
`target/gui-perf-final3-*.png`。串口命中新 renderer marker、`rx-buffers=2`、
`cache=128`、Files/Monitor `wait=event-blocked`、windowd/desktop/icon、以及
`partial-present=true rect=1024x720+0,48`。Terminal FS
`mkdir/write/stat/mv/cat/ls/rm/rm/sync` 全为 `status=ok`；Mica 首次 Present
先于 Reader 两次 HTTP 200，same-origin/cross-origin、Editor save/reload 与
desktop Editor icon launches/presents/reclaims=2 均通过；无 panic/DMA fault。

最终 QMP：PNG 1024x768 PPM-P6 15046 bytes，partial geometry
`1024x720+0,48`，title rows `962/418/418/0`、taskbar row `121`，stable crop
`9014e4dd`，terminal help rows `140`。CRC：desktop_pre `b1998cf3`、Mica
pre/button/text `7d6a4f44/e8128290/60f7c6f6`、terminal help/ascii
`1e646e75/9f1f4603`、resize/max/restore/minimized
`f19b1e8d/524aacc7/957bdb75/083b1435`、taskbar/second-pre/button/text
`c9110363/cca2e7d6/40858a7f/3db12925`、after-close/reuse
`35366f14/35366f14`、Reader pre/link/cross
`c7d5ed78/5dab179e/f60453d9`、Editor pre/apply/saved/reload
`3eeb31ac/da105c73/18f38838/7e895e1d`；`changed=true`。

清理核对：QEMU、xtask/helper、当前 OrbStack 容器、默认 QMP socket、custom serial
FIFO 及所有本次 GUI fixture 临时目录均不存在；预期
`target/gui-browser-fixture.log` 保留。此前 `1024x720` 首错属于旧 checker
要求两个维度同时小于全屏的假阳性，已由面积严格小于全屏的语义修正并由本 PASS
验证。

### Unified GUI-performance acceptance (2026-08-14)

按授权从宿主 OrbStack 冻结树执行唯一
`make test > target/unified-gui-performance-final.log 2>&1`，exit **0**；随后紧接
唯一 `make fsck > target/unified-gui-performance-fsck.log 2>&1`，exit **0**。

Host suites 共 **74 passed / 0 failed**：MFS1 recovery **16/16**、ABI **9/9**、
GUI command_stream **6/6** + desktop **9/9**、kernel core_boundaries **11/11**、
Mica core **23/23**；unit/doc-test harnesses 均为 0 tests。集成阶段全部 PASS：

- smoke serial milestones、scheduler/IPC/EL0 readiness、file-chain
  `mkdir/write/sync/ls/cat=ok`。
- powercut `first-status=0 second-status=0`。
- fault 五案例：records-written=old、data-flushed=old、superblock-written=new、
  superblock-flushed=new、GC candidate=gc-proof；每案 `first-status=137`
  （预期注入 kill）、`second-status=0`。GC validated fill=4194304 bytes、
  iterations=1、stats `generation=2101 used_blocks=14594 free_blocks=1534
  capacity_blocks=16128 total_blocks=16384`。
- GUI PASS；串口命中 `rx-buffers=2`、`cache=128`、Files/Monitor
  `wait=event-blocked`、windowd renderer marker
  `damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
  input-batch=16`，以及合法 partial Present `rect=1024x720+0,48`。
  该矩形在 1024x768 边界内，面积 737280 严格小于全屏 786432。Terminal FS
  `mkdir/write/stat/mv/cat/ls/rm/rm/sync` 全为 `status=ok`；Reader 首次 Present
  先于两次 HTTP 200 load（lines=4、3），same-origin/cross-origin 断言通过；
  Editor save/reload `bytes=44 atomic=true fsync=true`；桌面 Editor icon
  launches/presents/reclaims=2。QMP PNG 为 1024x768 PPM-P6、15046 bytes、
  `changed=true`；geometry rows `962/418/418/0/121`，stable crop `9014e4dd`，
  terminal help rows=140。代表性 decoded CRC：desktop `b1998cf3`、Mica
  `7d6a4f44/e8128290/60f7c6f6`、browser `c7d5ed78/5dab179e/f60453d9`、
  Editor `3eeb31ac/da105c73/18f38838/7e895e1d`、after-close/reuse
  `35366f14/35366f14`。
- SSH PASS：port 2223、GUI port 5901、uptime 32527 ms，Mica eval/repl/file/
  denied statuses=0，disconnect=124、recovery=0、wrong-key/no-key=255。
- Mica/TLS PASS：bootfs=24/23、first-pid=13、Mica output=42/status=0；
  `trusted=true`，unknown-ca/expired/not-yet-valid/hostname-mismatch 均 rejected，
  combo 全 true。

最终 fsck tuple：`MFS1 clean generation=2119 transactions=1986 entries=24
used_blocks=11887`。

只读残留核对：无 QEMU、test/xtask/helper 进程；无
`microsystem-dev:rust-1.97.1` OrbStack 容器；`target/gui-qmp.sock`、
`target/gui-serial.in`、SSH/Mica sockets/FIFOs 与本次临时 fixture 目录均不存在。
保留预期 `target/gui-browser-fixture.log`、`target/mica-dns-fixture.log`、
`target/mica-manual/helper.mica`、`target/ssh-mica-fixture` 及统一日志/QMP/截图。
另有用户此前遗留的 `target/fault-injection-1786510519-706-6347/
transaction-data-flushed.in`（Aug 12，非本次 run，未删除）。日志中的 fault
`Killed` 行均为预期 first-status=137 注入，不是失败；无 panic/DMA fault。

### Browser `net.browse` gate preparation (2026-08-14)

针对 Reader 互联网能力的测试契约已完成静态更新（以下记录为动态门禁前状态）。
`crates/mica/tests/core.rs` 现在验证 `net.browse`
只能解析为 unscoped Browse，manifest/launcher 交集不会产生 raw host Connect，且
browser asset 仍编译并保留 query/fragment parsing、same-origin navigation、以及
Loading 前 `app:present()` 标记。`scripts/gui-qemu.sh` 的 fixture rewrite 与 Reader
launcher 改为保留并授予 `net.browse`，不再伪造 exact HTTP Connect。

`scripts/mica-qemu.sh` 的既有 combo fixture 改为 Browse-only HTTP GET：GET 带 query
并验证 fragment 不发送；raw `net.resolve`、TCP、UDP 与 `http.post` 必须拒绝，同时
exact DNS/TCP/UDP/TLS paths 仍保留。fixture 日志断言 GET path 为
`/index.txt?browse=1` 且没有 POST 请求。bash/Python heredoc 与 `git diff --check`
静态检查通过；动态 gate 结果见下一节。

### Browser `net.browse` dynamic gate attempt (2026-08-14)

按授权在 OrbStack 中执行 targeted Mica core，`crates/mica/tests/core.rs` **24/24
passed**。随后 fresh `make build` 完成全部 cargo release/asset/mfsctl 阶段，证据保留在
`target/browser-browse-build.log`；外层 zsh 记录命令因 wrapper 使用只读变量名
`status` 在 make 完成后的后处理阶段退出，构建产物与日志本身无错误，未重跑 build。

唯一一次 `scripts/mica-qemu.sh`（日志 `target/browser-browse-mica-qemu.log`）按首个
真实错误停止：Browse-only `http.get` 到动态 fixture 首次连接被拒，串口为
`mica network error stage=http-get message=system service denied access kind=access
operation=net.tcp_connect code=-3`，随后 Mica status=0 仅反映脚本错误返回路径。该失败发生
在现有 `services/netd/src/main.rs` 仍仅解析 `net.connect:` 的 broker 授权边界，不能归因
于 fixture 断言。raw resolve/TCP/UDP/POST、query/fragment fixture 后续断言及 GUI gate
均未执行；严格首错停止。

Mica gate 的临时 fixture、QEMU 与容器已清理；保留专用日志和预期
`target/mica-dns-fixture.log`。`target/gui-qmp.sock` 当前为无进程持有的既有 stale socket，
未删除；未产生本次 GUI fixture/serial FIFO 残留，也未运行 `scripts/gui-qemu.sh`。

### Browser `net.browse` final validation (2026-08-14)

生产 netd 增加 Browse magic 的双层授权后，按授权重新执行 targeted/build/QEMU：

- OrbStack `cargo test -p microsystem-mica --test core`：**24/24 passed**。
- fresh `make build`：**exit 0**，更新后的 ABI/netd/Mica/kernel 与 MFS image 均完成；日志
  `target/browser-browse-build-final.log`。
- 唯一 `scripts/mica-qemu.sh`（`MICROSYSTEM_MICA_TLS_FIXTURE=1`）：**PASS**，日志
  `target/browser-browse-mica-qemu-final.log`。Combo marker 同时报告
  `http=true browse=true browse-resolve-denied=true browse-raw-denied=true
  browse-udp-denied=true browse-post-denied=true https=true`，且
  `tls-unknown-ca/expired/not-yet-valid/hostname-mismatch=true`；Mica eval/REPL
  输出 42、三次 status=0。Fixture 日志 `target/mica-dns-fixture.log` 记录 Browse GET
  `/index.txt?browse=1`（fragment 未发送），TLS trusted 请求及四类 TLS 拒绝；无 POST。
- 唯一 `scripts/gui-qemu.sh`（默认 `target/gui-qmp.sock`，自定义日志
  `target/browser-browse-gui-final.log`/`.qmp.jsonl`）：**PASS**。串口命中
  `rx-buffers=2`、Unifont `cache=128`、renderer
  `damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
  input-batch=16`、Files/Monitor `wait=event-blocked`、partial-present
  `rect=1024x720+0,48`。该矩形在 1024x768 内且面积 737280 < 786432。
  Terminal `mkdir/write/stat/mv/cat/ls/rm/rm/sync` 均 `status=ok`；Reader 首次
  `gui presented` 在两次 HTTP 200 loads（lines=4、3）之前，fixture 仅 GET
  `/index.html` 和 `/next.html`，same-origin/cross-origin 断言通过；Editor
  save/reload `bytes=44 atomic=true fsync=true`，桌面 Editor icon launches/
  presents/reclaims=2。

GUI QMP 最终 PNG 1024x768 PPM-P6 15046 bytes、`changed=true`；geometry rows
`962/418/418/0/121`，stable crop `9014e4dd`，terminal help foreground rows=140。
Decoded CRC 代表值：desktop `b1998cf3`、Mica `43d776a4/e8128290/60f7c6f6`、
terminal help `20187d0c`、Reader `c7d5ed78/5dab179e/f60453d9`、Editor
`3eeb31ac/da105c73/18f38838/7e895e1d`、after-close/reuse
`35366f14/35366f14`。

清理核对：无 QEMU、Mica/GUI helper、OrbStack 测试容器；默认 QMP socket、serial FIFO、
本次 GUI/Mica 临时 fixture 目录均不存在。保留预期
`target/gui-browser-fixture.log`、`target/mica-dns-fixture.log` 及上述日志/截图；无
panic/DMA fault。此前 netd AccessDenied 首错已由本次 PASS 覆盖，未重跑同一 gate。

### Unified browser-internet acceptance (2026-08-14)

按授权从宿主 OrbStack 冻结树执行唯一 `make test >
target/unified-browser-internet-final.log 2>&1`，exit **0**；随后仅执行唯一
`make fsck > target/unified-browser-internet-fsck.log 2>&1`，exit **0**。

Host suites 共 **75 passed / 0 failed**：MFS1 recovery **16/16**、ABI **9/9**、GUI
command_stream **6/6** + desktop **9/9**、kernel core_boundaries **11/11**、Mica
core **24/24**；unit/doc-test harnesses 均为 0 tests。集成阶段全部 PASS：

- smoke serial milestones 与 file-chain `mkdir/write/sync/ls/cat=ok`；powercut
  `first-status=0 second-status=0`。
- fault 五案例全部 PASS：records-written=old、data-flushed=old、
  superblock-written=new、superblock-flushed=new、GC candidate=gc-proof；各案
  `first-status=137`（预期注入 kill）、`second-status=0`，GC
  `fill=4194304`、iterations=1、stats `generation=2177 used_blocks=13826
  free_blocks=2302 capacity=16128 total=16384`。
- GUI PASS；串口命中 `rx-buffers=2`、Unifont `cache=128`、renderer
  `damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
  input-batch=16`、Files/Monitor `wait=event-blocked`、合法 partial Present
  `rect=1024x720+0,48`（面积 737280 < 786432）。Terminal
  `mkdir/write/stat/mv/cat/ls/rm/rm/sync` 全部 `status=ok`。
  Reader 首次 `gui presented` 先于两次 HTTP 200 loads（lines=4、3），fixture
  仅 GET `/index.html`、`/next.html`，same-origin/cross-origin 断言通过；Editor
  save/reload `bytes=44 atomic=true fsync=true`，desktop Editor icon
  launches/presents/reclaims=2。QMP PNG 1024x768 PPM-P6 15046 bytes，
  `changed=true`，geometry rows `962/418/418/0/121`，stable crop `9014e4dd`，
  terminal help rows=140；代表性 CRC desktop `b1998cf3`、Reader
  `2286d988/5dab179e/f60453d9`、Editor `3eeb31ac/da105c73/18f38838/7e895e1d`、
  after-close/reuse `35366f14/35366f14`。
- SSH PASS：port 2223、GUI port 5901、uptime 30739 ms，Mica eval/repl/file/
  denied statuses=0，disconnect=124、recovery=0、wrong-key/no-key=255。
- Mica/TLS PASS：bootfs=24/23、first-pid=13、Mica output=42/status=0。Browse
  combo marker 为 `http=true browse=true browse-resolve-denied=true
  browse-raw-denied=true browse-udp-denied=true browse-post-denied=true`；fixture
  记录 GET `/index.txt?browse=1`（fragment 未发送）且无 POST。TLS trusted 请求成功，
  unknown-ca/expired/not-yet-valid/hostname-mismatch 全部 rejected，combo TLS flags
  全 true。

最终 fsck tuple：`MFS1 clean generation=2189 transactions=2053 entries=24
used_blocks=13420`。

只读残留核对：无 QEMU、make/xtask/helper 进程；无 `microsystem-dev:rust-1.97.1`
容器；本次 QMP socket、serial FIFO、SSH/Mica sockets 与 GUI/Mica 临时 fixture 目录
均不存在。保留预期 `target/gui-browser-fixture.log`、`target/mica-dns-fixture.log`、
统一日志及截图；另有用户此前遗留的
`target/fault-injection-1786510519-706-6347/transaction-data-flushed.in`（Aug 12，
非本次 run，未删除）。统一日志中的 `Killed` 行均为预期 fault first-status=137，
无 panic/DMA fault。

### Browser CA bundle host-root validation (2026-08-14)

静态/构建证据：`rustup run 1.97.1 rustfmt --check xtask/src/main.rs` 与
OrbStack `cargo check -p xtask` 均通过；全仓 `cargo fmt` 未作为门禁（容器缺
`cargo-fmt` 且仓库存在既有格式漂移）。唯一 fresh `make build`
（`target/browser-internet-ca-build.log`）exit **0**，日志 marker 为
`CA bundle: pinned=121 host-extra=43 bytes=173925`（总大小小于 256 KiB）。
`build/ca-bundle.derpack` 解析为 MCAB v1、164 条证书、总 173925 bytes；其中
包含 Cloudflare Gateway 根 SHA-256
`FE:1B:1D:68:24:EE:B4:9D:E0:A5:9B:3F:19:BD:B7:3B:CF:FE:2A:A1:6D:8C:FD:86:5C:4F:B7:8E:E8:85:00:C1`
（匹配 2 条证书记录，含追加宿主根）。

按授权只启动一次 `GUI_PORT=5903 SSH_PORT=2224 make gui`，点击 Reader 图标
中心 `(896,682)`，QMP 截图 `target/browser-internet-reader.ppm`（1024x768，
并转存 `target/browser-internet-reader.png`）保留。首个真实运行错误为 Reader
页面显示 `CA bundle integrity check failed`，随后 `Request failed`，没有
`mica-reader loaded status=200`/HTTP 成功 marker；因此按首错停止，未执行
`make test` 或 `make fsck`，也未关闭 TLS 校验。串口已确认 windowd/Reader launch
与 `gui presented` 成功，未见 panic/DMA fault。

GUI 已 Ctrl-C 干净停止（QEMU 正常 `terminating on signal 2`）；5903/2224 无
监听，`target/gui-qmp.sock`、serial FIFO、测试容器与 qemu/xtask 进程均无残留。
本轮 CA 证据保留于上述 build log、bundle 和截图，待修正运行时 bundle hash
契约后再重试公网 Reader 与统一门禁。

### Browser CA bundle runtime-hash retry (2026-08-14)

生产修正后重新执行聚焦验证：`rustup run 1.97.1 rustfmt --check
xtask/src/main.rs`、OrbStack `cargo check -p xtask` 均 exit **0**；唯一 fresh
`make build` 的新日志为 `target/browser-internet-ca-build-final2.log`，其
Mica service 编译明确注入
`MICROSYSTEM_CA_BUNDLE_SHA256=70572323c1e9e151d300b76888bfb43906afb6473c573208a565e943b5426d50`，
并再次报告 `pinned=121 host-extra=43 bytes=173925`。bundle 仍为 MCAB v1、
164 entries、173925 bytes，Cloudflare Gateway root fingerprint FE1B…00C1
存在。

唯一新 GUI profile 为 `GUI_PORT=5903 SSH_PORT=2224 make gui`；Reader 图标
点击 `(896,682)` 后，QMP 截图
`target/browser-internet-reader-final2-loading.ppm`（1024x768，PNG 转存同名
`.png`）显示 bundle integrity 已通过，但真实 `https://example.com/` 的首个
错误为 `TLS handshake validation failed: InvalidCertificate`，随后
`Request failed`，仍没有 HTTP 200/`mica-reader loaded` marker。串口先出现
网络 ARP first TX/RX、Reader launch、`gui presented`，未见 panic/DMA fault；按
首错停止，未运行 `make test`/`make fsck`，未关闭 TLS 校验。

GUI 以 Ctrl-C 干净停止（QEMU `terminating on signal 2`）；5903/2224、QMP
socket、serial FIFO、QEMU/xtask 进程和 OrbStack 测试容器均无残留。当前阻塞已
从 bundle-integrity mismatch 收敛为公网证书链验证失败，需在 TLS 根/链证据层
修正后再重试统一门禁。

### Browser CA bundle image-bound/public Reader validation (2026-08-14)

按最新 image-window 修复后的冻结树完成聚焦验证。`rustup run 1.97.1 rustfmt
--check xtask/src/main.rs` 与 `rustup run 1.97.1 rustfmt --check
services/mica/src/tls.rs` 均 exit **0**；OrbStack
`cargo test -p microsystem-kernel --test core_boundaries` 为 **11/11 passed**。
唯一 fresh OrbStack `make build`（`target/browser-internet-ca-build-imagebound.log`）
exit **0**，日志包含 `CA bundle: pinned=121 host-extra=43 bytes=173925`，并向
Mica service 注入完整 bundle hash
`70572323c1e9e151d300b76888bfb43906afb6473c573208a565e943b5426d50`。
Mica service ELF 的最大 PT_LOAD `p_memsz=0xd4e28`，经解析确认严格小于新的
`0xe0000` task image 窗口。

随后仅启动一次 `GUI_PORT=5903 SSH_PORT=2224 make gui`，QMP 点击 Reader 图标
`(896,682)`。串口生命周期命中 `desktop launch application=4 accepted=true`、
`mica client registered`、`[mica] gui presented`，并完成网络 ARP first TX/RX；
未出现 `InvalidCertificate`、`Request failed`、panic 或 DMA fault。最终 QMP
截图 `target/browser-internet-reader-imagebound-final.ppm`（PPM-P6，1024x768，
2359312 bytes；PNG 同名 `.png`）显示 Reader 页面 `HTTP 200 — https://example.com/`、
`# Example Domain` 与正文，证明真实公网 TLS/HTTP 已成功。

GUI 以 Ctrl-C 正常停止（`qemu-system-aarch64: terminating on signal 2`）。只读
清理核对：5903/2224 无监听；无 qemu、make/xtask、helper 进程；无
`microsystem-dev:rust-1.97.1` 容器；`target/gui-qmp.sock` 与 `target/gui-serial.in`
不存在。保留 CA build log、ELF/边界测试 log 与上述截图；本轮未运行统一
`make test`/`make fsck`。

### Mica TLS trusted-root long-SAN correction (2026-08-14)

The prior Cloudflare-anchor fixture was superseded: it did not exercise the
presented trusted root's name path. The fixture now adds the valid 68-byte DNS
SAN `root-long-san-for-microsystem-tls-parser-0123456789abcd.example.test`
directly to `test-ca.pem`, and the trusted server still presents
`trusted.pem` plus that exact `test-ca.pem` root as a two-certificate chain.
The fixture no longer extracts or fingerprints an environment-specific root;
the marker is `tls-ca-name-long=true`.

After `bash -n scripts/mica-qemu.sh`, fresh OrbStack `make build` exited **0**
(`target/mica-tls-long-san-build.log`, CA `pinned=121 host-extra=43
bytes=173925`). The single authorized
`MICROSYSTEM_MICA_TLS_FIXTURE=1 scripts/mica-qemu.sh` exited **0**
(`target/mica-tls-long-san-final.log`): trusted HTTPS returned
`mica-https-ok` with `tls-trusted-root-chain=true tls-p384=true
tls-ca-name-long=true`; unknown-CA, expired, not-yet-valid, and
hostname-mismatch all remained rejected; combo and eval/REPL status markers
were successful. The fixture log recorded
`tls server port=18443 certificate=trusted-chain` and the trusted request.

Cleanup removed all Mica fixture/artifact backups and QEMU/container state;
the standard GUI gate was not rerun in this correction and no unified gates
were run.

### Unified browser/public-internet acceptance (2026-08-14)

On the frozen tree, the single OrbStack host command
`make test > target/unified-browser-public-internet-final.log 2>&1` exited **0**;
only then the single `make fsck > target/unified-browser-public-internet-fsck.log
2>&1` exited **0**.

Host suites totaled **75 passed / 0 failed**: MFS1 recovery **16/16**, ABI
**9/9**, GUI command-stream **6/6** plus desktop **9/9**, kernel boundaries
**11/11**, and Mica core **24/24** (unit/doc-test harnesses had zero tests).
The build marker was `CA bundle: pinned=121 host-extra=43 bytes=173925`; the
Mica build received full bundle SHA-256
`70572323c1e9e151d300b76888bfb43906afb6473c573208a565e943b5426d50`.

Integration stages all passed: smoke serial milestones and file-chain
`mkdir/write/sync/ls/cat=ok`; powercut `first-status=0 second-status=0`; and all
five fault cases (`old`, `old`, `new`, `new`, `gc-proof`) with expected
`first-status=137`, `second-status=0`. GC reported fill 4194304 bytes,
iterations=2, and validated capacity 16128/total 16384.

GUI passed with `rx-buffers=2`, `cache=128`, damage/cull renderer markers,
partial Present `rect=1024x720+0,48`, Terminal filesystem commands all `ok`,
Reader HTTP 200 loads (4 and 3 lines), Editor save/reload
`bytes=44 atomic=true fsync=true`, and Editor icon launches/presents/reclaims
2/2/2. The QMP decoded record remained `changed=true`, 1024x768, geometry rows
`962/418/418/0/121`, stable crop `9014e4dd`; representative Reader CRCs were
`2286d988/5dab179e/f60453d9` and Editor CRCs
`3eeb31ac/da105c73/18f38838/7e895e1d`. Browser fixture recorded only
`GET /index.html` and `GET /next.html`.

SSH passed on port 2223 (uptime 31370 ms; Mica eval/repl/file/denied status 0,
disconnect 124, wrong/no-key 255). Mica/TLS passed with trusted body,
`tls-trusted-root-chain=true tls-p384=true tls-ca-name-long=true`, all four
certificate negatives rejected, and combo
`browse=true browse-resolve-denied=true browse-raw-denied=true browse-udp-denied=true
browse-post-denied=true`; fixture logged `certificate=trusted-chain` and the
browse query/fragment GET with no POST.

Final fsck tuple: `MFS1 clean generation=2415 transactions=2261 entries=24
used_blocks=12663`.

Read-only cleanup found no QEMU, make/xtask, helper process, listening test
port, or `microsystem-dev:rust-1.97.1` container; no current QMP/serial socket
or fixture directory remained. Expected persisted logs/assets remain
(`gui-browser-fixture.log`, `mica-dns-fixture.log`, `ssh-mica-fixture`, and
`mica-manual/helper.mica`). One FIFO
`target/fault-injection-1786510519-706-6347/transaction-data-flushed.in` is a
pre-existing Aug-12 residue, not from this run; it was not removed. Unified
logs are preserved; no further gates were run.

### Mica TLS long-CN/root-chain/P-384 regression plus GUI (superseded 2026-08-14)

`bash -n scripts/mica-qemu.sh` and the embedded fixture-contract checks passed.
This historical run used a production Cloudflare trust anchor and is retained
only as prior evidence; it did not exercise the presented root's name path.

Fresh OrbStack `make build` exited **0** (`target/mica-tls-p384-chain-build.log`):
`CA bundle: pinned=121 host-extra=43 bytes=173925`; injected Mica bundle hash is
`70572323c1e9e151d300b76888bfb43906afb6473c573208a565e943b5426d50`.

The single authorized TLS fixture gate
(`MICROSYSTEM_MICA_TLS_FIXTURE=1 scripts/mica-qemu.sh`) exited **0**;
`target/mica-tls-p384-chain-final.log` contains:

- trusted HTTPS body `mica-https-ok`, root-chain and P-384 checks;
- `unknown-ca=rejected`, `expired=rejected`, `not-yet-valid=rejected`, and
  `hostname-mismatch=rejected`;
- combo HTTPS/TLS flags all true and normal eval/REPL status 0.

The fixture log proves the server presented `certificate=trusted-chain` and
recorded the trusted request; cleanup restored the image/artifacts and removed
the temporary fixture, QEMU, and container state.

Because the TLS gate passed, the single standard `scripts/gui-qemu.sh` gate also
exited **0** (`target/mica-tls-p384-chain-gui.log` and
`target/mica-tls-p384-chain-gui.qmp.jsonl`). Serial evidence includes
`rx-buffers=2`, `cache=128`, renderer damage/cull markers, partial Present
`rect=1024x720+0,48`, Terminal filesystem commands all `status=ok`, Reader two
HTTP 200 loads, Editor save/reload `bytes=44 atomic=true fsync=true`, and two
Editor icon launches/presents/reclaims. Reader fixture recorded only
`GET /index.html` and `GET /next.html`.

QMP final PNG is 1024x768 PPM-P6 (15046 bytes), `changed=true`; geometry rows are
`962/418/418/0/121`, partial rect area is 737280 < 786432, stable crop CRC is
`9014e4dd`, and representative Reader/Editor CRCs are
`2286d988/5dab179e/f60453d9` and
`3eeb31ac/da105c73/18f38838/7e895e1d`. No panic/DMA fault appeared. Final
cleanup found no QEMU/helper process, OrbStack test container, QMP socket, or
serial FIFO; only the expected browser fixture log remains. No unified
`make test`/`make fsck` was run.

### FP/SIMD EL0 context-isolation regression (targeted smoke passed; unified gates deferred)

The production hook keeps the ordinary AArch64 exception frame at 256 bytes and
uses a separate 528-byte, 16-byte-aligned FP/SIMD context only from the real
`preempt::save`/`load` switch path.  It saves/restores q0-q31 plus FPCR/FPSR,
enables FP/ASIMD on both boot CPUs, and has two EL0 round-robin workers install
distinct `0x11`/`0x22` signatures while checking every q register and both
control registers after each resume.  The soft-float target does not emit
compiler vector instructions; only the explicit assembly context helpers touch
the architectural FP/SIMD state.  Before the hook landed, the existing
round-robin demo only incremented integer memory and could not detect
q-register or FP control-state contamination.

The smoke gate now requires one production serial marker with this contract:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=N \
  checks=[A,B] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true \
  signatures=[0x11,0x22]
```

The two EL0 workers must install distinct q0-q31 byte patterns and distinct
FPCR/FPSR values, survive at least 20 timer-driven context switches, and check
the complete state after every resume.  `scripts/smoke-qemu.sh` rejects a
missing/malformed marker, fewer than 20 switches, zero checks for either task,
or any non-zero mismatch; the distinct signatures and explicit q0-q31 and
FPCR/FPSR fields prevent a banner-only implementation from satisfying the gate.

The first focused OrbStack attempt was stopped at the first setup error before
any build or QEMU run.  The exact command used `bash -lc` inside the existing
`microsystem-dev:rust-1.97.1` image; `bash` reported:

```text
bash: line 1: cargo: command not found
```

The image declares `/usr/local/cargo/bin` in `PATH`, so this is a login-shell
environment issue rather than a source compilation result.  After switching to
non-login `bash -c`, focused `bash -n` plus `cargo check` exited 0 (with only the
existing `invalidate_user_page` dead-code warning), recorded in
`target/fp-simd-focused-compile-rerun.log`.

The subsequent fresh OrbStack `make build` stopped before producing a kernel
ELF.  LLVM rejected the FP/SIMD helper's pair access at the 512-byte boundary:

```text
<inline asm>:600:22: index must be a multiple of 8 in range [-512, 504].
    stp x1, x2, [x0, #512]
```

The matching `ldp x1, x2, [x0, #512]` must use a valid encoding as well.  The
build log is `target/fp-simd-make-build.log`; make exited 1 and no targeted
smoke, unified `make test`, or `make fsck` has run.  After the production
offset encoding is corrected, rerun the targeted OrbStack sequence:

```text
docker context use orbstack
make build
MICROSYSTEM_QEMU_LOG=target/fp-simd-smoke-qemu.log scripts/smoke-qemu.sh
make test
make fsck
```

The offset encoding was corrected and a fresh build was retried against the
latest tree (`target/fp-simd-make-build-rerun.log`).  The next first failure is
the soft-float target rejecting the explicit FP/SIMD assembly itself:

```text
error: instruction requires: neon
  | dup v0.2d, x3
error: expected writable system register or pstate
  | msr fpcr, x5
error: instruction requires: fp-armv8
  | stp q0, q1, [x0, #0]
```

The kernel build emitted about 170 such diagnostics and exited 1; no kernel
ELF or targeted smoke was produced.  The explicit assembly needs an assembler
FP/NEON feature declaration (or a kernel-only target feature) while keeping
Rust soft-float code generation unchanged.  The smoke and unified gates remain
unrun.

After the assembler feature declaration landed, the next fresh OrbStack
`make build` exited 0 (`target/fp-simd-make-build-arch-rerun.log`).  The kernel
release ELF, bootfs, disk image, and CA bundle were rebuilt; only the existing
`invalidate_user_page` dead-code and SSHD unused-import warnings remained.
The one authorized targeted smoke was then run inside the same image without a
second build and exited 0 (`target/fp-simd-targeted-smoke-run.log`).  Its exact
FP/SIMD evidence was:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=20 checks=[712304,732293] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]
smoke-qemu: fp-simd context-switches=20 checks=[712304,732293] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true
```

The same smoke reported timer ticks `cpu0=2237 cpu1=2233`, scheduler
preemptions `[10,9]`, round-robin progress `[712304,732293]`, and normal file
chain completion.  A post-run scan found no QEMU process, no microsystem test
container, no listener on ports 2222/2223/5900, and no panic/DMA/translation
fault/timeout marker in `target/fp-simd-targeted-smoke-qemu.log`.  A stale
`target/gui-qmp.sock` from the earlier GUI packet remained at mtime 10:49; it
was not created by this smoke and was not removed.  Unified `make test` and
`make fsck` were intentionally not run.

Record the exact FP/SIMD marker and the smoke summary in this section only
after the targeted gate and the full `make test` complete successfully.

### Final unified acceptance (OrbStack, 2026-08-14)

With the production 256-byte exception frame, separate 528-byte FP/SIMD
switch context, and `.arch armv8-a+fp+simd` declarations in place, the
authorized host `make test` exited 0; full output is
`target/unified-fp-simd-final.log`.  Host suites passed 75/75 (MFS1 16,
capability ABI 9, GUI command stream 6, GUI desktop 9, kernel 11, Mica 24),
with all doc-tests reporting zero failures.

The unified smoke's FP/SIMD marker was:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=20 checks=[603697,686002] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]
```

The same run reported timer ticks `cpu0=4047 cpu1=4036`, timer preemptions
`[11,11]`, and round-robin progress `[603697,686002]`.  Powercut passed with
`first-status=0 second-status=0`; all five transaction/GC fault cuts passed
with expected recovery statuses; GUI, SSH, and Mica/TLS all passed.  The
follow-up authorized `make fsck` also exited 0; output is
`target/unified-fp-simd-fsck.log` and reports:

```text
MFS1 clean generation=2480 transactions=2323 entries=24 used_blocks=13431
```

Final residual checks found no QEMU/helper process, no microsystem container,
no listeners on ports 2222/2223/5900/5901, no target socket/FIFO, and no
panic, DMA fault, translation fault, QEMU fatal, or shutdown-timeout marker in
the unified log.  No further gate was run.

### MFS1 resilience and bounded repair acceptance (OrbStack, 2026-08-14)

The focused recovery gate was rerun once after the active-superblock ambiguity
guard landed:

```text
docker context show                         # orbstack
docker info --format '{{.OperatingSystem}} | {{.ServerVersion}}'
                                            # OrbStack | 29.4.0
docker run --rm -e MICROSYSTEM_CONTAINER=1 -e RUSTUP_TOOLCHAIN=1.97.1 \
  -v "$PWD:/workspace" -w /workspace microsystem-dev:rust-1.97.1 \
  cargo test -p mfs1 --test recovery -- --test-threads=1 --nocapture
                                            # 22 passed; 0 failed; 36.54s
```

The deterministic campaign prints and asserts its replay inputs and actual
fault consumption:

```text
mfs1-crash-campaign cases=10024 seed=0x4d46533143524153
mfs1-crash-campaign selected=[1668, 1711, 1626, 5019] triggered=[1668, 1711, 1626, 5019]
```

The four slots are write-I/O, write-short, write-torn (512-byte sectors), and
flush-I/O. Every case consumes its armed fault and remounts to a complete old
or new snapshot followed by `check()`. Repair regressions cover one stale
checksum-invalid mirror (heal and preserve data), active/latest checksum
corruption (refuse as ambiguous without writes), double corruption, unreadable
metadata, record corruption, and healthy no-op repair.

Focused host formatting passed with `rustfmt --edition 2024 --check` for the
changed recovery test. The OrbStack image does not contain the `rustfmt`/
`cargo-fmt` component (`rustfmt: not found` / component not installed), so no
toolchain component was installed solely for this check.

A fresh production build then passed through OrbStack:

```text
make build                                  # exit 0
```

Only the existing SSHD unused-import and kernel dead-code warnings remained.

The real `mfsctl` CLI boundary used temporary images and cleaned them after the
run. It verified healthy `mkfs + put + fsck`, damaged the stale mirror and
observed degraded `fsck`, repaired with `superblocks=1 verified=2/2`, then
confirmed clean `fsck` and `/payload` remained listed. A checksum loss in the
active/latest mirror was refused as `mfsctl: Corrupt` with the image hash
unchanged; double-mirror corruption was likewise refused with unchanged bytes:

```text
mfs1-cli-repair: PASS clean->degraded->stale-mirror-repaired->clean; latest-ambiguous-refused; double-refused-bytes-unchanged
```

The unified `make test` and `make fsck` gates were intentionally not rerun in
this focused packet.

### MFS1 resilience unified acceptance (OrbStack, 2026-08-14)

After the focused gate and CLI boundary passed, the authorized unified commands
were run once with first-error handling:

```text
make test > target/unified-mfs1-resilience-final.log 2>&1  # exit 0
make fsck  > target/unified-mfs1-resilience-fsck.log 2>&1   # exit 0
```

The host suites passed 22 MFS1 recovery tests (including the 10,024-case
campaign), 9 ABI, 6 GUI command-stream, 9 GUI desktop, 11 kernel, and 24 Mica
tests; all doc-tests reported zero failures. The unified recovery campaign ran
in the real gate and retained the fixed marker:

```text
mfs1-crash-campaign cases=10024 seed=0x4d46533143524153
mfs1-crash-campaign selected=[1668, 1711, 1626, 5019] triggered=[1668, 1711, 1626, 5019]
```

QEMU and integration gates all passed. Key evidence from the final log:

```text
smoke-qemu: PASS (serial milestones present)
smoke-qemu: fp-simd context-switches=20 checks=[717155,762092] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true
powercut-qemu: PASS ... first-status=0 second-status=0
fault-injection-qemu: PASS case=transaction-records-written result=old ...
fault-injection-qemu: PASS case=transaction-data-flushed result=old ...
fault-injection-qemu: PASS case=transaction-superblock-written result=new ...
fault-injection-qemu: PASS case=transaction-superblock-flushed result=new ...
fault-injection-qemu: PASS case=gc-candidate-superblock-flushed result=gc-proof ...
gui-qemu: PASS ...
ssh-qemu: PASS ... mica-eval-status=0 mica-repl-status=0 mica-file-status=0 ... recovery-status=0
mica-qemu: PASS bootfs=24/23 first-pid=13 mica-output=42 status=0
```

The only `Killed` lines in the fault log are the expected first-run power-cut
process termination; each is followed by a PASS result and clean fsck. The
follow-up fsck completed cleanly:

```text
MFS1 clean generation=2544 transactions=2381 entries=24 used_blocks=11898
```

Post-run checks found no microsystem/QEMU helper process, no microsystem test
container, and no listener on ports 2222/2223/5900/5901. Existing unrelated
OrbStack containers (Greatduty and shared infrastructure) remained untouched.
There is one pre-existing FIFO from an earlier fault run, not created by this
acceptance: `target/fault-injection-1786510519-706-6347/transaction-data-flushed.in`;
no socket was present. It was not removed without explicit narrow-cleanup
authorization.
