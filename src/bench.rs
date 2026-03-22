//! 单进程基准测试模块（Bench 模式）
//!
//! 在同一个进程内同时运行 server 和 client，用于本地快速测试。
//! 共享同一个 Aeron Media Driver，但 server 和 client 各自创建独立的 Aeron 客户端实例。
//!
//! 架构：
//!   [主线程] Media Driver + Client
//!   [子线程] Server（使用相同的 Media Driver 目录）
//!
//! 适用场景：本机开发调试、CI 测试、快速验证协议正确性

use crate::driver::EmbeddedDriver;
use crate::server;
use crate::client;
use crate::stats::BenchResults;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 运行 bench 模式：在单进程内启动 server + client
///
/// 流程：
/// 1. 启动一个共享的 Media Driver
/// 2. 在后台线程启动 server（共用同一个 Driver 目录）
/// 3. 等待 500ms 让 server 完成初始化
/// 4. 在主线程运行 client（执行延迟 + 吞吐量测试）
/// 5. 测试完成后，发送停止信号给 server 线程
/// 6. 等待 server 线程退出，返回测试结果
pub fn run_bench(
    endpoint: &str,
    reply_endpoint: &str,
    stream_id_send: i32,
    stream_id_recv: i32,
    ping_count: u64,
    warmup: u64,
    duration_secs: u64,
) -> Result<BenchResults, Box<dyn std::error::Error>> {
    // 启动共享的 Media Driver（主线程持有，负责生命周期管理）
    let driver = EmbeddedDriver::launch()?;
    let driver_dir = driver.dir.clone();

    // server 线程的停止信号
    let server_stop = Arc::new(AtomicBool::new(false));
    let server_stop_clone = server_stop.clone();

    // 克隆 endpoint 字符串供 server 线程使用（跨线程传递需要 owned String）
    let srv_endpoint = endpoint.to_string();
    let srv_reply_endpoint = reply_endpoint.to_string();

    // 在后台线程启动 server
    // server 创建自己的 EmbeddedDriver 实例，但只用它来连接到共享的 Driver 目录，
    // 不会启动新的 Media Driver（handle 为 None）
    let server_handle = std::thread::spawn(move || -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // 创建一个"伪" EmbeddedDriver，仅用于提供 dir 路径以连接到共享 Driver
        let server_driver = EmbeddedDriver {
            stop: Arc::new(AtomicBool::new(false)), // 不用于控制 Driver（Driver 由主线程管理）
            handle: None, // 无 Driver 线程句柄（共享 Driver）
            dir: driver_dir,
        };

        let client_configs = vec![server::ClientConfig {
            reply_endpoint: srv_reply_endpoint,
            stream_id_recv: stream_id_send,
            stream_id_send: stream_id_recv,
        }];
        server::run_server(
            &server_driver,
            &srv_endpoint,
            &client_configs,
            server_stop_clone,
        ).map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            e.to_string().into()
        })?;
        Ok(())
    });

    // 等待 500ms 让 server 完成 Subscription/Publication 的创建和连接
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 在主线程运行 client，执行两个测试阶段
    let results = client::run_client(
        &driver,
        endpoint,
        reply_endpoint,
        stream_id_send,
        stream_id_recv,
        ping_count,
        warmup,
        duration_secs,
    )?;

    // 测试完成，通知 server 线程退出
    server_stop.store(true, Ordering::SeqCst);

    // 等待 server 线程结束，处理可能的错误
    match server_handle.join() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("Server error: {}", e),
        Err(_) => eprintln!("Server thread panicked"),
    }

    // Driver 会在 drop 时自动停止并清理
    Ok(results)
}
