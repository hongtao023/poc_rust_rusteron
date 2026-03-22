//! 服务端模块
//!
//! 服务端的职责：
//! 1. 收到 Ping → 立即回复 Pong（用于延迟测量）
//! 2. 收到 Data → 计数（用于吞吐量统计）
//! 3. 收到 Control 指令 → 执行对应操作（开始/停止计数、返回统计报告）
//!
//! 通信模型：
//!   Client --[endpoint]--> Server (接收 Ping/Data/Control)
//!   Server --[reply_endpoint]--> Client (发送 Pong/ReportResponse)

use crate::driver::{self, EmbeddedDriver};
use crate::protocol::{BenchMessage, ControlCode, MsgType, MESSAGE_SIZE};
use rusteron_client::{AeronFragmentHandlerCallback, AeronHeader, Handler};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// 服务端内部状态
///
/// 实现了 AeronFragmentHandlerCallback trait，
/// 当 Aeron Subscription 收到消息时会调用 handle_aeron_fragment_handler。
struct ServerState {
    throughput_count: u64,                                  // 吞吐量测试期间收到的 Data 消息计数
    throughput_start: Option<Instant>,                      // 吞吐量测试开始时间
    throughput_end: Option<Instant>,                        // 吞吐量测试结束时间
    reply_buf: [u8; MESSAGE_SIZE],                          // 回复消息的序列化缓冲区（复用，避免每次分配）
    publication: Option<rusteron_client::AeronPublication>, // 用于发送回复的 Publication
}

/// Aeron 消息处理回调实现
///
/// 每次 subscription.poll() 拉取到消息时，Aeron 会调用此方法。
/// 根据消息类型分发到不同的处理逻辑。
impl AeronFragmentHandlerCallback for ServerState {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        // 忽略不足 64 字节的消息（不符合协议格式）
        if buffer.len() < MESSAGE_SIZE {
            return;
        }
        let msg = BenchMessage::read_from(buffer);
        match msg.msg_type {
            // ---- Ping 处理 ----
            // 收到 Ping 后，构造 Pong 回复（保留相同的 sequence 和 timestamp）
            // 这样 client 可以通过 sequence 匹配请求和响应，用 timestamp 计算 RTT
            MsgType::Ping => {
                let pong = BenchMessage {
                    msg_type: MsgType::Pong,
                    ..msg // 复制 sequence、timestamp_ns、payload 等字段
                };
                pong.write_to(&mut self.reply_buf);
                if let Some(pub_ref) = &self.publication {
                    driver::offer_with_retry(pub_ref, &self.reply_buf);
                }
            }
            // ---- Data 处理 ----
            // 吞吐量测试阶段：记录第一条 Data 的到达时间，并累加计数
            MsgType::Data => {
                if self.throughput_start.is_none() {
                    self.throughput_start = Some(Instant::now());
                }
                self.throughput_count += 1;
            }
            // ---- Control 处理 ----
            MsgType::Control => match msg.control_code {
                // 收到"开始吞吐量测试"指令：重置计数器和计时器
                ControlCode::StartThroughput => {
                    self.throughput_count = 0;
                    self.throughput_start = Some(Instant::now());
                }
                // 收到"停止吞吐量测试"指令：记录结束时间
                ControlCode::StopThroughput => {
                    self.throughput_end = Some(Instant::now());
                }
                // 收到"请求报告"指令：将收到的消息总数通过 ReportResponse 回复给 client
                // 消息总数同时存在 sequence（低 32 位）和 timestamp_ns（完整 64 位）中
                ControlCode::ReportRequest => {
                    let mut response = BenchMessage::control(ControlCode::ReportResponse, 0);
                    response.sequence = (self.throughput_count & 0xFFFF_FFFF) as u32;
                    response.timestamp_ns = self.throughput_count;
                    response.write_to(&mut self.reply_buf);
                    if let Some(pub_ref) = &self.publication {
                        driver::offer_with_retry(pub_ref, &self.reply_buf);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// 启动并运行服务端
///
/// 流程：
/// 1. 连接到 Aeron Media Driver
/// 2. 在 endpoint 上创建 Subscription（接收 client 消息）
/// 3. 在 reply_endpoint 上创建 Publication（发送回复）
/// 4. 进入 busy-spin 主循环，持续 poll 消息直到收到停止信号
///
/// 参数：
/// - driver:         内嵌 Media Driver 实例
/// - endpoint:       接收通道地址（如 "0.0.0.0:20121"）
/// - reply_endpoint: 回复通道地址（如 "10.0.0.100:20122"）
/// - stream_id_recv: 接收方向的 stream ID
/// - stream_id_send: 发送方向的 stream ID
/// - stop:           停止信号（Ctrl+C 或外部设置为 true 时退出循环）
pub fn run_server(
    driver: &EmbeddedDriver,
    endpoint: &str,
    reply_endpoint: &str,
    stream_id_recv: i32,
    stream_id_send: i32,
    stop: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 连接到 Aeron Media Driver
    let aeron = driver.connect()?;

    // 创建 Subscription（接收 client→server 消息）和 Publication（发送 server→client 回复）
    let subscription = driver::add_subscription(&aeron, endpoint, stream_id_recv)?;
    let publication = driver::add_publication(&aeron, reply_endpoint, stream_id_send)?;

    println!("Server listening on {} (stream {})", endpoint, stream_id_recv);
    println!("Server replying on {} (stream {})", reply_endpoint, stream_id_send);

    // 初始化服务端状态
    let state = ServerState {
        throughput_count: 0,
        throughput_start: None,
        throughput_end: None,
        reply_buf: [0u8; MESSAGE_SIZE],
        publication: Some(publication),
    };

    // Handler::leak 将 state 移动到堆上并泄露（不释放），
    // 这样 Aeron 的 C 回调可以安全地持有指针。
    // 在基准测试场景下这个泄露是可接受的（进程退出时由 OS 回收）。
    let handler = Handler::leak(state);

    // 主循环：busy-spin 轮询，每次最多处理 10 条消息
    while !stop.load(Ordering::Relaxed) {
        let _ = subscription.poll(Some(&handler), 10);
    }

    println!("Server shutting down.");
    Ok(())
}
