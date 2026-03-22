//! 客户端模块
//!
//! 客户端执行两个测试阶段：
//! - Phase 1（延迟测试）：发送 Ping，等待 Pong，记录 RTT
//! - Phase 2（吞吐量测试）：持续发送 Data，然后向 server 请求收到的消息总数
//!
//! 通信模型：
//!   Client --[endpoint]--> Server (发送 Ping/Data/Control)
//!   Client <--[reply_endpoint]-- Server (接收 Pong/ReportResponse)

use crate::driver::{self, EmbeddedDriver};
use crate::protocol::{BenchMessage, ControlCode, MsgType, MESSAGE_SIZE};
use crate::stats::{BenchResults, LatencyStats, ThroughputStats};
use hdrhistogram::Histogram;
use rusteron_client::{AeronFragmentHandlerCallback, AeronHeader, AeronPublication, AeronSubscription, Handler};
use std::time::{Duration, Instant};

/// 等待单条消息回复的超时时间
const MSG_TIMEOUT: Duration = Duration::from_secs(1);

/// Ping-Pong 回调状态
/// 用于在 Aeron 的 fragment handler 回调中标记是否收到了 Pong 回复
struct PingPongState {
    received: bool,       // 是否已收到 Pong
    pong_sequence: u32,   // 收到的 Pong 的序列号
}

impl AeronFragmentHandlerCallback for PingPongState {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        if buffer.len() >= MESSAGE_SIZE {
            let msg = BenchMessage::read_from(buffer);
            if msg.msg_type == MsgType::Pong {
                self.pong_sequence = msg.sequence;
                self.received = true;
            }
        }
    }
}

/// 吞吐量报告回调状态
/// 用于接收 server 返回的 ReportResponse（包含 server 实际收到的消息总数）
struct ReportState {
    received: bool,  // 是否已收到报告
    count: u64,      // server 报告的消息总数
}

impl AeronFragmentHandlerCallback for ReportState {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        if buffer.len() >= MESSAGE_SIZE {
            let msg = BenchMessage::read_from(buffer);
            if msg.msg_type == MsgType::Control && msg.control_code == ControlCode::ReportResponse {
                self.count = msg.timestamp_ns; // server 将消息计数存在 timestamp_ns 字段
                self.received = true;
            }
        }
    }
}

/// Phase 1：延迟测试（Ping-Pong）
///
/// 工作流程：
/// 1. 循环发送 ping_count 条 Ping 消息
/// 2. 每发一条就等待对应的 Pong 回复
/// 3. 记录每次 RTT（往返延迟）
/// 4. 前 warmup 条消息不计入统计（让系统预热，如 JIT、缓存、路由表等）
/// 5. 使用 HDR Histogram 统计延迟分布
///
/// 注意：使用 unsafe 裸指针访问 handler 内部状态，
/// 因为 Handler::leak 会将状态移到堆上，回调和主循环需要共享访问。
fn run_phase1_latency(
    publication: &AeronPublication,
    subscription: &AeronSubscription,
    ping_count: u64,
    warmup: u64,
) -> Result<LatencyStats, Box<dyn std::error::Error>> {
    // 创建 3 位有效数字精度的 HDR 直方图，用于记录延迟分布
    let mut histogram = Histogram::<u64>::new(3)?;
    let mut send_buf = [0u8; MESSAGE_SIZE];

    // 创建回调状态并泄露到堆上（Aeron C 回调需要稳定的指针）
    let state = PingPongState {
        received: false,
        pong_sequence: 0,
    };
    let handler = Handler::leak(state);
    // 获取裸指针，用于在主循环中读取/重置回调状态
    let state_ptr = handler.as_raw() as *mut PingPongState;

    println!("  Sending {} pings ({} warmup)...", ping_count, warmup);

    for seq in 0..ping_count {
        // 构造 Ping 消息
        let ping = BenchMessage::new(MsgType::Ping, seq as u32);
        ping.write_to(&mut send_buf);

        // 重置接收标志
        unsafe {
            (*state_ptr).received = false;
        }

        // 记录发送时间，然后发送 Ping
        let send_time = Instant::now();
        driver::offer_with_retry(publication, &send_buf);

        // 自旋轮询等待 Pong 回复，超时则跳过该消息
        let deadline = send_time + MSG_TIMEOUT;
        loop {
            let _ = subscription.poll(Some(&handler), 10);
            let received = unsafe { (*state_ptr).received };
            if received {
                break;
            }
            if Instant::now() >= deadline {
                break; // 超时，放弃这条消息
            }
            std::hint::spin_loop();
        }

        let rtt = send_time.elapsed();
        let received = unsafe { (*state_ptr).received };

        // 只记录预热之后且成功收到回复的 RTT
        if seq >= warmup && received {
            let rtt_us = rtt.as_nanos() as u64 / 1000; // 纳秒转微秒
            let _ = histogram.record(rtt_us.max(1));    // 最少记录 1us
        }
    }

    let count = histogram.len();
    if count == 0 {
        return Err("No pong responses received".into());
    }

    // 从直方图中提取统计指标
    Ok(LatencyStats {
        count,
        min_us: histogram.min() as f64,
        avg_us: histogram.mean(),
        p50_us: histogram.value_at_percentile(50.0) as f64,
        p95_us: histogram.value_at_percentile(95.0) as f64,
        p99_us: histogram.value_at_percentile(99.0) as f64,
        max_us: histogram.max() as f64,
    })
}

/// Phase 2：吞吐量测试（单向发送）
///
/// 工作流程：
/// 1. 发送 StartThroughput 控制指令通知 server 开始计数
/// 2. 在指定时间内持续发送 Data 消息（尽最大速率）
/// 3. 发送 StopThroughput 控制指令通知 server 停止计数
/// 4. 等待 1 秒让网络上的剩余数据包到达 server（drain period）
/// 5. 发送 ReportRequest 请求 server 返回实际收到的消息总数
/// 6. 根据 server 报告的数量和本地计时计算吞吐量
fn run_phase2_throughput(
    publication: &AeronPublication,
    subscription: &AeronSubscription,
    duration_secs: u64,
) -> Result<ThroughputStats, Box<dyn std::error::Error>> {
    let mut send_buf = [0u8; MESSAGE_SIZE];

    // 步骤 1：发送"开始吞吐量测试"控制指令
    let start_msg = BenchMessage::control(ControlCode::StartThroughput, 0);
    start_msg.write_to(&mut send_buf);
    driver::offer_with_retry(publication, &send_buf);

    // 步骤 2：在指定时间内持续发送 Data 消息
    let start = Instant::now();
    let duration = Duration::from_secs(duration_secs);
    let mut seq: u32 = 0;

    println!("  Sending data for {} seconds...", duration_secs);

    while start.elapsed() < duration {
        let data = BenchMessage::new(MsgType::Data, seq);
        data.write_to(&mut send_buf);
        driver::offer_with_retry(publication, &send_buf);
        seq = seq.wrapping_add(1); // wrapping_add 防止 u32 溢出 panic
    }

    let elapsed = start.elapsed();

    // 步骤 3：发送"停止吞吐量测试"控制指令
    let stop_msg = BenchMessage::control(ControlCode::StopThroughput, 0);
    stop_msg.write_to(&mut send_buf);
    driver::offer_with_retry(publication, &send_buf);

    // 步骤 4：等待 1 秒，让网络中的剩余数据包到达 server
    std::thread::sleep(Duration::from_secs(1));

    // 步骤 5：发送"请求报告"控制指令
    let report_msg = BenchMessage::control(ControlCode::ReportRequest, 0);
    report_msg.write_to(&mut send_buf);
    driver::offer_with_retry(publication, &send_buf);

    // 步骤 6：等待 server 返回 ReportResponse
    let state = ReportState {
        received: false,
        count: 0,
    };
    let handler = Handler::leak(state);
    let state_ptr = handler.as_raw() as *mut ReportState;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = subscription.poll(Some(&handler), 10);
        let received = unsafe { (*state_ptr).received };
        if received {
            break;
        }
        if Instant::now() >= deadline {
            return Err("Timeout waiting for throughput report".into());
        }
        std::hint::spin_loop();
    }

    // 从 server 报告中读取实际收到的消息数，计算吞吐量指标
    let server_count = unsafe { (*state_ptr).count };
    let duration_secs_f = elapsed.as_secs_f64();
    let msgs_per_sec = server_count as f64 / duration_secs_f;
    // 带宽 = 消息数 × 64 字节 / 1MB / 时间
    let mb_per_sec = (server_count as f64 * MESSAGE_SIZE as f64) / (1024.0 * 1024.0) / duration_secs_f;

    Ok(ThroughputStats {
        duration_secs: duration_secs_f,
        messages: server_count,
        msgs_per_sec,
        mb_per_sec,
    })
}

/// 运行客户端：依次执行 Phase 1（延迟）和 Phase 2（吞吐量）
///
/// 参数：
/// - driver:         内嵌 Media Driver 实例
/// - endpoint:       发送通道地址（client→server，如 "10.0.0.100:20121"）
/// - reply_endpoint: 接收通道地址（server→client，如 "0.0.0.0:20122"）
/// - stream_id_send: 发送方向的 stream ID
/// - stream_id_recv: 接收方向的 stream ID
/// - ping_count:     延迟测试发送的 Ping 总数
/// - warmup:         预热消息数（不计入统计）
/// - duration_secs:  吞吐量测试持续秒数
pub fn run_client(
    driver: &EmbeddedDriver,
    endpoint: &str,
    reply_endpoint: &str,
    stream_id_send: i32,
    stream_id_recv: i32,
    ping_count: u64,
    warmup: u64,
    duration_secs: u64,
) -> Result<BenchResults, Box<dyn std::error::Error>> {
    // 连接到 Aeron Media Driver
    let aeron = driver.connect()?;

    // 创建 Publication（client→server）和 Subscription（server→client）
    let publication = driver::add_publication(&aeron, endpoint, stream_id_send)?;
    let subscription = driver::add_subscription(&aeron, reply_endpoint, stream_id_recv)?;

    println!("Client connected.");

    // Phase 1: 延迟测试
    println!("\n--- Phase 1: Latency (Ping-Pong) ---");
    let latency = run_phase1_latency(&publication, &subscription, ping_count, warmup)?;

    // Phase 2: 吞吐量测试
    println!("\n--- Phase 2: Throughput (Unidirectional) ---");
    let throughput = run_phase2_throughput(&publication, &subscription, duration_secs)?;

    Ok(BenchResults {
        latency: Some(latency),
        throughput: Some(throughput),
    })
}
