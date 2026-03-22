//! 服务端模块
//!
//! 服务端的职责：
//! 1. 收到 Ping → 立即回复 Pong（用于延迟测量）
//! 2. 收到 Data → 计数（用于吞吐量统计）
//! 3. 收到 Control 指令 → 执行对应操作（开始/停止计数、返回统计报告）
//!
//! 支持多客户端：每个客户端使用独立的 stream ID 对，
//! server 为每个客户端创建独立的 subscription + publication。
//!
//! 通信模型（每个客户端）：
//!   Client --[endpoint, stream_id_recv]--> Server (接收 Ping/Data/Control)
//!   Server --[reply_endpoint, stream_id_send]--> Client (发送 Pong/ReportResponse)

use crate::driver::{self, EmbeddedDriver};
use crate::protocol::{BenchMessage, ControlCode, MsgType, MESSAGE_SIZE};
use rusteron_client::{AeronFragmentHandlerCallback, AeronHeader, AeronSubscription, Handler};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// 单个客户端连接的配置
pub struct ClientConfig {
    pub reply_endpoint: String,
    pub stream_id_recv: i32,
    pub stream_id_send: i32,
}

/// 服务端内部状态（每个客户端独立一份）
struct ServerState {
    throughput_count: u64,
    throughput_start: Option<Instant>,
    throughput_end: Option<Instant>,
    reply_buf: [u8; MESSAGE_SIZE],
    publication: Option<rusteron_client::AeronPublication>,
}

impl AeronFragmentHandlerCallback for ServerState {
    fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
        if buffer.len() < MESSAGE_SIZE {
            return;
        }
        let msg = BenchMessage::read_from(buffer);
        match msg.msg_type {
            MsgType::Ping => {
                let pong = BenchMessage {
                    msg_type: MsgType::Pong,
                    ..msg
                };
                pong.write_to(&mut self.reply_buf);
                if let Some(pub_ref) = &self.publication {
                    driver::offer_with_retry(pub_ref, &self.reply_buf);
                }
            }
            MsgType::Data => {
                if self.throughput_start.is_none() {
                    self.throughput_start = Some(Instant::now());
                }
                self.throughput_count += 1;
            }
            MsgType::Control => match msg.control_code {
                ControlCode::StartThroughput => {
                    self.throughput_count = 0;
                    self.throughput_start = Some(Instant::now());
                }
                ControlCode::StopThroughput => {
                    self.throughput_end = Some(Instant::now());
                }
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

/// 启动并运行服务端（支持多客户端）
///
/// 每个 ClientConfig 对应一个独立的 subscription + publication + 状态。
/// 主循环中轮询所有 subscription。
pub fn run_server(
    driver: &EmbeddedDriver,
    endpoint: &str,
    client_configs: &[ClientConfig],
    stop: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let aeron = driver.connect()?;

    let mut subscriptions: Vec<AeronSubscription> = Vec::new();
    let mut handlers: Vec<Handler<ServerState>> = Vec::new();

    for (i, config) in client_configs.iter().enumerate() {
        let subscription =
            driver::add_subscription_no_wait(&aeron, endpoint, config.stream_id_recv)?;
        let publication =
            driver::add_publication_no_wait(&aeron, &config.reply_endpoint, config.stream_id_send)?;

        println!(
            "Server client[{}]: recv stream {} on {}, reply stream {} to {}",
            i, config.stream_id_recv, endpoint, config.stream_id_send, config.reply_endpoint
        );

        let state = ServerState {
            throughput_count: 0,
            throughput_start: None,
            throughput_end: None,
            reply_buf: [0u8; MESSAGE_SIZE],
            publication: Some(publication),
        };
        handlers.push(Handler::leak(state));
        subscriptions.push(subscription);
    }

    println!(
        "Server listening on {} ({} client(s))",
        endpoint,
        client_configs.len()
    );

    // 主循环：轮询所有 subscription
    while !stop.load(Ordering::Relaxed) {
        for (sub, handler) in subscriptions.iter().zip(handlers.iter()) {
            let _ = sub.poll(Some(handler), 10);
        }
    }

    println!("Server shutting down.");
    Ok(())
}
