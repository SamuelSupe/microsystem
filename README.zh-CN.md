# MicroSystem

![AArch64](https://img.shields.io/badge/target-AArch64%20%2B%20QEMU-2563eb)
![Rust](https://img.shields.io/badge/implementation-Rust%202024-f97316)
![Status](https://img.shields.io/badge/status-research%20prototype-7c3aed)

> 一次由 AI 辅助、从 AArch64 微内核延伸到存储、网络、SSH、GUI 与能力感知脚本运行时的完整操作系统生态实现尝试。

**简体中文** · [English](README.md)

MicroSystem 是一个小型、明确、端到端的操作系统实验。它并不是把 Linux 缩小重写，而是尝试回答一个更具体的问题：AI 能否帮助我们把一致的系统设计，从启动代码与硬件隔离一路推进到用户可操作的应用生态。

## 运行画面

下面的画面来自仓库已有的 QEMU GUI profile；该 profile 会通过默认 `5900` 端口向 VNC 提供 guest 画面，是一张真实 GUI 帧：其中包含代码绘制的桌面、Terminal、Mica Counter、Monitor，以及 Files、Reader、Editor 启动器。

![通过 GUI/VNC 路径捕获的 MicroSystem 桌面](docs/assets/microsystem-vnc.png)

*这是此前 VNC-backed GUI 运行产生的 QMP screendump，不是 mockup，也不是 VNC 客户端外壳截图；本次仅整理文档，没有现场重新捕获。*

## 系统架构

设计将必须接触硬件的机制保留在 EL1，并把设备策略放到相互隔离的 EL0 服务中。应用通过 broker IPC 与 capability 使用资源，不直接获得原始 framebuffer、块设备、网络、DMA 或 IRQ 权限。

![MicroSystem 系统架构图](docs/assets/microsystem-architecture.svg)

主路径如下：

```text
QEMU virt-7.2 硬件
          ↓
EL1 微内核：MMU · 调度 · IPC · capability · IRQ/IOMMU
          ↓
EL0 服务：init · devmgr · block · MFS1 · netd · sshd · windowd
          ↓
Mica 脚本与 GUI 应用：Counter · Reader · Editor · Terminal
```

## 已实现的部分

| 层次 | 当前实验内容 |
| --- | --- |
| 内核 | AArch64 EL1 启动、高半区 MMU、GICv3 定时器/中断、调度、ASID、带 W^X 校验的 ELF 加载、capability 派生/撤销与有界资源回收 |
| 隔离 | 基于 DTB 的 VirtIO 发现、SMMUv3 域设置、设备授予，以及严格的 EL1 机制 / EL0 策略边界 |
| 存储 | MFS1 事务文件系统、元数据镜像、fsck、受限离线修复，以及断电和确定性故障注入路径 |
| 网络与访问 | EL0 `netd`、端点 broker 网络、有限 HTTP/HTTPS 访问、SSH 会话，以及脚本权限交集策略 |
| 桌面 | VirtIO-GPU/输入设备 profile、代码绘制的 1024×768 桌面、保留式 GUI 命令流、窗口管理、输入批处理、damage culling，以及 Terminal、Files、Monitor、Reader、Editor |
| 运行时 | Mica 词法器/编译器/VM、能力感知权限、文件系统/网络/GUI broker，以及有界应用槽位 |

当前仓库面向 QEMU `virt-7.2` AArch64 机器：双 CPU、4 KiB 页、256 MiB 内存。它是研究型原型，不是 Linux/POSIX 发行版、通用桌面系统，也不承诺任意真实硬件或 ELF 兼容性。

## 快速开始

推荐使用固定 Rust toolchain 与 OrbStack Docker context：

```sh
docker context show                         # 预期：orbstack
make build
make test
make fsck
```

运行串口 shell：

```sh
make run
```

运行 GUI profile。QEMU 命令会打印 VNC/QMP 端点；默认 VNC 端口是 `5900`。

```sh
make gui
```

Mica 示例：

```text
mica -e 'print(40 + 2)'
mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica
mica --gui --timeout 86400s --allow gui.window --allow net.browse \
  /.system/examples/mica/browser.mica -- https://example.com/
```

## 已记录的验收证据

仓库保留了详细 runbook 与原始 workflow 结果。已有验收矩阵记录：MFS recovery `22/22`、ABI `9/9`、GUI command stream `6/6`、GUI Desktop `9/9`、kernel boundaries `11/11`、Mica `24/24`，合计 `81/81` 个 host tests；同时覆盖 QEMU smoke、SSH、GUI、TLS、断电与 MFS1 故障/恢复 gate。

这些数字是仓库保留的历史证据。本次 GitHub 发布有意不编译、不重跑系统；证据来源与边界见 [`docs/runbook.md`](docs/runbook.md) 和 [`.workflow/microsystem-kernel/results/tests.md`](.workflow/microsystem-kernel/results/tests.md)。

## 仓库结构

```text
crates/kernel/     EL1 内核与 AArch64 启动/运行机制
crates/mfs1/       MFS1 文件系统实现
crates/mica/       Mica 语言、VM、权限与标准库
crates/gui/        GUI ABI 与命令流类型
services/          静态 EL0 服务与示例应用
assets/            guest 镜像使用的 CA 与字体输入
docs/              架构、ABI、GUI、MFS1、网络、SSH 与 runbook 文档
scripts/           OrbStack/QEMU 构建和验收辅助脚本
xtask/             镜像生成与 QEMU 编排
```

## 推荐阅读

- [系统架构](docs/architecture.md) — 启动、地址空间、IPC、capability、存储与 GUI 边界。
- [GUI profile](docs/gui.md) — windowd、保留式 Present、输入、启动策略与 VNC/QEMU 路径。
- [Mica runtime contract](docs/mica.md) — 语言、VM 限制、权限与 broker API。
- [MFS1](docs/mfs1.md) — 事务格式、恢复、故障模型、fsck 与修复边界。
- [网络与 `netd`](docs/network.md) — 端点策略与有界网络能力。
- [SSH](docs/ssh.md) — SSH 命令与启动策略。
- [Runbook](docs/runbook.md) — 可复现命令与已有验收证据。

## 项目意图

MicroSystem 刻意保持足够小，便于阅读；同时保持足够完整，能够暴露真实取舍。核心问题很实际：AI 辅助的工程循环，能否交付的不只是零散内核代码，而是一套在存储、网络、GUI、应用、故障处理和安全边界上保持一致的操作系统生态？

答案仍在探索中。欢迎贡献代码、提出批评，并复现实验。

## 许可证

Rust workspace 声明许可证为 `MIT OR Apache-2.0`。具体适用条款以源文件头部与 package metadata 为准。
