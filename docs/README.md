# Rusteron UDP Benchmark

基于 [Aeron](https://github.com/real-logic/aeron) 的超低延迟 UDP 基准测试工具，使用 [rusteron](https://crates.io/crates/rusteron-client) Rust 绑定。适用于评估金融交易系统（如撮合引擎、行情分发）的通信性能。

## 功能

- **Ping-Pong 延迟测试** — 测量 RTT，输出 min / avg / p50 / p95 / p99 / max
- **单向吞吐量测试** — 测量 msgs/sec 和 MB/sec
- **多客户端支持** — 每个客户端使用独立 stream ID 对，互不干扰
- **双协议实现** — 手写协议 + SBE（Simple Binary Encoding）对比
- 双通道 UDP（发送 / 回复各一个 endpoint）
- 内嵌 Aeron Media Driver，无需单独启动

## 基准测试结果（AWS t3.micro, eu-north-1, 同 VPC）

### 单客户端

| 指标 | 结果 |
|------|------|
| P50 延迟 | ~121 µs |
| P95 延迟 | ~236 µs |
| P99 延迟 | ~2,665 µs |
| Min / Max | 81 / 5,123 µs |
| 吞吐量 | ~800K-1M msgs/sec |
| 带宽 | ~49-62 MB/sec |

### 双客户端（独立 stream ID）

| 指标 | Client 1 | Client 2 | 合计 |
|------|----------|----------|------|
| P50 延迟 | 129 µs | 134 µs | — |
| msgs/sec | 889,661 | 737,099 | **1,626,760** |
| MB/sec | 54.3 | 45.0 | **99.3** |

> P50 延迟几乎无影响（+2~6%），总吞吐量提升 61%。瓶颈在 t3.micro 的 CPU，升级实例可线性扩展。

## 构建

### macOS

```bash
cargo build --release
```

### Linux (Ubuntu)

```bash
# 安装依赖
sudo apt update && sudo apt install -y build-essential pkg-config uuid-dev libbsd-dev clang libclang-dev g++
sudo snap install cmake --classic
export PATH=/snap/bin:$PATH

# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# 编译（Linux 需要 bfd 链接器）
cargo rustc --release --bin rusteron-bench -- -C link-arg=-fuse-ld=bfd
```

## 快速开始

### 本地测试（单进程 bench 模式）

```bash
cargo run --release -- --mode bench
```

### 远程部署（Server + Client 分离）

**Server（监听端）：**

```bash
./target/release/rusteron-bench \
  --mode server \
  --endpoint 0.0.0.0:20121 \
  --reply-endpoint CLIENT_IP:20122
```

**Client（测试端）：**

```bash
./target/release/rusteron-bench \
  --mode client \
  --endpoint SERVER_IP:20121 \
  --reply-endpoint CLIENT_IP:20122
```

### 多客户端模式

**Server（指定多个 reply endpoint）：**

```bash
./target/release/rusteron-bench \
  --mode server \
  --endpoint 0.0.0.0:20121 \
  --reply-endpoints CLIENT1_IP:20122,CLIENT2_IP:20122
```

**Client 1（stream 1001/1002）：**

```bash
./target/release/rusteron-bench \
  --mode client \
  --endpoint SERVER_IP:20121 \
  --reply-endpoint CLIENT1_IP:20122 \
  --stream-id-send 1001 --stream-id-recv 1002
```

**Client 2（stream 2001/2002）：**

```bash
./target/release/rusteron-bench \
  --mode client \
  --endpoint SERVER_IP:20121 \
  --reply-endpoint CLIENT2_IP:20122 \
  --stream-id-send 2001 --stream-id-recv 2002
```

## 参数说明

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--mode` | (必填) | `server` / `client` / `bench` |
| `--endpoint` | `localhost:20121` | Client→Server 通道地址 |
| `--reply-endpoint` | `localhost:20122` | Server→Client 通道地址（单客户端） |
| `--reply-endpoints` | — | 逗号分隔的多个 reply endpoint（多客户端模式） |
| `--stream-id-send` | `1001` | Client→Server stream ID |
| `--stream-id-recv` | `1002` | Server→Client stream ID |
| `--ping-count` | `100,000` | 延迟测试消息数 |
| `--warmup` | `10,000` | 预热消息数（不计入统计） |
| `--duration` | `10` | 吞吐量测试持续秒数 |

## 通信架构

```
Client ──[endpoint:20121, stream 1001]──► Server    (Ping/Data/Control)
Client ◄──[reply_endpoint:20122, stream 1002]── Server  (Pong/Report)
```

## 协议

### 手写协议（`protocol.rs`）

固定 64 字节消息格式：

```
[0]      msg_type     (Ping/Pong/Data/Control)
[1]      control_code
[2-3]    padding
[4-7]    sequence     (u32 LE)
[8-15]   timestamp_ns (u64 LE)
[16-63]  payload      (48 bytes)
```

### SBE 协议（`protocol_sbe.rs`）

基于 XML Schema（`schemas/bench.xml`）自动生成的类型安全编解码器：

```
MessageHeader (8 bytes):
  [0-1]  blockLength   (u16 LE)
  [2-3]  templateId    (u16 LE) — 消息类型路由
  [4-5]  schemaId      (u16 LE)
  [6-7]  version       (u16 LE) — 版本兼容

PingPong (templateId=1):    msgType(1) + sequence(4) + timestampNs(8)
DataMessage (templateId=2): sequence(4) + timestampNs(8)
ControlMessage (templateId=3): controlCode(1) + sequence(4) + value(8)
```

**SBE 优势：**

| 对比项 | 手写协议 | SBE |
|--------|---------|-----|
| 序列化代码 | ~60 行手写 | 从 XML 自动生成 |
| 类型安全 | 手动偏移，容易出错 | 编译期检查 |
| 添加字段 | 手动调整所有偏移 | 改 XML 重新生成 |
| 版本兼容 | 无 | schema version 内置 |
| 跨语言 | 仅 Rust | Java/C++/Python/Go |

## 项目结构

```
src/
  main.rs          — CLI 入口（clap 参数解析）
  lib.rs           — 模块导出
  driver.rs        — Aeron Media Driver 封装
  protocol.rs      — 手写 64 字节协议
  protocol_sbe.rs  — SBE 协议（ironsbe-codegen 生成）
  server.rs        — 服务端（多客户端支持）
  client.rs        — 客户端（延迟 + 吞吐量测试）
  bench.rs         — 单进程模式
  stats.rs         — 统计输出
schemas/
  bench.xml        — SBE XML Schema
tests/
  integration.rs   — 集成测试
```

## 交易系统应用

Aeron 广泛用于证券交易所和 HFT 系统。本项目验证了 Aeron + Rust 的组合适用于：

- **Gateway → 撮合引擎** — UDP 单播，微秒级延迟
- **撮合 → 行情分发** — UDP 多播，1 对多
- **撮合 → 结算** — UDP 单播 + Aeron Archive 持久化
- **故障恢复** — Aeron Archive 回放重建状态

详细分析见 [RUSTERON_POC_REPORT.md](../RUSTERON_POC_REPORT.md)。

## 依赖

| 组件 | 版本 | 用途 |
|------|------|------|
| rusteron-client | 0.1.162 | Aeron 客户端 FFI 绑定 |
| rusteron-media-driver | 0.1.162 | Aeron Media Driver FFI 绑定 |
| ironsbe-core | 0.2.0 | SBE 编解码核心 |
| ironsbe-codegen | 0.2.0 | SBE XML→Rust 代码生成 |
| clap | 4 | CLI 参数解析 |
| hdrhistogram | 7 | 延迟统计直方图 |
