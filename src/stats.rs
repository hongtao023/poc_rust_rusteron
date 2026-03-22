//! 统计结果与输出模块
//!
//! 定义基准测试的结果数据结构，并提供格式化输出功能。
//! 延迟结果来自 HDR Histogram，吞吐量结果来自 server 的 ReportResponse。

/// 延迟统计结果（来自 Phase 1 Ping-Pong 测试）
///
/// 所有延迟值单位为微秒（us）
#[derive(Debug, Default)]
pub struct LatencyStats {
    pub count: u64,    // 有效测量的消息数（排除预热）
    pub min_us: f64,   // 最小 RTT
    pub avg_us: f64,   // 平均 RTT
    pub p50_us: f64,   // 中位数 RTT（50th 百分位）
    pub p95_us: f64,   // 95th 百分位 RTT
    pub p99_us: f64,   // 99th 百分位 RTT
    pub max_us: f64,   // 最大 RTT
}

/// 吞吐量统计结果（来自 Phase 2 单向发送测试）
#[derive(Debug, Default)]
pub struct ThroughputStats {
    pub duration_secs: f64,   // 测试实际持续时间（秒）
    pub messages: u64,        // server 实际收到的消息总数
    pub msgs_per_sec: f64,    // 消息吞吐率（条/秒）
    pub mb_per_sec: f64,      // 带宽（MB/秒）
}

/// 基准测试汇总结果
///
/// latency 和 throughput 都是 Option，因为：
/// - bench 模式会同时产生两者
/// - 未来可能支持只跑其中一个阶段
#[derive(Debug, Default)]
pub struct BenchResults {
    pub latency: Option<LatencyStats>,
    pub throughput: Option<ThroughputStats>,
}

/// 将基准测试结果格式化输出到终端
///
/// 输出两个部分（如果有的话）：
/// 1. 延迟统计：消息数、min/avg/p50/p95/p99/max
/// 2. 吞吐量统计：持续时间、消息数、msgs/sec、MB/sec
pub fn print_results(results: &BenchResults, warmup: u64) {
    println!("=== Rusteron UDP Benchmark ===");
    println!("Message size: 64 bytes | Warmup: {} msgs", warmup);

    // 输出延迟统计
    if let Some(lat) = &results.latency {
        println!();
        println!("--- Phase 1: Latency (Ping-Pong) ---");
        println!("  Messages:  {}", format_count(lat.count));
        println!("  Min:       {:.1} us", lat.min_us);
        println!("  Avg:       {:.1} us", lat.avg_us);
        println!("  P50:       {:.1} us", lat.p50_us);
        println!("  P95:       {:.1} us", lat.p95_us);
        println!("  P99:       {:.1} us", lat.p99_us);
        println!("  Max:       {:.1} us", lat.max_us);
    }

    // 输出吞吐量统计
    if let Some(tp) = &results.throughput {
        println!();
        println!("--- Phase 2: Throughput (Unidirectional) ---");
        println!("  Duration:  {:.2} s", tp.duration_secs);
        println!("  Messages:  {}", format_count(tp.messages));
        println!("  Throughput: {} msgs/sec", format_count(tp.msgs_per_sec as u64));
        println!("  Bandwidth:  {:.1} MB/sec", tp.mb_per_sec);
    }
}

/// 数字格式化：添加千位分隔符
///
/// 例如：1000000 → "1,000,000"
/// 实现方式：将数字字符串反转，每 3 位插入逗号，再反转回来
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_count_small_numbers() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(1), "1");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn format_count_thousands() {
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(12_345), "12,345");
        assert_eq!(format_count(999_999), "999,999");
    }

    #[test]
    fn format_count_millions() {
        assert_eq!(format_count(1_000_000), "1,000,000");
        assert_eq!(format_count(38_421_000), "38,421,000");
    }

    #[test]
    fn latency_stats_default() {
        let stats = LatencyStats::default();
        assert_eq!(stats.count, 0);
        assert_eq!(stats.min_us, 0.0);
        assert_eq!(stats.avg_us, 0.0);
    }

    #[test]
    fn throughput_stats_default() {
        let stats = ThroughputStats::default();
        assert_eq!(stats.messages, 0);
        assert_eq!(stats.msgs_per_sec, 0.0);
        assert_eq!(stats.mb_per_sec, 0.0);
    }

    #[test]
    fn bench_results_with_latency_only() {
        let results = BenchResults {
            latency: Some(LatencyStats {
                count: 90_000,
                min_us: 5.2,
                avg_us: 9.8,
                p50_us: 9.1,
                p95_us: 12.3,
                p99_us: 15.7,
                max_us: 42.1,
            }),
            throughput: None,
        };
        assert!(results.latency.is_some());
        assert!(results.throughput.is_none());
        assert_eq!(results.latency.as_ref().unwrap().count, 90_000);
    }

    #[test]
    fn bench_results_with_throughput_only() {
        let results = BenchResults {
            latency: None,
            throughput: Some(ThroughputStats {
                duration_secs: 10.0,
                messages: 38_421_000,
                msgs_per_sec: 3_842_100.0,
                mb_per_sec: 234.5,
            }),
        };
        assert!(results.latency.is_none());
        assert!(results.throughput.is_some());
        let tp = results.throughput.as_ref().unwrap();
        assert_eq!(tp.messages, 38_421_000);
    }

    #[test]
    fn print_results_does_not_panic() {
        // Verify print_results handles all combinations without panicking
        let full = BenchResults {
            latency: Some(LatencyStats {
                count: 1000,
                min_us: 1.0,
                avg_us: 5.0,
                p50_us: 4.0,
                p95_us: 8.0,
                p99_us: 10.0,
                max_us: 20.0,
            }),
            throughput: Some(ThroughputStats {
                duration_secs: 1.0,
                messages: 1_000_000,
                msgs_per_sec: 1_000_000.0,
                mb_per_sec: 61.0,
            }),
        };
        print_results(&full, 100);

        let empty = BenchResults::default();
        print_results(&empty, 0);
    }
}
