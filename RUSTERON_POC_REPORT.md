# Rusteron UDP Benchmark POC 报告

## 目录

1. [项目概述](#1-项目概述)
2. [环境搭建](#2-环境搭建)
3. [架构设计](#3-架构设计)
4. [基准测试结果](#4-基准测试结果)
5. [多客户端支持](#5-多客户端支持)
6. [Aeron Transport 接口详解](#6-aeron-transport-接口详解)
7. [Aeron Archive 接口详解](#7-aeron-archive-接口详解)
8. [交易系统应用场景](#8-交易系统应用场景)
9. [生产部署建议](#9-生产部署建议)

---

## 1. 项目概述

本项目是一个基于 [Aeron](https://github.com/real-logic/aeron)（通过 [rusteron](https://crates.io/crates/rusteron-client) Rust 绑定）的 UDP 基准测试工具，用于评估 Aeron 在低延迟金融交易系统中的通信性能。

### 技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Rust |
| 传输层 | Aeron（C 库 + Rust FFI 绑定） |
| Rust 绑定 | `rusteron-client` 0.1.162、`rusteron-media-driver` 0.1.162 |
| CLI | clap 4 |
| 延迟统计 | hdrhistogram 7 |
| 构建 | Cargo + CMake（Aeron C 库编译） |

### 运行模式

- **server** — 监听并回复消息，支持多客户端
- **client** — 执行延迟（Ping-Pong）+ 吞吐量（单向发送）测试
- **bench** — 单进程内同时运行 server 和 client

---

## 2. 环境搭建

### AWS 基础设施

| 角色 | 实例类型 | 内网 IP | 公网 IP |
|------|---------|---------|---------|
| Server | t3.micro | 172.31.33.118 | 16.171.193.205 |
| Client 1 | t3.micro | 172.31.36.93 | 13.60.248.132 |
| Client 2 | t3.micro | 172.31.38.162 | 13.49.44.88 |

- **区域**: eu-north-1
- **VPC**: 同一 VPC，通过内网通信
- **安全组**: 开放 TCP 22（SSH）、UDP 20121-20122

### 编译依赖安装

```bash
# 系统依赖
sudo apt update && sudo apt install -y build-essential pkg-config uuid-dev libbsd-dev clang libclang-dev g++
sudo snap install cmake --classic
export PATH=/snap/bin:$PATH

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# 编译（需要使用 bfd 链接器解决 rust-lld 静态库链接问题）
git clone https://github.com/hongtao023/poc_rust_rusteron.git
cd poc_rust_rusteron
cargo rustc --release --bin rusteron-bench -- -C link-arg=-fuse-ld=bfd
```

### Linux 编译注意事项

Rust 默认使用 `rust-lld` 链接器，在 `-Bdynamic` 模式下无法找到 `.a` 静态库文件。解决方案是使用 GNU ld（bfd）作为链接器：

```bash
cargo rustc --release --bin rusteron-bench -- -C link-arg=-fuse-ld=bfd
```

---

## 3. 架构设计

### 通信模型

使用两个独立的 UDP 单向通道：

```
Client ──[endpoint:20121, stream 1001]──► Server    (发送 Ping/Data/Control)
Client ◄──[reply_endpoint:20122, stream 1002]── Server  (接收 Pong/Report)
```

### 消息协议

固定 64 字节定长消息（小端序）：

| 偏移 | 长度 | 字段 | 说明 |
|------|------|------|------|
| 0 | 1 | msg_type | Ping(1)/Pong(2)/Data(3)/Control(4) |
| 1 | 1 | control_code | StartThroughput/StopThroughput/ReportRequest/ReportResponse |
| 2-3 | 2 | padding | 对齐填充 |
| 4-7 | 4 | sequence | 序列号（u32 LE） |
| 8-15 | 8 | timestamp_ns | 时间戳纳秒（u64 LE） |
| 16-63 | 48 | payload | 载荷数据 |

### 测试流程

**Phase 1 — 延迟测试（Ping-Pong）：**
1. Client 发送 Ping（带序列号）
2. Server 收到后立即回复 Pong（保留相同序列号和时间戳）
3. Client 计算 RTT，记录到 HDR Histogram
4. 前 10,000 条为预热，不计入统计

**Phase 2 — 吞吐量测试（单向发送）：**
1. Client 发送 StartThroughput 控制指令
2. Client 在指定时间内（默认 10s）持续发送 Data 消息
3. Client 发送 StopThroughput
4. Client 发送 ReportRequest，Server 返回实际收到的消息总数
5. 根据 Server 报告的数量和时间计算吞吐量

---

## 4. 基准测试结果

### 单 Client 测试（1v1）

```
Server: ./target/release/rusteron-bench --mode server --endpoint 0.0.0.0:20121 \
        --reply-endpoint 172.31.36.93:20122
Client: ./target/release/rusteron-bench --mode client --endpoint 172.31.33.118:20121 \
        --reply-endpoint 172.31.36.93:20122
```

| 指标 | 结果 |
|------|------|
| P50 延迟 | 126 µs |
| P95 延迟 | 214 µs |
| P99 延迟 | 2,719 µs |
| Min/Max 延迟 | 86 / 4,883 µs |
| 吞吐量 | 1,009,760 msgs/sec |
| 带宽 | 61.6 MB/sec |

### 双 Client 测试（2v1）

```
Server: ./target/release/rusteron-bench --mode server --endpoint 0.0.0.0:20121 \
        --reply-endpoints 172.31.36.93:20122,172.31.38.162:20122
Client1: --stream-id-send 1001 --stream-id-recv 1002
Client2: --stream-id-send 2001 --stream-id-recv 2002
```

| 指标 | 单 Client | Client 1 (双) | Client 2 (双) | 双 Client 合计 |
|------|-----------|---------------|---------------|----------------|
| P50 延迟 | 126 µs | 129 µs (+2%) | 134 µs (+6%) | — |
| P95 延迟 | 214 µs | 198 µs (-7%) | 471 µs (+120%) | — |
| P99 延迟 | 2,719 µs | 291 µs (-89%) | 1,123 µs (-59%) | — |
| Max 延迟 | 4,883 µs | 7,231 µs | 52,927 µs | — |
| msgs/sec | 1,009,760 | 889,661 (-12%) | 737,099 (-27%) | **1,626,760 (+61%)** |
| MB/sec | 61.6 | 54.3 | 45.0 | **99.3 (+61%)** |

### 结论

1. **P50 延迟几乎无影响** — 独立 stream ID 有效隔离，中位延迟只增 2~6%
2. **尾部延迟分化明显** — Client 1（轮询靠前）P99 更好；Client 2 的 Max 达到 53ms
3. **单 client 吞吐下降 12~27%** — t3.micro 的 2 vCPU 需服务 4 个 Aeron 通道
4. **总吞吐提升 61%** — 合计 163 万 msgs/sec，接近 t3.micro 网络带宽上限
5. **瓶颈在 server CPU** — 升级到 c5.xlarge 预计可线性扩展

---

## 5. 多客户端支持

### Stream ID 分配策略

使用 `--reply-endpoints` 指定多个客户端，自动分配 stream ID：

| 客户端 | stream_id_send (C→S) | stream_id_recv (S→C) |
|--------|----------------------|----------------------|
| Client 1 | 1001 | 1002 |
| Client 2 | 2001 | 2002 |
| Client N | N×1000+1 | N×1000+2 |

### 命令行示例

```bash
# Server（多客户端模式）
./rusteron-bench --mode server --endpoint 0.0.0.0:20121 \
  --reply-endpoints 172.31.36.93:20122,172.31.38.162:20122

# Client 1
./rusteron-bench --mode client --endpoint SERVER_IP:20121 \
  --reply-endpoint CLIENT1_IP:20122 --stream-id-send 1001 --stream-id-recv 1002

# Client 2
./rusteron-bench --mode client --endpoint SERVER_IP:20121 \
  --reply-endpoint CLIENT2_IP:20122 --stream-id-send 2001 --stream-id-recv 2002
```

---

## 6. Aeron Transport 接口详解

Aeron 通过 `rusteron-client` 和 `rusteron-media-driver` 提供 Rust 接口。

### 6.1 Media Driver（`rusteron-media-driver`）

Media Driver 是 Aeron 的核心组件，负责管理 UDP 传输和共享内存通信。

#### AeronDriverContext

驱动上下文，用于配置 Media Driver。

```rust
// 创建上下文
let ctx = AeronDriverContext::new()?;

// 设置共享内存目录
ctx.set_dir(&CString::new("/tmp/aeron-driver")?)?;

// 其他可配置项：
// - 发送/接收缓冲区大小
// - 术语缓冲区长度
// - 线程空闲策略
```

#### AeronDriver

Media Driver 实例，管理所有 UDP 传输。

```rust
// 在独立线程中启动嵌入式 Driver
// 返回 (stop_flag, thread_handle)
let (stop, handle) = AeronDriver::launch_embedded(ctx, false);

// 停止 Driver
stop.store(true, Ordering::SeqCst);
handle.join()?;
```

| 方法 | 说明 |
|------|------|
| `launch_embedded(ctx, delete_on_start)` | 在新线程启动 Driver，返回停止标志和线程句柄 |
| `new(ctx)` | 创建 Driver 实例（不自动启动） |
| `start()` | 手动启动 Driver |
| `main_do_work()` | 执行一次 Driver 工作循环（手动驱动模式） |
| `main_idle_strategy(work_count)` | 空闲策略（无工作时降低 CPU） |

### 6.2 Aeron 客户端（`rusteron-client`）

#### AeronContext

客户端上下文，用于配置 Aeron 客户端连接。

```rust
let ctx = AeronContext::new()?;
ctx.set_dir(&CString::new("/tmp/aeron-driver")?)?;
// 设置与 Media Driver 相同的目录，通过共享内存通信
```

| 方法 | 说明 |
|------|------|
| `new()` | 创建默认上下文 |
| `set_dir(dir)` | 设置 Aeron Driver 的共享内存目录 |
| `set_client_name(name)` | 设置客户端名称（调试用） |
| `set_error_handler(handler)` | 设置错误处理回调 |

#### Aeron

Aeron 客户端主接口，用于创建 Publication 和 Subscription。

```rust
let aeron = Aeron::new(&ctx)?;
aeron.start()?;

// 创建发布端
let pub_reg = aeron.add_publication(&channel, stream_id, timeout)?;

// 创建订阅端
let sub_reg = aeron.add_subscription::<AvailHandler, UnavailHandler>(
    &channel, stream_id, None, None, timeout
)?;
```

| 方法 | 说明 |
|------|------|
| `new(ctx)` | 创建 Aeron 客户端实例 |
| `start()` | 启动客户端，连接到 Media Driver |
| `add_publication(channel, stream_id, timeout)` | 创建 Publication（发布端） |
| `add_exclusive_publication(channel, stream_id, timeout)` | 创建独占 Publication |
| `add_subscription(channel, stream_id, on_available, on_unavailable, timeout)` | 创建 Subscription（订阅端） |
| `is_closed()` | 检查客户端是否已关闭 |
| `close()` | 关闭客户端 |

#### AeronPublication

消息发布端，用于向指定 channel + stream 发送消息。

```rust
let channel = CString::new("aeron:udp?endpoint=10.0.0.1:20121")?;
let publication = aeron.add_publication(&channel, 1001, timeout)?;

// 发送消息
let result = publication.offer::<ReservedValueSupplier>(&buffer, None);
// result > 0: 成功，返回新的 stream position
// result == BACK_PRESSURED: 背压，需重试
// result == NOT_CONNECTED: 未连接
// result == CLOSED: 已关闭
```

| 方法 | 返回值 | 说明 |
|------|--------|------|
| `offer(buffer, reserved_value_supplier)` | `i64` | 发送消息，返回 position 或错误码 |
| `try_claim(length)` | `BufferClaim` | 零拷贝发送：先预留空间再写入 |
| `is_connected()` | `bool` | 对端 Subscription 是否已连接 |
| `is_closed()` | `bool` | Publication 是否已关闭 |
| `channel()` | `&str` | 获取 channel URI |
| `stream_id()` | `i32` | 获取 stream ID |
| `session_id()` | `i32` | 获取会话 ID |
| `position()` | `i64` | 当前发送位置 |
| `position_limit()` | `i64` | 发送位置上限（背压边界） |
| `constants()` | `PublicationConstants` | 获取发布参数常量 |
| `close()` | `Result` | 关闭 Publication |

**offer() 返回值含义：**

| 值 | 含义 | 处理方式 |
|---|---|---|
| `> 0` | 成功，值为新的 stream position | 继续 |
| `-1` (NOT_CONNECTED) | 无 Subscriber 连接 | 等待连接 |
| `-2` (BACK_PRESSURED) | 发送缓冲区满 | 自旋重试或退避 |
| `-3` (ADMIN_ACTION) | 管理操作中 | 重试 |
| `-4` (CLOSED) | Publication 已关闭 | 停止发送 |
| `-5` (MAX_POSITION_EXCEEDED) | 超过最大位置 | 需要新的 Publication |

#### AeronSubscription

消息订阅端，用于从指定 channel + stream 接收消息。

```rust
let channel = CString::new("aeron:udp?endpoint=0.0.0.0:20121")?;
let subscription = aeron.add_subscription::<AvailLogger, UnavailLogger>(
    &channel, 1001, None, None, timeout
)?;

// 轮询消息
let fragments_read = subscription.poll(Some(&handler), 10)?;
// fragments_read: 本次 poll 读取到的消息数
```

| 方法 | 返回值 | 说明 |
|------|--------|------|
| `poll(handler, fragment_limit)` | `i32` | 轮询消息，最多处理 fragment_limit 条 |
| `is_connected()` | `bool` | 是否有 Publication 连接 |
| `is_closed()` | `bool` | Subscription 是否已关闭 |
| `channel()` | `&str` | 获取 channel URI |
| `stream_id()` | `i32` | 获取 stream ID |
| `constants()` | `SubscriptionConstants` | 获取订阅参数常量 |
| `image_count()` | `i32` | 连接的 Image 数量 |
| `close()` | `Result` | 关闭 Subscription |

#### AeronFragmentHandlerCallback

消息处理回调 trait，每次 `poll()` 拉取到消息时调用。

```rust
impl AeronFragmentHandlerCallback for MyHandler {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], header: AeronHeader) {
        // buffer: 消息内容
        // header: 消息元数据（position、session_id、flags 等）
        let position = header.position();   // 消息在 stream 中的位置
        let session_id = header.session_id(); // 发送方会话 ID
    }
}

// 使用 Handler::leak 将 handler 移到堆上（C FFI 需要稳定指针）
let handler = Handler::leak(MyHandler { ... });
subscription.poll(Some(&handler), 10);
```

#### Aeron URI 格式

```
aeron:udp?endpoint=host:port          # UDP 单播
aeron:udp?endpoint=224.0.0.1:port     # UDP 多播
aeron:ipc                              # 进程内通信（共享内存）
```

| 参数 | 说明 |
|------|------|
| `endpoint` | UDP 目标/监听地址 |
| `interface` | 绑定的网络接口 |
| `ttl` | 多播 TTL |
| `control` | MDC（Multi-Destination-Cast）控制地址 |
| `term-length` | 术语缓冲区长度 |
| `mtu` | 最大传输单元 |

---

## 7. Aeron Archive 接口详解

Aeron Archive（`rusteron-archive` 0.1.162）在标准 pub/sub 基础上增加消息持久化功能。

### 7.1 核心概念

| 概念 | 说明 |
|------|------|
| **Recording** | 一个 channel+stream 的消息录制实例，有唯一 recording_id |
| **Position** | 字节偏移量（类似 Kafka offset），用于定位消息 |
| **Segment** | 录制数据按固定大小（默认 128MB）分文件存储 |
| **Replay** | 将录制的消息重新发送到一个 Subscription |
| **Catalog** | 所有录制的索引文件 |

### 7.2 部署模式

| 模式 | 说明 | 适用场景 |
|------|------|---------|
| 嵌入式 | Archive 运行在业务进程内 | 开发测试 |
| 独立 Server | Archive 作为独立进程运行 | **生产推荐** |
| 集群 | 主备 Archive，自动复制 | 高可用 |

### 7.3 AeronArchiveContext

Archive 连接上下文配置。

```rust
let control_req = CString::new("aeron:udp?endpoint=localhost:8010")?;
let control_resp = CString::new("aeron:udp?endpoint=localhost:8011")?;
let recording_events = CString::new("aeron:udp?endpoint=localhost:8012")?;

let ctx = AeronArchiveContext::new_with_no_credentials_supplier(
    &aeron,
    &control_req,      // Archive 控制请求通道
    &control_resp,     // Archive 控制响应通道
    &recording_events, // 录制事件通知通道
)?;
```

| 通道 | 说明 |
|------|------|
| control_request | 客户端 → Archive 的控制指令通道 |
| control_response | Archive → 客户端 的响应通道 |
| recording_events | Archive 录制状态变更通知 |

### 7.4 AeronArchiveAsyncConnect

异步连接到 Archive Server。

```rust
let connect = AeronArchiveAsyncConnect::new_with_aeron(&ctx, &aeron)?;

// 阻塞等待连接完成
let archive = connect.poll_blocking(Duration::from_secs(10))?;

// 或非阻塞轮询
loop {
    match connect.poll() {
        Ok(Some(archive)) => break archive,
        Ok(None) => continue,  // 未完成，继续轮询
        Err(e) => return Err(e),
    }
}
```

### 7.5 AeronArchive — 录制操作

#### start_recording — 开始录制

```rust
let channel = CString::new("aeron:udp?endpoint=0.0.0.0:20121")?;
let subscription_id = archive.start_recording(
    &channel,
    stream_id,           // 要录制的 stream ID
    source_location,     // LOCAL(0): 同机器 | REMOTE(1): 来自网络
    auto_stop,           // true: 发布者断开后自动停止录制
)?;
```

| 参数 | 说明 |
|------|------|
| `channel` | 要录制的 Aeron channel URI |
| `stream_id` | 要录制的 stream ID |
| `source_location` | `LOCAL`(0) = 同机 / `REMOTE`(1) = 来自网络 |
| `auto_stop` | 发布者断开时是否自动停止 |

#### stop_recording — 停止录制

```rust
// 按 subscription ID 停止
archive.stop_recording_subscription(subscription_id)?;

// 按 channel + stream 停止
archive.stop_recording_channel_and_stream(&channel, stream_id)?;
```

#### extend_recording — 扩展已有录制

```rust
// 在已有录制上继续追加（而非创建新录制）
archive.extend_recording(
    recording_id,
    &channel,
    stream_id,
    source_location,
    auto_stop,
)?;
```

#### 录制位置查询

```rust
// 当前录制写入位置（实时）
let position = archive.get_recording_position(recording_id)?;

// 录制起始位置
let start = archive.get_start_position(recording_id)?;

// 录制停止位置（仍在录制中返回 ACTIVE）
let stop = archive.get_stop_position(recording_id)?;
```

### 7.6 AeronArchive — 回放操作

#### start_replay — 开始回放

```rust
let replay_channel = CString::new("aeron:udp?endpoint=localhost:20199")?;
let params = AeronArchiveReplayParams::new(
    start_position,   // 从哪个 position 开始回放
    replay_length,    // 回放长度（字节），i32::MAX 表示全部
    bounding_limit_counter_id,
    file_io_max_length,
    replay_token,
    subscription_registration_id,
)?;

let replay_session_id = archive.start_replay(
    recording_id,       // 要回放的录制 ID
    &replay_channel,    // 回放输出的 channel
    replay_stream_id,   // 回放输出的 stream ID
    &params,
)?;

// 通过标准 Subscription 消费回放的消息
let replay_sub = aeron.add_subscription(&replay_channel, replay_stream_id, ...)?;
while replay_sub.poll(Some(&handler), 100) > 0 {
    // 处理每条回放的消息
}
```

#### stop_replay — 停止回放

```rust
archive.stop_replay(replay_session_id)?;

// 停止某个录制的所有回放
archive.stop_all_replays(recording_id)?;
```

#### AeronArchiveReplayParams

| 参数 | 说明 |
|------|------|
| `position` | 起始 position（字节偏移），0 = 从头开始 |
| `length` | 回放长度，`i32::MAX` = 到最新位置 |
| `bounding_limit_counter_id` | 限速 counter ID（0 = 不限速） |
| `file_io_max_length` | 文件 IO 最大读取长度 |
| `replay_token` | 回放令牌（安全验证用） |

### 7.7 AeronArchive — 查询操作

#### list_recordings — 列出录制

```rust
// 列出所有录制（从 ID=0 开始，最多 100 条）
let count = archive.list_recordings(0, 100, |descriptor| {
    println!("Recording ID: {}", descriptor.recording_id());
    println!("  Channel:    {}", descriptor.channel());
    println!("  Stream ID:  {}", descriptor.stream_id());
    println!("  Start Pos:  {}", descriptor.start_position());
    println!("  Stop Pos:   {}", descriptor.stop_position());
    println!("  Start Time: {}", descriptor.start_timestamp());
    println!("  Stop Time:  {}", descriptor.stop_timestamp());
})?;
```

#### list_recordings_for_uri — 按 URI 过滤

```rust
// 只列出匹配指定 channel 和 stream 的录制
let count = archive.list_recordings_for_uri(
    0,              // 起始 recording_id
    100,            // 最大返回数
    &channel,       // channel URI 过滤
    stream_id,      // stream ID 过滤
    |descriptor| {
        // 处理每条匹配的录制
    },
)?;
```

#### find_last_matching_recording — 找最新匹配录制

```rust
let recording_id = archive.find_last_matching_recording(
    min_recording_id,  // 最小 recording_id
    &channel,
    stream_id,
    session_id,
)?;
```

#### list_recording_subscriptions — 列出活跃录制

```rust
// 查看当前正在录制的 subscription
let count = archive.list_recording_subscriptions(
    pseudo_index,
    subscription_count,
    apply_stream_id,
    stream_id,
    &channel_fragment,
    |subscription| {
        // 处理每个活跃录制
    },
)?;
```

### 7.8 AeronArchive — Segment 管理

录制数据按 segment 文件存储（默认 128MB/文件）：

```
/var/aeron/archive/
  ├── recording-0-0.dat        ← segment 0
  ├── recording-0-1.dat        ← segment 1
  ├── recording-0-2.dat        ← segment 2（最新）
  └── catalog.dat              ← 索引
```

#### detach_segments — 分离旧 segment

```rust
// 将 new_start_position 之前的 segment 从 Archive 管理中分离
// 文件仍在磁盘上，但 Archive 不再管理它们
archive.detach_segments(recording_id, new_start_position)?;

// 分离后可以手动移到 S3 或其他慢存储
// aws s3 cp recording-0-0.dat s3://bucket/archive/
```

#### attach_segments — 重新附加 segment

```rust
// 从 S3 下载回来后，重新附加到 Archive
archive.attach_segments(recording_id)?;
```

#### purge_segments — 清理 segment

```rust
// 清理 new_start_position 之前的 segment（删除文件）
archive.purge_segments(recording_id, new_start_position)?;
```

#### truncate_recording — 截断录制

```rust
// 从指定 position 截断录制尾部
archive.truncate_recording(recording_id, position)?;
```

### 7.9 AeronArchive — 复制操作

#### replicate — 跨 Archive 复制

```rust
// 将录制从另一个 Archive Server 复制到本地
archive.replicate(
    src_recording_id,      // 源 recording_id
    dst_recording_id,      // 目标 recording_id（-1 = 新建）
    stop_position,         // 复制到哪个位置
    src_control_channel,   // 源 Archive 的控制通道
    src_control_stream_id, // 源 Archive 的控制 stream
    &live_destination,     // 实时流目标（用于 replay merge）
    &replication_channel,  // 复制数据传输通道
)?;

// 停止复制
archive.stop_replication(replication_id)?;
```

### 7.10 RecordingPos — 辅助工具

用于通过 Aeron counters 查找录制信息。

```rust
// 通过 session_id 找到对应的 counter ID
let counter_id = RecordingPos::find_counter_id_by_session(
    &counters_reader, session_id
);

// 从 counter 获取 recording_id
let recording_id = RecordingPos::get_recording_id(
    &counters_reader, counter_id
)?;
```

### 7.11 存储分层策略

| 层级 | 存储 | 数据范围 | 用途 |
|------|------|---------|------|
| 热 | NVMe SSD | 最近 1-2 小时 | 实时录制 + 快速恢复 |
| 温 | HDD / EBS st1 | 1 小时 ~ 7 天 | 故障排查、审计 |
| 冷 | S3 / Glacier | 7 天 ~ 数年 | 合规归档 |

### 7.12 Checkpoint 设计（从上次失败处恢复）

Archive 使用 position（字节偏移）作为消息定位，类似 Kafka 的 offset。需要自行管理 checkpoint：

```rust
struct Checkpoint {
    recording_id: i64,
    position: i64,       // 已处理到的位置
    msg_count: u64,
}

// 处理消息时：从 header.position() 获取当前 position
// 每 N 条消息保存 checkpoint
// 恢复时：start_replay(recording_id, checkpoint.position, ...)
```

| Checkpoint 间隔 | 恢复重放量 | IO 开销 |
|-----------------|-----------|---------|
| 每条消息 | 最多 1 条 | 高 |
| **每 1000 条（推荐）** | **最多 1000 条** | **低** |
| 每 10000 条 | 最多 10000 条 | 极低 |

---

## 8. 交易系统应用场景

### 推荐架构

```
                  Aeron UDP (单播)              Aeron UDP (单播)
  Client ──────────────────► Gateway ──────────────────► 撮合引擎
                                                           │
                                 Archive Server ◄──────────┤ (自动录制)
                                                           │
                       Aeron UDP (多播)                     │
  Market Data ◄────────────────────────────────────────────┤
                                                           │
                       Aeron UDP (单播)                     │
  Settlement  ◄────────────────────────────────────────────┘
```

### 路径适配

| 通信路径 | 传输模式 | Aeron 特性 |
|---------|---------|-----------|
| Gateway → 撮合 | UDP 单播，低延迟 | `aeron:udp?endpoint=...` |
| 撮合 → 结算 | UDP 单播，可靠传输 | `aeron:udp?endpoint=...` + Archive |
| 撮合 → 行情 | **UDP 多播，1对多** | `aeron:udp?endpoint=224.x.x.x:port` |
| 故障恢复 | Archive 回放 | `archive.start_replay()` |

### 对比其他方案

| 方案 | 延迟 | 适用场景 |
|------|------|---------|
| **Aeron** | 1-10 µs (专用硬件) | 金融交易系统（本项目选择） |
| gRPC/TCP | 50-200 µs | 延迟不敏感的微服务 |
| DPDK/kernel bypass | < 1 µs | 极致 HFT |
| Kafka | ms 级 | 日志/审计/非关键路径 |

---

## 9. 生产部署建议

### 硬件

| 组件 | 推荐实例 | 原因 |
|------|---------|------|
| Gateway | c5n.xlarge | 增强网络，低延迟 |
| 撮合引擎 | c5.2xlarge / bare metal | 专用 CPU，确定性延迟 |
| Archive Server | i3.xlarge | 大容量 NVMe SSD |

### OS 调优

| 项目 | 设置 |
|------|------|
| CPU 隔离 | `isolcpus=2,3` 隔离核心给 Aeron |
| 超线程 | 关闭（减少延迟抖动） |
| Huge Pages | 启用 2MB huge pages |
| 网卡中断 | 绑定到指定 CPU 核心 |
| Aeron Driver | 独立进程，绑核运行 |

### 消息协议升级

当前 POC 使用自定义 64 字节定长协议。生产建议使用 [SBE (Simple Binary Encoding)](https://github.com/real-logic/simple-binary-encoding)：
- 零拷贝序列化
- 与 Aeron 同生态
- 金融行业标准（FIX/SBE）

### 监控

| 指标 | 来源 |
|------|------|
| 消息延迟 | Aeron counters / HDR Histogram |
| 吞吐量 | Aeron counters |
| 背压事件 | Publication offer 返回值监控 |
| Archive 磁盘 | 磁盘使用率告警 |
| Recording 状态 | `archive.list_recordings()` 定期检查 |

---

*本文档基于 2026-03-22 的 POC 测试结果和研究整理。*
