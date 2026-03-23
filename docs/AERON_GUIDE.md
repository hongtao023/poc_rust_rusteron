# Aeron 传输架构指南

> 面向工程师的 Aeron 快速上手指南，涵盖核心概念、通信模式、Channel 配置、Archive 架构与故障恢复。

---

## 目录

1. [核心概念](#1-核心概念)
2. [通信模式](#2-通信模式)
3. [传输协议](#3-传输协议)
4. [Channel URI 参数详解](#4-channel-uri-参数详解)
5. [消息可靠性与排序](#5-消息可靠性与排序)
6. [Aeron Archive](#6-aeron-archive)
7. [Archive 独立部署架构](#7-archive-独立部署架构)
8. [故障恢复](#8-故障恢复)
9. [生产环境配置建议](#9-生产环境配置建议)

---

## 1. 核心概念

### 1.1 整体架构

```
┌─────────────────────────────────────────┐
│              应用进程                     │
│  ┌─────────────┐   ┌──────────────┐     │
│  │ Publication  │   │ Subscription │     │
│  └──────┬──────┘   └──────┬───────┘     │
│         │ 共享内存          │ 共享内存     │
│  ┌──────┴──────────────────┴───────┐    │
│  │        Aeron Client (API)       │    │
│  └──────────────┬──────────────────┘    │
└─────────────────│───────────────────────┘
                  │ 共享内存 (mmap)
┌─────────────────│───────────────────────┐
│  ┌──────────────┴──────────────────┐    │
│  │         Media Driver            │    │  ← 可内嵌或独立进程
│  │  ┌─────────┐  ┌──────────────┐  │    │
│  │  │ Sender  │  │   Receiver   │  │    │
│  │  └────┬────┘  └──────┬───────┘  │    │
│  └───────│──────────────│──────────┘    │
└──────────│──────────────│───────────────┘
           │ UDP/IPC      │ UDP/IPC
         网络 / 共享内存
```

**Media Driver** 是 Aeron 的核心引擎，负责实际的网络 I/O。应用通过共享内存与 Driver 通信，避免了系统调用和数据拷贝的开销。

### 1.2 六个核心实体

| 实体 | 说明 |
|---|---|
| **Media Driver** | 传输引擎，管理网络收发。可内嵌在应用进程中或作为独立守护进程运行 |
| **Aeron Client** | 应用层 API 入口，通过共享内存连接到 Media Driver |
| **Publication** | 发送端抽象。绑定 Channel + Stream ID，每个 Publication 拥有唯一 session_id |
| **Subscription** | 接收端抽象。订阅 Channel + Stream ID，通过 `poll()` 拉取消息 |
| **Image** | Subscription 内部对象——一个 Publisher session 在 Subscriber 端的投影。每个连接进来的 Publication session 对应一个 Image |
| **Fragment** | Aeron 中的最小传输单元。大消息会被拆分为多个 fragment |

### 1.3 Channel、Stream ID 与 Session ID 的关系

```
Channel (URI)          = 传输通道（协议 + 地址 + 网络参数）
  └── Stream ID (i32)  = 逻辑数据流（在同一 Channel 上复用）
        └── Session ID = 单个 Publication 实例的唯一标识
```

**匹配规则**：Publication 和 Subscription 必须 Channel + Stream ID 都匹配才能通信。

```
// 类比
Channel   ≈ 一根网线（物理传输）
Stream ID ≈ 网线上的逻辑频道号
Session   ≈ 某个发送者在该频道上的会话
```

**示例**：同一个 UDP 端口上跑多个独立数据流

```
aeron:udp?endpoint=10.0.0.5:20121  stream_id=1001  → 客户端 A 的数据
aeron:udp?endpoint=10.0.0.5:20121  stream_id=2001  → 客户端 B 的数据
aeron:udp?endpoint=10.0.0.5:20121  stream_id=3001  → 客户端 C 的数据
           同一个 UDP socket                         不同的逻辑流
```

### 1.4 Image 详解

Image 是 Subscriber 端看到的"一个 Publisher 的连接"：

```
Publisher A (session 100) ──┐
                            ├──> Subscription (stream 1001)
Publisher B (session 200) ──┘     ├─ Image A ← session 100 的数据
                                  └─ Image B ← session 200 的数据
```

**Image 包含的信息**：

| 字段 | 说明 |
|---|---|
| session_id | 对应哪个 Publisher |
| term buffers | 接收到的消息数据（共享内存中的 log buffer） |
| subscriber_position | 当前消费到的位置 |
| source_identity | Publisher 的地址信息 |

**Image 生命周期**（通过回调通知）：

```
Publisher 上线 → 创建 Image → 触发 available_image 回调 → 开始收消息
Publisher 下线 → Image 失效 → 触发 unavailable_image 回调 → 清理资源
Publisher 重连 → 新 Image   → 再次触发 available_image → 自动恢复
```

Subscription 不需要重建，自动感知 Publisher 的上下线。

---

## 2. 通信模式

### 2.1 支持的模式

| 模式 | 描述 | 典型场景 |
|---|---|---|
| 1 Pub : 1 Sub | 点对点 | 请求-响应、RPC |
| 1 Pub : N Sub | 一发多收（扇出） | 行情分发、事件广播 |
| N Pub : 1 Sub | 多发一收（扇入） | 数据聚合、日志收集 |
| N Pub : M Sub | 多对多 | 以上组合 |

### 2.2 一发多收 (1:N)

一个 Publisher 的数据被多个 Subscriber 接收。

**方式 A：Multicast（网络层扇出）**

```
Publisher ──1份UDP包──> 交换机 ──┬──> Subscriber A
                                ├──> Subscriber B
                                └──> Subscriber C
```

```
// 发送方和所有接收方使用同一个组播地址
channel = "aeron:udp?endpoint=224.0.1.1:40456|interface=eth0"
```

发送方只发一份，网络层复制。带宽不随接收方数量增长。

**方式 B：MDC（应用层扇出）**

```
Publisher ──MDC──┬──> Subscriber A
                 ├──> Subscriber B
                 └──> Subscriber C
```

```
// 发送方
pub_channel = "aeron:udp?control=10.0.0.1:20200|control-mode=dynamic"

// 接收方（动态注册）
sub_channel = "aeron:udp?control=10.0.0.1:20200|control-mode=dynamic|endpoint=10.0.0.3:20121"
```

不需要组播网络支持，Subscriber 可动态加入/退出。

**方式 C：Unicast 逐个发送**

```
// 为每个接收方创建独立的 Publication
pub_to_A = "aeron:udp?endpoint=10.0.0.2:20121"
pub_to_B = "aeron:udp?endpoint=10.0.0.3:20121"
pub_to_C = "aeron:udp?endpoint=10.0.0.4:20121"
```

N 个接收方 = N 份数据。灵活但不高效。

### 2.3 多发一收 (N:1)

多个 Publisher 发布到同一个 Channel + Stream ID，Subscription 收到所有消息。

```
Publisher A (session 100) ──┐
Publisher B (session 200) ──┼──> Subscription(stream 1001)
Publisher C (session 300) ──┘
```

- 每个 Publisher 的消息在各自 Image 内保证有序
- **跨 Publisher 的消息无全局顺序保证**
- Subscriber 可通过 `AeronHeader` 中的 session_id 区分来源

---

## 3. 传输协议

### 3.1 三种传输方式对比

| | UDP Unicast | UDP Multicast | IPC |
|---|---|---|---|
| URI 前缀 | `aeron:udp` | `aeron:udp` | `aeron:ipc` |
| 通信范围 | 跨机器 | 跨机器（同子网/路由可达） | 同一台机器 |
| 网络要求 | 普通 UDP | 需要 IGMP 支持 | 无网络 |
| 延迟 | 微秒级 | 微秒级 | 纳秒级（共享内存） |
| 选择依据 | 点对点通信 | 大规模扇出 | 同机进程间通信 |

### 3.2 选择决策树

```
同一台机器?
├─ 是 → IPC（最低延迟）
└─ 否 → 多少接收方?
         ├─ 1个     → Unicast
         ├─ 少量    → Unicast 或 MDC
         └─ 大量    → 网络支持组播?
                      ├─ 是 → Multicast
                      └─ 否 → MDC
```

### 3.3 URI 示例

```
aeron:udp?endpoint=10.0.0.5:20121                            // unicast
aeron:udp?endpoint=224.0.1.1:40456|interface=eth0             // multicast
aeron:udp?control=10.0.0.1:20200|control-mode=dynamic        // MDC
aeron:ipc                                                     // 进程间
```

---

## 4. Channel URI 参数详解

### 4.1 URI 格式

```
aeron:<transport>?key1=value1|key2=value2|key3=value3
```

> **注意**：参数之间用 `|`（竖线）分隔，不是 `&`。

### 4.2 核心参数

#### endpoint

对端地址，最基础的参数。

```
aeron:udp?endpoint=10.0.0.5:20121
```

| 角色 | 含义 |
|---|---|
| Publication | 消息发往的目标地址 |
| Subscription (unicast) | 绑定并监听的本地地址 |
| Subscription (multicast) | 加入的组播组地址 |

#### interface

绑定的网卡，多网卡机器上必须指定。

```
aeron:udp?endpoint=224.0.1.1:40456|interface=10.0.0.5   // 用 IP
aeron:udp?endpoint=224.0.1.1:40456|interface=eth0        // 用网卡名
```

#### control / control-mode

MDC 模式的控制通道。

```
// Publisher：监听控制端口
aeron:udp?control=10.0.0.1:20200|control-mode=dynamic

// Subscriber：通过控制端口注册
aeron:udp?control=10.0.0.1:20200|control-mode=dynamic|endpoint=10.0.0.3:20121
```

| control-mode | 说明 |
|---|---|
| `dynamic` | Subscriber 可随时加入/退出 |
| `manual` | 由应用代码手动管理目标列表 |

#### reliable

是否启用可靠传输（NAK 重传）。

```
aeron:udp?endpoint=10.0.0.5:20121|reliable=true    // 默认，丢包会重传
aeron:udp?endpoint=10.0.0.5:20121|reliable=false   // 不重传，适合只要最新数据的场景
```

#### mtu

最大传输单元（单个 Aeron frame 的最大字节数）。

```
aeron:udp?endpoint=10.0.0.5:20121|mtu=8192
```

| 值 | 说明 |
|---|---|
| 1408（默认） | 适配标准以太网 MTU 1500 |
| 8192 | 需要 jumbo frame 支持（网络 MTU ≥ 9000） |

更大的 MTU 减少大消息的分片开销，提升吞吐量，但不能超过网络实际 MTU。

#### term-length

Log buffer 中每个 term 的大小（必须是 2 的幂，范围 64KB ~ 1GB）。

```
aeron:udp?endpoint=10.0.0.5:20121|term-length=16777216   // 16MB（默认）
```

| 策略 | 值 | 场景 |
|---|---|---|
| 大 buffer | 16MB ~ 64MB | 高吞吐、允许 burst |
| 小 buffer | 64KB ~ 1MB | 大量低速流、节省内存 |

#### ttl

组播 TTL（Time To Live），控制组播包穿越路由器的跳数。

```
aeron:udp?endpoint=224.0.1.1:40456|ttl=4
```

| 值 | 范围 |
|---|---|
| 0 | 仅本机 |
| 1（默认） | 同一子网 |
| 2 ~ 31 | 跨路由器 |

#### sparse

是否使用稀疏文件映射 term buffer。

```
aeron:udp?endpoint=10.0.0.5:20121|sparse=false   // 默认，预分配物理内存
aeron:udp?endpoint=10.0.0.5:20121|sparse=true    // 按需分配，节省内存
```

生产环境建议 `false`，避免运行时 page fault 导致延迟抖动。

#### session-id

手动指定 Publication 的 session ID（默认由 Driver 自动分配）。

```
aeron:udp?endpoint=10.0.0.5:20121|session-id=12345
```

Archive 回放或需要可预测 session 分配时使用。

#### linger

Publication 关闭后 buffer 保留时间（纳秒）。

```
aeron:udp?endpoint=10.0.0.5:20121|linger=5000000000   // 5 秒（默认）
```

#### tags

给 Channel 打标签，共享底层传输资源。

```
aeron:udp?endpoint=10.0.0.5:20121|tags=1001
aeron:udp?tags=1001                              // 引用已有 channel
```

#### eos

Publication 关闭时是否发送 End-of-Stream 信号。

```
aeron:udp?endpoint=10.0.0.5:20121|eos=true   // 默认
```

### 4.3 参数速查表

| 参数 | 适用传输 | 默认值 | 说明 |
|---|---|---|---|
| `endpoint` | UDP | 必填 | 目标/绑定地址 |
| `interface` | UDP | 系统默认 | 绑定网卡 |
| `control` | UDP | — | MDC 控制地址 |
| `control-mode` | UDP | — | `dynamic` / `manual` |
| `reliable` | UDP | `true` | 可靠传输开关 |
| `ttl` | UDP (multicast) | 0 | 组播 TTL |
| `mtu` | UDP | 1408 | 最大帧大小 |
| `term-length` | UDP / IPC | 16MB | term buffer 大小 |
| `linger` | UDP / IPC | 5s | 关闭后 buffer 保留 |
| `sparse` | UDP / IPC | `false` | 稀疏内存映射 |
| `session-id` | UDP / IPC | 自动 | 手动指定 session |
| `tags` | UDP / IPC | — | 共享底层资源 |
| `eos` | UDP / IPC | `true` | 关闭时发 EOS |

### 4.4 常用组合

```
// 最简 unicast
aeron:udp?endpoint=10.0.0.5:20121

// 高吞吐 unicast
aeron:udp?endpoint=10.0.0.5:20121|mtu=8192|term-length=16777216

// 组播
aeron:udp?endpoint=224.0.1.1:40456|interface=eth0|ttl=4

// 不可靠组播（行情推送，只要最新的）
aeron:udp?endpoint=224.0.1.1:40456|interface=eth0|reliable=false

// MDC 动态扇出
aeron:udp?control=10.0.0.1:20200|control-mode=dynamic

// 低延迟生产配置
aeron:udp?endpoint=10.0.0.5:20121|sparse=false|term-length=8388608|mtu=8192

// 进程间通信
aeron:ipc
```

---

## 5. 消息可靠性与排序

### 5.1 offer() 的语义

`publication.offer()` 返回正值 **仅代表消息写入了本地 log buffer**，不代表对方已收到。

```
offer() 写入 → Media Driver 发送 → 网络传输 → 对端 Driver 接收 → poll() 读取
    ↑
  offer()>0 只保证到这里
```

**offer() 返回值**：

| 返回值 | 含义 | 处理方式 |
|---|---|---|
| > 0 | 成功写入，返回新 position | 继续 |
| -1 (BACK_PRESSURED) | 发送缓冲区满 | 等待后重试 |
| -2 (NOT_CONNECTED) | 无 Subscriber 连接 | 等待连接或报错 |
| -3 (ADMIN_ACTION) | 内部管理操作 | 立即重试 |
| -4 (CLOSED) | Publication 已关闭 | 不可恢复 |
| -5 (MAX_POSITION_EXCEEDED) | 超出最大位置 | 不可恢复 |

### 5.2 传输可靠性

**在 Publisher 和 Subscriber 已建立连接的前提下**，Aeron 提供可靠有序传输：

- Subscriber 检测到序号空洞时发送 **NAK（否定应答）**
- Publisher 端 Media Driver **重传**丢失的数据
- 在 flow control 窗口内保证可靠交付

**边界条件**：

| 场景 | 行为 |
|---|---|
| 无 Subscriber 连接 | 消息写入 buffer 但无人接收，最终被覆盖 |
| Subscriber 消费太慢 | 先触发背压，持续落后则 Image 可能被断开 |
| 网络持续丢包超过重传窗口 | Image 关闭，连接断开 |

### 5.3 排序保证

| 范围 | 保证 | 原因 |
|---|---|---|
| 单个 Publication (单 session) 内 | **严格 FIFO，确定性** | log buffer 基于 position 顺序写入 |
| 跨 Publication (跨 session) | **不保证顺序** | 各 Image 独立，poll() 轮流拉取 |

如需跨 Publisher 全局有序，常见做法：

1. **消息体内嵌逻辑时间戳**，接收端排序
2. **引入 Sequencer 节点**：所有 Publisher 先发给 Sequencer，统一编号后转发。下游只有一个 session，天然全局有序

### 5.4 端到端确认

Aeron 不提供内建的端到端 ACK。如需确认对方收到，需应用层实现：

- Ping-Pong 模式（发一条等回复）
- 批量 ACK（每 N 条回复一次确认）
- 事后核对（发送总量 vs 接收总量）

---

## 6. Aeron Archive

### 6.1 Archive 是什么

Aeron Archive 在 Aeron 传输层之上提供**持久化录制与回放**能力：

```
┌───────────────────────────────────────┐
│            Aeron Archive              │
│  ┌─────────┐  ┌────────┐  ┌───────┐  │
│  │ 录制引擎 │  │ 回放引擎│  │ 控制  │  │
│  │ (Record)│  │(Replay)│  │(Ctrl) │  │
│  └────┬────┘  └───┬────┘  └──┬────┘  │
│       │ write     │ read     │       │
│  ┌────┴───────────┴──────────┘       │
│  │       持久化存储 (磁盘)            │
│  │  recording-0.dat                  │
│  │  recording-1.dat                  │
│  │  ...                              │
│  └───────────────────────────────────┘
└───────────────────────────────────────┘
```

### 6.2 五类 Channel

Archive 涉及五类通道：

| Channel | 方向 | 用途 |
|---|---|---|
| **Control Request** | Client → Archive | 发送命令（开始录制、请求回放等） |
| **Control Response** | Archive → Client | 返回命令执行结果 |
| **Recording** | Publisher → Archive | Archive 订阅此通道录制数据 |
| **Replay** | Archive → Client | Archive 在此通道上回放录制数据 |
| **Live** | Publisher → Subscriber | 正常的实时数据流（可选） |

### 6.3 录制粒度

Archive 的录制粒度是 **per Image（per session）**：

- 同一个 stream_id 上有 N 个 Publisher → N 个 Image → **N 个独立 Recording**
- 每个 Recording 有唯一 `recording_id`
- 回放时按 `recording_id` 指定，回放的是单个 session 的数据

```
Subscription (stream 1001)
  ├─ Image (session 100) → Recording #1
  └─ Image (session 200) → Recording #2
```

### 6.4 Recording 内的顺序

| 维度 | 保证？ | 说明 |
|---|---|---|
| 单个 Recording 内 | **严格保证** | 与原始发送顺序完全一致，position-based |
| 跨 Recording | **不保证** | 各 Recording 独立，无全局时序 |
| 回放确定性 | **确定** | 同一 Recording 每次回放顺序完全相同 |

---

## 7. Archive 独立部署架构

### 7.1 节点规划

```
Publisher:   10.0.0.1
Archive:     10.0.0.2
Subscriber:  10.0.0.3
```

### 7.2 Channel 配置全景

```
 10.0.0.1                  10.0.0.2                       10.0.0.3
┌──────────┐              ┌───────────────────┐           ┌───────────────┐
│Publisher  │              │  Archive Node     │           │  Subscriber   │
│          │  ③ 录制数据   │                   │           │               │
│ Pub ─────│─────────────>│ Sub (录制)        │           │               │
│ :20121   │  stream 1001 │ :20121            │           │               │
│          │              │     │ 写磁盘       │           │               │
│          │              │     ▼              │           │               │
│          │              │ /data/archive/     │           │               │
│          │              │                   │  ④ 回放   │               │
│          │              │ Pub (回放) ───────│──────────>│ Sub (回放)    │
│          │              │ → :30001          │  replay   │ :30001        │
│          │              │                   │           │               │
│          │              │ ① 控制请求 Sub ◄──│───────────│ Pub (控制)    │
│          │              │    :8010          │  stream 0 │ → :8010       │
│          │              │                   │           │               │
│          │              │ ② 控制响应 Pub ───│──────────>│ Sub (控制)    │
│          │              │ → :8020           │  stream 0 │ :8020         │
└──────────┘              └───────────────────┘           └───────────────┘
```

**各 Channel 的具体 URI**：

```
① Control Request:
   Archive  Sub = "aeron:udp?endpoint=10.0.0.2:8010"  stream_id=0
   Client   Pub = "aeron:udp?endpoint=10.0.0.2:8010"  stream_id=0

② Control Response:
   Client   Sub = "aeron:udp?endpoint=10.0.0.3:8020"  stream_id=0
   Archive  Pub = "aeron:udp?endpoint=10.0.0.3:8020"  stream_id=0

③ Recording (Publisher → Archive):
   Publisher Pub = "aeron:udp?endpoint=10.0.0.2:20121" stream_id=1001
   Archive 调用:  archive.start_recording("aeron:udp?endpoint=10.0.0.2:20121", 1001)

④ Replay (Archive → Subscriber):
   Client 调用:   archive.start_replay(recording_id, position, length,
                    "aeron:udp?endpoint=10.0.0.3:30001", 2001)
   Client   Sub = "aeron:udp?endpoint=10.0.0.3:30001" stream_id=2001
```

### 7.3 三种架构模式

#### 架构 A：Archive 中转站

Publisher 只发给 Archive，Subscriber 只从 Archive 获取数据。

```
Publisher ──③──> Archive ──④──> Subscriber
                   │
                 磁盘录制
```

| 优点 | 缺点 |
|---|---|
| 架构最简单 | 多一跳延迟 |
| Archive 是唯一数据源 | Archive 成为单点 |
| 故障恢复逻辑简单 | 吞吐量受限于 Archive |

**适用场景**：内网简单部署、对延迟要求不极端的场景。

#### 架构 B：Multicast + Archive 旁路录制

Publisher 用 multicast，Archive 和 Subscriber 都直接从网络接收。

```
Publisher ── multicast 224.0.1.1:40456 ──┬──> Subscriber（实时）
                                         └──> Archive（录制）
                                                │
                                                └──④──> Subscriber（回放）
```

```
Publisher:
  channel = "aeron:udp?endpoint=224.0.1.1:40456|interface=eth0"

Subscriber (live):
  channel = "aeron:udp?endpoint=224.0.1.1:40456|interface=eth0"

Archive (recording):
  archive.start_recording("aeron:udp?endpoint=224.0.1.1:40456|interface=eth0", 1001)
```

| 优点 | 缺点 |
|---|---|
| 实时数据零额外延迟 | 需要网络支持组播（IGMP） |
| Archive 不在关键路径 | 公有云通常不支持 |
| 带宽最优（一份数据） | |

**适用场景**：数据中心内部、金融行情分发。

#### 架构 C：MDC + Archive

用 Aeron MDC，Publisher 一份数据发给 Archive 和 Subscriber。

```
Publisher ──MDC──┬──> Subscriber（实时）
                 └──> Archive（录制）
                        │
                        └──④──> Subscriber（回放）
```

```
Publisher:
  channel = "aeron:udp?control=10.0.0.1:20200|control-mode=dynamic"

Subscriber (live):
  channel = "aeron:udp?control=10.0.0.1:20200|control-mode=dynamic|endpoint=10.0.0.3:20121"

Archive (recording):
  channel = "aeron:udp?control=10.0.0.1:20200|control-mode=dynamic|endpoint=10.0.0.2:20121"
```

| 优点 | 缺点 |
|---|---|
| 不需要组播网络 | Publisher 端 driver 负责复制 |
| Subscriber 可动态加入/退出 | CPU 略高于 multicast |
| 云环境兼容 | |

**适用场景**：云环境、跨数据中心。

### 7.4 架构选型对比

| | 架构 A（中转） | 架构 B（Multicast） | 架构 C（MDC） |
|---|---|---|---|
| 实时延迟 | 多一跳 | 最低 | 接近最低 |
| Archive 在关键路径 | 是 | 否 | 否 |
| 网络要求 | 普通 UDP | IGMP | 普通 UDP |
| 云环境 | 兼容 | 通常不兼容 | 兼容 |
| 动态扩缩 | 不支持 | IGMP join/leave | 支持 |

---

## 8. 故障恢复

### 8.1 连接断开与自动重连

Aeron 的 Publication 和 Subscription 都是长生命周期对象，**原生支持断线自动恢复**，无需重建。

**Subscription 端**：

```
t0  创建 Subscription          → 等待中, poll() 返回 0
t1  Publisher 上线             → Image 出现, available_image 回调
t2  正常收消息                  → poll() 返回数据
t3  Publisher 挂了             → Image 消失, unavailable_image 回调
t4  等待中                     → poll() 返回 0, Subscription 仍然存活
t5  Publisher 重连             → 新 Image 出现, 自动恢复收消息
```

**Publication 端**：

```
t0  创建 Publication           → is_connected() = false
t1  Subscriber 上线            → is_connected() = true
t2  正常发消息                  → offer() 返回正值
t3  Subscriber 挂了            → is_connected() = false
t4  offer() 返回 NOT_CONNECTED → 等待或缓存
t5  Subscriber 重连            → is_connected() = true, 恢复正常
```

**断线检测超时参数**：

| 参数 | 默认值 | 说明 |
|---|---|---|
| `client_liveness_timeout` | 10s | Driver 判定 client 死亡 |
| `image_liveness_timeout` | 10s | Subscriber 判定 Publisher 断开 |
| `publication_linger_timeout` | 5s | Publication 关闭后 buffer 保留时间 |

### 8.2 Archive 故障恢复：Subscriber 崩溃

**Archive 与 Subscriber 同进程**：

```
Publisher → [Subscription + Archive 录制 + 业务处理]  ← 一起崩溃
                                                       丢失未 flush 的尾部数据
```

恢复后每个 Recording 从最后落盘的 position 回放，已录制部分顺序正确。

**Archive 独立部署（推荐）**：

```
Publisher → [Archive 录制]   ← 不受业务崩溃影响，持续录制
              ↓ replay
           [Subscriber]     ← 崩溃后从 Archive 回放补数据
```

零丢失，Archive 持续运行。

**恢复流程**：

```
Subscriber 正常运行:
  1. 消费 live 数据
  2. 定期记录 last_consumed_position

Subscriber 崩溃后重启:
  3. 读取 last_consumed_position
  4. 请求 Archive replay：
     archive.start_replay(recording_id, last_position, LENGTH_MAX,
                          replay_channel, replay_stream_id)
  5. 消费 replay 数据，追赶到最新
  6. 切换回 live 数据

     time ────────────────────────────>
     live:    ▓▓▓▓▓▓▓▓░░░░░░▓▓▓▓▓▓▓▓▓
     replay:           ██████
                       ↑     ↑
                  last_pos  追上 live
```

Aeron Archive 提供 **Replaying Merger** 工具类，可自动合并 replay 流和 live 流，实现无缝切换。

### 8.3 N:1 场景的故障恢复

N 个 Publisher 对 1 个 Subscriber，Archive 录制的是 N 个独立 Recording。

恢复时：
- 每个 Recording 单独回放，**各自内部顺序完全保证**
- 跨 Recording 的交错顺序 **无法恢复**（原始交错顺序未被记录）

如需全局有序恢复，需要 **Sequencer 模式**：

```
Publisher A ──┐                           ┌──> Subscriber X
Publisher B ──┼──> Sequencer ────────────┼──> Subscriber Y
Publisher C ──┘   (单 session 编号)       └──> Archive
                  全局唯一 position            单个 Recording
                                              故障后回放顺序完全一致
```

---

## 9. 生产环境配置建议

### 9.1 低延迟配置

```
// Channel 参数
aeron:udp?endpoint=10.0.0.5:20121|sparse=false|term-length=8388608|mtu=8192

// OS 层面
- CPU 绑核（isolcpus + taskset），避免 Media Driver 被调度走
- 关闭 C-States 和 Turbo Boost，降低频率跳变
- 使用 huge pages（减少 TLB miss）
- 网卡中断绑核，避免跨 NUMA 访问
```

### 9.2 高吞吐配置

```
// Channel 参数
aeron:udp?endpoint=10.0.0.5:20121|mtu=8192|term-length=33554432

// 增大 term-length 允许更大的 burst
// 增大 mtu 减少大消息的分片
// 确保 SO_SNDBUF / SO_RCVBUF 足够大
```

### 9.3 Publication 发送的健壮写法

```rust
fn offer_resilient(
    publication: &AeronPublication,
    buf: &[u8],
    timeout: Duration,
) -> Result<i64, &'static str> {
    let deadline = Instant::now() + timeout;
    loop {
        let result = publication.offer(buf, None);
        match result {
            r if r > 0 => return Ok(r),           // 成功
            -1 => {}                                // BACK_PRESSURED，重试
            -2 => {                                 // NOT_CONNECTED
                if Instant::now() >= deadline {
                    return Err("not connected");
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            -3 => {}                                // ADMIN_ACTION，重试
            _ => return Err("publication closed"),  // CLOSED / MAX_POSITION
        }
        if Instant::now() >= deadline {
            return Err("offer timeout");
        }
        std::hint::spin_loop();
    }
}
```

### 9.4 防火墙端口规划

| 用途 | 默认端口 | 协议 |
|---|---|---|
| 业务数据 | 20121 | UDP |
| 回复通道 | 20122 | UDP |
| Archive 控制 | 8010 | UDP |
| Archive 响应 | 8020 | UDP |
| Replay 数据 | 30001+ | UDP |
| MDC 控制 | 20200 | UDP |

> 端口号可自定义，以上为建议值。每种用途的端口需在防火墙上开放 UDP 入站。

---

## 附录：术语表

| 术语 | 说明 |
|---|---|
| **Media Driver** | Aeron 传输引擎，管理 UDP/IPC 收发 |
| **Publication** | 发送端抽象，绑定 Channel + Stream ID |
| **Subscription** | 接收端抽象，订阅 Channel + Stream ID |
| **Image** | Subscriber 端对单个 Publisher session 的连接表示 |
| **Channel** | 传输通道 URI（协议 + 地址 + 参数） |
| **Stream ID** | 同一 Channel 上的逻辑数据流标识 |
| **Session ID** | 单个 Publication 实例的唯一标识 |
| **Fragment** | Aeron 最小传输单元 |
| **Term** | Log buffer 的分段，3 个 term 轮转使用 |
| **NAK** | 否定应答，Subscriber 检测丢包后请求重传 |
| **MDC** | Multi-Destination-Cast，应用层一对多发送 |
| **Recording** | Archive 中单个 session 的持久化数据 |
| **Replay** | 从 Archive 读取 Recording 并回放 |
| **Sequencer** | 将多个输入流合并为单一全局有序流的节点 |
