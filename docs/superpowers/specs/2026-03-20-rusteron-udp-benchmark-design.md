# Rusteron UDP Benchmark — Design Spec

## Overview

A single Rust binary (`rusteron-bench`) that benchmarks the [rusteron](https://github.com/gsrxyz/rusteron) library's UDP communication capabilities using a two-phase testing approach: ping-pong latency measurement followed by unidirectional throughput measurement.

## Goals

- Measure end-to-end UDP message latency (RTT/2) with percentile breakdown (min, avg, p50, p95, p99, max)
- Measure maximum unidirectional throughput (msgs/sec, MB/sec)
- Fixed 64-byte message size
- Pure terminal text output

## Architecture

### Single Binary, Three Modes

```
rusteron-bench --mode server    # Echo server + throughput receiver
rusteron-bench --mode client    # Runs two-phase benchmark
rusteron-bench --mode bench     # Embedded: launches server + client in one process
```

### UDP Channels (Two Separate Channels)

Aeron UDP requires separate channels for each direction (a single endpoint cannot be used bidirectionally):

- **Channel A** (`aeron:udp?endpoint=localhost:20121`, `stream_id=1001`): Client publishes → Server subscribes
- **Channel B** (`aeron:udp?endpoint=localhost:20122`, `stream_id=1002`): Server publishes → Client subscribes

Configurable via `--endpoint` (port 20121) and `--reply-endpoint` (port 20122).

### Aeron Media Driver

Embedded media driver launched via `rusteron-media-driver`:

1. Create `AeronDriverContext`, configure directory (use temp dir with unique name per instance)
2. Call `AeronDriver::launch_embedded(driver_ctx, false)` → returns `(Arc<AtomicBool>, JoinHandle)`
3. Create `AeronContext`, call `ctx.set_dir()` with the same directory as the driver
4. Create `Aeron::new(&ctx)` and call `aeron.start()`
5. Store the stop handle (`Arc<AtomicBool>`) for graceful shutdown

In `server`/`client` mode, each side launches its own embedded driver with separate directories. In `bench` mode, a single shared driver is used (both Aeron client instances connect to the same driver directory).

### Connection Establishment

Before sending any messages, both sides must wait for readiness:
- Publication: spin on `publication.is_connected()` until true (with timeout)
- Subscription: spin on `subscription.is_connected()` or check `image_count() > 0` (with timeout)

This ensures the Aeron async setup is complete before benchmarking begins.

## Message Protocol

```rust
#[repr(C)]
struct BenchMessage {
    msg_type: u8,        // 1=Ping, 2=Pong, 3=Data, 4=Control
    control_code: u8,    // 0=None, 1=StartThroughput, 2=StopThroughput, 3=ReportRequest, 4=ReportResponse
    _padding: [u8; 2],
    sequence: u32,       // Message sequence number
    timestamp_ns: u64,   // Nanosecond timestamp (Instant-based)
    payload: [u8; 48],   // Padding to reach 64 bytes total
}
```

Total: 1 + 1 + 2 + 4 + 8 + 48 = 64 bytes.

### Serialization

Field-by-field serialization using `to_le_bytes()` / `from_le_bytes()` into a `[u8; 64]` buffer. No unsafe transmute. Provide `BenchMessage::write_to(&self, buf: &mut [u8])` and `BenchMessage::read_from(buf: &[u8]) -> Self` methods.

## Server Logic

1. Start embedded Aeron media driver (see Media Driver section)
2. Wait for connection establishment
3. Create subscription on Channel A (stream 1001), publication on Channel B (stream 1002)
4. Busy-spin poll loop (call `subscription.poll()` continuously):
   - `Ping` → reply with `Pong` (same sequence + timestamp) via `publication.offer()`, retry on back-pressure
   - `Data` → increment counter
   - `Control(StopThroughput)` → record end time, compute stats
   - `Control(ReportRequest)` → send `Control(ReportResponse)` with throughput count, retry on back-pressure

All `offer()` calls must handle back-pressure: retry in a spin loop when return value is negative (`BACK_PRESSURED`, `NOT_CONNECTED`, `ADMIN_ACTION`).

## Client Logic

### Phase 1 — Latency (Ping-Pong)

1. Send 100,000 ping messages sequentially (configurable via `--ping-count`)
2. For each ping: record send time via `Instant::now()`, call `publication.offer()` (retry on back-pressure)
3. Busy-spin `subscription.poll()` until pong received, compute RTT = `Instant::now() - send_time`
4. Timeout after 1 second per message (treat as lost, skip)
5. Discard first 10,000 as warmup (configurable via `--warmup`)
6. Collect RTTs into HdrHistogram, report min/avg/p50/p95/p99/max

### Phase 2 — Throughput (Unidirectional)

1. Send `Control(StartThroughput)` to Server (retry on back-pressure)
2. Continuously send `Data` messages for 10 seconds (configurable via `--duration`), retry on back-pressure
3. Send `Control(StopThroughput)`
4. Wait 1 second drain period for server to process remaining queued messages
5. Send `Control(ReportRequest)`
6. Busy-spin `subscription.poll()` until `Control(ReportResponse)` received
7. Compute and report msgs/sec and MB/sec

## Bench Mode

1. Launch single embedded media driver
2. Spawn server logic in a background thread — server creates its **own** `Aeron` instance connected to the shared driver directory (Aeron structs are not Send/Sync, each thread needs its own instance)
3. Run client logic on main thread with its **own** `Aeron` instance
4. Join server thread, set stop handle to true, shut down driver

### Graceful Shutdown

- Server mode: trap SIGINT (via `ctrlc` crate) to set the stop flag and cleanly shut down the media driver
- Bench mode: client signals server thread via an `AtomicBool` after benchmark completes

## Terminal Output

```
=== Rusteron UDP Benchmark ===
Message size: 64 bytes | Warmup: 10000 msgs

--- Phase 1: Latency (Ping-Pong) ---
  Messages:  90,000 (after warmup)
  Min:       5.2 us
  Avg:       9.8 us
  P50:       9.1 us
  P95:      12.3 us
  P99:      15.7 us
  Max:      42.1 us

--- Phase 2: Throughput (Unidirectional) ---
  Duration:  10.00 s
  Messages:  38,421,000
  Throughput: 3,842,100 msgs/sec
  Bandwidth:  234.5 MB/sec
```

## CLI Arguments

| Flag | Default | Description |
|------|---------|-------------|
| `--mode` | required | `server`, `client`, or `bench` |
| `--endpoint` | `localhost:20121` | Client→Server UDP endpoint |
| `--reply-endpoint` | `localhost:20122` | Server→Client UDP endpoint |
| `--stream-id-send` | `1001` | Client→Server stream |
| `--stream-id-recv` | `1002` | Server→Client stream |
| `--ping-count` | `100000` | Number of ping messages |
| `--warmup` | `10000` | Warmup messages to discard |
| `--duration` | `10` | Throughput test duration (seconds) |

## Dependencies

```toml
[dependencies]
rusteron-client = { version = "0.1", features = ["static"] }
rusteron-media-driver = { version = "0.1", features = ["static"] }
clap = { version = "4", features = ["derive"] }
hdrhistogram = "7"
ctrlc = "3"
```

## Project Structure

```
src/
  main.rs          # CLI parsing, mode dispatch
  protocol.rs      # BenchMessage definition, serialization
  driver.rs        # Media driver setup/teardown
  server.rs        # Server mode logic
  client.rs        # Client mode logic (phase 1 + phase 2)
  bench.rs         # Bench mode (embedded server + client)
  stats.rs         # Latency histogram + throughput formatting
```

## Agent Team

| Role | Name | Responsibilities |
|------|------|-----------------|
| Project Manager | `pm` | Task coordination, progress tracking, review |
| Developer | `developer` | All code implementation |
| QA | `qa` | Tests, verification, end-to-end validation |

## Task Breakdown

1. Project initialization (Cargo.toml, directory structure, dependencies)
2. Message protocol module (BenchMessage, serialization/deserialization)
3. Media driver management module (embedded launch/stop)
4. Server mode implementation (echo + throughput receiver)
5. Client mode — Phase 1 latency test (ping-pong)
6. Client mode — Phase 2 throughput test (unidirectional)
7. Bench mode (embedded server + client)
8. Terminal output formatting
9. Unit tests + integration tests
10. End-to-end validation

## Safety Notes

- rusteron operates in unsafe Rust contexts; careful lifetime management required
- Aeron structs are not Send/Sync — do not share across threads without proper wrapping
- In bench mode, server runs in its own thread with its own Aeron client instance
