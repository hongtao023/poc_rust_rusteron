//! 程序入口
//! 通过命令行参数选择运行模式：server / client / bench
//! - server: 启动服务端，监听并回复消息（支持多客户端）
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

    /// Server→Client UDP 通道地址（单客户端模式）
    #[arg(long, default_value = "localhost:20122")]
    reply_endpoint: String,

    /// Server 模式：逗号分隔的多个 reply endpoint（多客户端模式）
    /// 例如：172.31.36.93:20122,172.31.38.162:20122
    /// 每个 endpoint 自动分配 stream ID 对：第 1 个用 1001/1002，第 2 个用 2001/2002
    /// 不指定时使用 --reply-endpoint（单客户端模式）
    #[arg(long, value_delimiter = ',')]
    reply_endpoints: Option<Vec<String>>,

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
    let args = Args::parse();

    match args.mode.as_str() {
        // ========== Server 模式 ==========
        "server" => {
            let drv = driver::EmbeddedDriver::launch()?;
            let stop = drv.stop.clone();
            ctrlc::set_handler(move || {
                stop.store(true, std::sync::atomic::Ordering::SeqCst);
            })?;

            // 构建客户端配置列表
            let client_configs = if let Some(ref endpoints) = args.reply_endpoints {
                // 多客户端模式：每个 reply endpoint 自动分配 stream ID 对
                endpoints
                    .iter()
                    .enumerate()
                    .map(|(i, ep)| {
                        let base = (i as i32 + 1) * 1000;
                        server::ClientConfig {
                            reply_endpoint: ep.clone(),
                            stream_id_recv: base + 1, // 1001, 2001, 3001, ...
                            stream_id_send: base + 2, // 1002, 2002, 3002, ...
                        }
                    })
                    .collect()
            } else {
                // 单客户端模式：使用 --reply-endpoint 和 --stream-id-* 参数
                vec![server::ClientConfig {
                    reply_endpoint: args.reply_endpoint.clone(),
                    stream_id_recv: args.stream_id_send,
                    stream_id_send: args.stream_id_recv,
                }]
            };

            server::run_server(&drv, &args.endpoint, &client_configs, drv.stop.clone())?;
        }
        // ========== Client 模式 ==========
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
