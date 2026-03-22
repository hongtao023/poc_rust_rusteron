# Rusteron UDP Benchmark

基于 [Aeron](https://github.com/real-logic/aeron) 的 UDP 延迟 & 吞吐量基准测试工具，使用 [rusteron](https://github.com/mimalloc/rusteron) Rust binding。

## 功能

- **Ping-Pong 延迟测试** — 测量 RTT，输出 min / avg / p50 / p95 / p99 / max
- **单向吞吐量测试** — 测量 msgs/sec 和 MB/sec
- 双通道 UDP（发送 / 回复各一个 endpoint）
- 内嵌 Aeron Media Driver，无需单独启动

## 构建

```bash
cargo build --release
```

> 依赖 `rusteron-media-driver` 的 `static` feature，会静态链接 Aeron C driver，首次编译较慢。

## 快速开始（本地测试）

单进程 bench 模式，自动启动 server + client：

```bash
cargo run --release -- --mode bench
```

## 远程部署（Client / Server 分离）

假设远程服务器 IP 为 `10.0.0.100`，本地为 client。

### 1. 在远程机器上编译并启动 Server

```bash
# 编译
cargo build --release

# 启动 server
./target/release/rusteron-bench \
  --mode server \
  --endpoint 0.0.0.0:20121 \
  --reply-endpoint 10.0.0.100:20122
```

- `--endpoint 0.0.0.0:20121` — 监听所有网卡，接收 client 发来的消息
- `--reply-endpoint 10.0.0.100:20122` — server 回复消息的发送地址（用服务器自身 IP）

### 2. 在本地启动 Client

```bash
./target/release/rusteron-bench \
  --mode client \
  --endpoint 10.0.0.100:20121 \
  --reply-endpoint 0.0.0.0:20122
```

- `--endpoint 10.0.0.100:20121` — client 发送消息到远程 server
- `--reply-endpoint 0.0.0.0:20122` — client 监听 server 的回复

### 防火墙

确保两台机器上 UDP 端口 `20121` 和 `20122` 已开放：

```bash
# Linux (远程)
sudo ufw allow 20121/udp
sudo ufw allow 20122/udp

# 或 iptables
sudo iptables -A INPUT -p udp --dport 20121 -j ACCEPT
sudo iptables -A INPUT -p udp --dport 20122 -j ACCEPT
```

## 参数说明

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--mode` | (必填) | `server` / `client` / `bench` |
| `--endpoint` | `localhost:20121` | Client→Server 通道地址 |
| `--reply-endpoint` | `localhost:20122` | Server→Client 通道地址 |
| `--stream-id-send` | `1001` | 发送 stream ID |
| `--stream-id-recv` | `1002` | 接收 stream ID |
| `--ping-count` | `100,000` | 延迟测试消息数 |
| `--warmup` | `10,000` | 预热消息数（不计入统计） |
| `--duration` | `10` | 吞吐量测试持续秒数 |

## 输出示例

```
=== Rusteron UDP Benchmark ===
Message size: 64 bytes | Warmup: 10,000 msgs

--- Phase 1: Latency (Ping-Pong) ---
  Messages:  90,000
  Min:       5.2 us
  Avg:       9.8 us
  P50:       9.1 us
  P95:       12.3 us
  P99:       15.7 us
  Max:       42.1 us

--- Phase 2: Throughput (Unidirectional) ---
  Duration:  10.00 s
  Messages:  38,421,000
  Throughput: 3,842,100 msgs/sec
  Bandwidth:  234.5 MB/sec
```

## 协议

固定 64 字节消息格式：

```
[0]      msg_type     (Ping/Pong/Data/Control)
[1]      control_code
[2-3]    padding
[4-7]    sequence     (u32 LE)
[8-15]   timestamp_ns (u64 LE)
[16-63]  payload      (48 bytes)
```
