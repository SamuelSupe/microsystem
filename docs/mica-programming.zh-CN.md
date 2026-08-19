# Mica 编程说明

**简体中文** · [English](mica-programming.md)

Mica 是 MicroSystem 随系统提供的小型脚本语言。它以带 capability
broker 的 EL0 应用运行：脚本向文件系统、网络、进程、时间或 GUI broker
请求操作，broker 会先根据当前会话策略检查权限，再决定是否执行。

这份文档用于快速上手。完整的运行时契约和 wire-level 限制见
[Mica runtime](mica.md)；可运行的源代码示例位于
[`assets/mica/`](../assets/mica/)。

## 1. 第一个脚本

每个 Mica 文件脚本都从版本标记开始：

```mica
--!mica 1

print("hello from Mica")
print("uptime_ns", time.uptime())
```

在串口 shell 中运行一段表达式：

```text
mica -e 'print(40 + 2)'
```

启动交互式 REPL：

```text
mica
mica> print(6 * 7)
mica> exit
```

运行已经位于 guest MFS 中的文件。`--` 后面是传给脚本的参数：

```text
mica --allow fs.read:/data /data/example.mica -- first second
```

文件命令要求目标 MFS 路径已经存在。`run mica` 被有意拒绝：Mica 会通
过 script broker 与 `ThreadStartEx` 创建会话，而不是走普通的静态应用加载
器。

## 2. 语言基础

Mica 支持 `nil`、布尔值、有符号十进制/十六进制整数、浮点数、UTF-8
字符串、bytes、table 和函数。语言包含局部变量、赋值、多返回值、词法闭
包、`if`/`elseif`/`else`、`while`、数值/table `for`、`break`、`return`、索
引、字段访问、方法调用和分号。

```mica
--!mica 1

local describe = function(name, score)
    if score >= 60 then
        return name + " passed", true
    else
        return name + " needs practice", false
    end
end

local message, passed = describe("Mica", 96)
print(message, passed)

local values = {2, 4, 6}
local total = 0
for index = 1, table.len(values) do
    total = total + values[index]
end
print("total", total)
```

运算符包括算术运算（`+ - * / // %`）、比较、相等判断、短路
`and`/`or` 与 `not`。示例中的 `+` 同时支持数值相加和字符串拼接。

运行时内建函数包括 `type`、`tostring`、`assert`、`error`、`pcall` 和
`require`。

## 3. 模块与标准库

使用 `require` 加载内置模块或 MFS 中的 UTF-8 `.mica` 模块：

```mica
--!mica 1

local json = require("json")
local status = {
    state = "ready",
    value = 42,
}

print(json.encode(status))
```

内置模块如下：

| 模块 | 主要函数 |
| --- | --- |
| `json` | `encode`、`decode` |
| `encoding` | 十六进制、Base64、UTF-8 校验 |
| `hash` | `sha256`、`hmac_sha256` |
| `string` | 长度、大小写、切片、查找、trim、替换、空白折叠 |
| `bytes` | 字符串与 bytes 互转、长度 |
| `table` | `len`、`insert`、`remove`、`next` |
| `math` | `abs`、`floor`、`ceil`、`min`、`max` |
| `fs` | 文件和目录操作 |
| `proc` | 受策略控制的列出、启动、等待、终止 |
| `time` | `uptime`、`sleep`、`realtime` |
| `sys` | `version`、`stats` |
| `random` | `bytes`、`int` |
| `net` | DNS、TCP、UDP 与取消原语 |
| `http` | `request`、`get`、`post`、`put`、`patch`、`delete` |
| `io` | 标准输入/输出操作 |
| `args` | `get`、`all` |

模块在每个会话内缓存。模块是从 MFS 读取的 Mica 源码，不是动态链接的
原生库。请保持模块较小：单个模块最多 64 KiB，一个会话的模块源码总量
最多 256 KiB。

## 4. 权限也是程序的一部分

文件脚本在 `--!mica 1` 之后、第一条非注释语句之前声明权限：

```mica
--!mica 1
--!allow fs.read:/data
--!allow fs.write:/data/output
--!allow net.connect:10.0.2.2:8080

print("this script has explicit capabilities")
```

常见规则包括：

```text
fs.read:/data                 路径前缀读权限
fs.write:/data/output        路径前缀写权限
net.connect:example.test:443 精确主机与端口
net.browse                   内置 Reader 的 HTTP/HTTPS GET 路径
proc.list                    查看进程
proc.spawn:counter           启动指定名称的应用
proc.kill:counter            控制指定名称的进程
sys.stats                    系统统计信息
random                       随机源
gui.window                   一个保留式 GUI 窗口
```

`fs.read` 和 `fs.write` 是绝对路径前缀权限；`net.connect` 是大小写不敏
感的精确主机/端口规则，不接受通配符。非法路径、越过根目录的 `..`、NUL、
通配符和格式错误的规则都会被拒绝。

文件会话的有效策略是：脚本 manifest、启动器 `--allow` 规则和固定入口策略
三者的交集。脚本不能通过写一条声明给自己增加能力。`net.browse` 是内置
Reader 使用的特殊 GET 权限，不授予原始 DNS/TCP/UDP，也不授权
POST/PUT/PATCH/DELETE。

## 5. 文件操作与结构化错误

broker 调用成功时返回值，失败时通常返回 `nil, error`。错误是带有
`kind`、`message`、`operation` 和数字 `code` 字段的 table。使用返回值之
前先检查它：

```mica
--!mica 1
--!allow fs.read:/data
--!allow fs.write:/data/output

local json = require("json")
local status = {
    generated_at = time.uptime(),
    source = "/data",
}

local ok, err = fs.write_file(
    "/data/output/status.json",
    json.encode(status),
    {atomic = true, fsync = true}
)
if not ok then
    error(err.message)
end
```

常用文件函数包括 `stat`、`list`、`open`、`read`、`write`、`close`、
`read_file`、`write_file`、`mkdir`、`rename`、`unlink`、`fsync` 和 `sync`。
`read_file` 默认最多读取 64 KiB，允许的最大值为 256 KiB。需要持久化配置
或用户数据时，优先使用 `write_file(..., {atomic = true, fsync = true})`，
并检查返回的错误。

脚本也可以用 `pcall` 将脚本/运行时错误转成普通返回值：

```mica
local ok, value = pcall(function()
    return assert(false, "demonstration failure")
end)
if not ok then
    print("caught", value)
end
```

`pcall` 不会取消资源限制。指令数、超时、中断、栈、调用深度和堆限制仍
然会以 VM 结果返回。

## 6. HTTP 与网络

网络能力受权限控制。对固定目标发起 HTTP 请求时，可以声明精确的
`net.connect` 规则：

```mica
--!mica 1
--!allow net.connect:10.0.2.2:8080

local http = require("http")
local response, err = http.get("http://10.0.2.2:8080/status")
if not response then
    error(err.message)
end

local body, read_err = response:read_all(1048576)
if not body then
    error(read_err.message)
end
print(response.status, body)
```

`http` 支持 HTTP/HTTPS、调用方请求头、`Content-Length`、chunked response
以及标准库表中列出的方法。响应会被缓冲，不是 streaming API。3xx 响应
会原样返回，不会自动跟随跳转。底层 `net.tls_connect` 有意返回
`NotSupported`；受信任的 HTTPS 路径位于 Mica runtime 内部。

内置 Reader 使用 `net.browse`，对运行时选择的主机执行受限的 HTTP/HTTPS
GET。原始 `net.resolve`、TCP、UDP 和非 GET 操作仍需要精确权限。原始网卡
能力只属于 `netd`；Mica 只获得 broker 访问，不会得到网卡或 MMIO capability。

## 7. GUI 应用

GUI 只能通过文件脚本运行，`-e` 和 REPL 不能创建 GUI 会话。GUI 文件必须
声明 `gui.window`，并使用 `--gui` 启动：

```text
mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica
```

下面是一个最小的 GUI 程序：

```mica
--!mica 1
--!allow gui.window

local gui = require("gui")
local count = 0
local label = gui.label {text = "Count: 0"}
local app, err = gui.window {
    title = "Mica Counter",
    width = 420,
    height = 260,
}
if not app then error(err.message) end

app:set_root(gui.column {
    padding = 16,
    gap = 12,
    children = {
        label,
        gui.button {
            text = "Increment",
            on_click = function()
                count = count + 1
                label:set_text("Count: " + tostring(count))
                app:invalidate()
            end,
        },
    },
})

app:run()
```

保留式 toolkit 包含 `window`、`label`、`button`、`text_input`、`checkbox`、
`list`、`scroll`、`row`、`column`、`spacer` 和 `canvas`。窗口方法包括
`set_root`、`set_title`、`invalidate`、`present`、`run` 和 `close`。控件支
持 `set_text`、`set_checked`、`set_items`，以及 `on_click`、`on_change`、
`on_submit`、`on_select` 回调。

一个脚本拥有一个顶层窗口。GUI 任务只获得已校验的 command、event 和端点
capability；它不会获得 framebuffer、GPU、输入设备、DMA、BAR 或 IRQ 权限。

## 8. 参数、时间与限制

参数放在 `--` 后面，通过 `args` 读取：

```mica
--!mica 1

local args = require("args")
print("first argument", args.get(1))
```

常用运行时函数包括 `time.uptime()`、`time.sleep(milliseconds)`、
`time.realtime()`、`sys.version()`，以及在策略允许时使用的 `sys.stats()`。

重要限制包括：

| 资源 | 限制 |
| --- | ---: |
| `compile` 接收的源码 | 64 KiB |
| 生成的 bytecode | 128 KiB |
| 调用帧 | 128 |
| 操作数栈 | 4,096 个值 |
| VM heap accounting | 默认 512 KiB |
| 指令数 | 默认 10,000,000 |
| 文件脚本墙钟时间 | 默认 10 秒 |
| REPL 墙钟时间 | 默认 5 秒 |

非 GUI 会话硬上限为 60 秒。GUI 文件会话可以请求最多 24 小时，但仍受
资源限制和清理窗口约束。

## 9. 实用开发流程

1. 从 `--!mica 1` 和一个可观察的 `print` 开始。
2. 只添加脚本真正需要的 `--!allow` 规则。
3. 用 `require` 使用标准模块，并检查每个 broker 错误。
4. 显式限制文件读取、网络 body、table 大小和循环工作量。
5. 持久化状态使用 atomic write 加 `fsync`。
6. GUI 代码采用事件驱动；状态变化后调用 `app:invalidate()`。
7. 通过串口 shell、SSH 或带策略的 GUI 启动器运行脚本，不要使用
   `run mica`。

完整契约见 [Mica runtime](mica.md)。更多示例请阅读
[`fs-status.mica`](../assets/mica/fs-status.mica)、
[`http-status.mica`](../assets/mica/http-status.mica)、
[`gui-counter.mica`](../assets/mica/gui-counter.mica)、
[`browser.mica`](../assets/mica/browser.mica) 和
[`editor.mica`](../assets/mica/editor.mica)。
