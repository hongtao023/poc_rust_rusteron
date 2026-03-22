//! 程序入口
//! 通过命令行参数选择运行模式：server / client / bench
//! - server: 启动服务端，监听并回复消息
//! - client: 启动客户端，执行延迟+吞吐量测试
//! - bench:  单进程模式，内部同时启动 server 和 client

use clap::Parser;
use rusteron_bench::{bench, client, driver, server, stats};

/// 命令行参数定义
#[derive(Parser, Debug)]
#[command(name = "rusteron-bench", about = "Rusteron UDP Benchmark")]
struct Args {
    /// 运行模式：server（服务端）、client（客户端）、bench（单进程）
    #[arg(long)]
    mode: String,

    /// Client→Server UDP 通道地址（server 监听 / client 发送目标）
    #[arg(long, default_value = "localhost:20121")]
    endpoint: String,

    /// Server→Client UDP 通道地址（server 回复发送 / client 监听）
    #[arg(long, default_value = "localhost:20122")]
    reply_endpoint: String,

    /// Client→Server 方向的 Aeron stream ID
    #[arg(long, default_value_t = 1001)]
    stream_id_send: i32,

    /// Server→Client 方向的 Aeron stream ID
    #[arg(long, default_value_t = 1002)]
    stream_id_recv: i32,

    /// 延迟测试（Phase 1）要发送的 Ping 消息总数
    #[arg(long, default_value_t = 100_000)]
    ping_count: u64,

    /// 预热消息数（前 N 条不计入统计）
    #[arg(long, default_value_t = 10_000)]
    warmup: u64,

    /// 吞吐量测试（Phase 2）持续秒数
    #[arg(long, default_value_t = 10)]
    duration: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 解析命令行参数
    let args = Args::parse();

    match args.mode.as_str() {
        // ========== Server 模式 ==========
        // 启动内嵌 Media Driver → 注册 Ctrl+C 信号处理 → 进入 server 主循环
        "server" => {
            let drv = driver::EmbeddedDriver::launch()?;
            // 克隆 stop 标志给 Ctrl+C handler，按下后通知 server 退出
            let stop = drv.stop.clone();
            ctrlc::set_handler(move || {
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
            })?;
            server::run_server(
                &drv,
                &args.endpoint,
                &args.reply_endpoint,
                args.stream_id_send,
                args.stream_id_recv,
                drv.stop.clone(),
            )?;
        }
        // ========== Client 模式 ==========
        // 启动内嵌 Media Driver → 运行延迟+吞吐量测试 → 打印结果
        "client" => {
            let drv = driver::EmbeddedDriver::launch()?;
            let results = client::run_client(
                &drv,
                &args.endpoint,
                &args.reply_endpoint,
                args.stream_id_send,
                args.stream_id_recv,
                args.ping_count,
                args.warmup,
                args.duration,
            )?;
            stats::print_results(&results, args.warmup);
        }
        // ========== Bench 模式 ==========
        // 单进程内同时运行 server 和 client（用于本地快速测试）
        "bench" => {
            let results = bench::run_bench(
                &args.endpoint,
                &args.reply_endpoint,
                args.stream_id_send,
                args.stream_id_recv,
                args.ping_count,
                args.warmup,
                args.duration,
            )?;
            stats::print_results(&results, args.warmup);
        }
        other => {
            eprintln!("Unknown mode: {}. Use server, client, or bench.", other);
            std::process::exit(1);
        }
    }

    Ok(())
}
