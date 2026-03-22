//! Aeron Media Driver 封装模块
//!
//! 提供以下功能：
//! 1. 启动内嵌的 Aeron Media Driver（在独立线程中运行）
//! 2. 创建 Aeron 客户端实例并连接到 Driver
//! 3. 创建 Publication（发布端）和 Subscription（订阅端）
//! 4. 带背压重试的消息发送（offer_with_retry）

use rusteron_client::{Aeron, AeronContext, AeronPublication, AeronSubscription};
use rusteron_media_driver::{AeronCError, AeronDriver, AeronDriverContext};
use std::ffi::CString;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 连接超时时间：等待 Publication/Subscription 建立连接的最大时长
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 内嵌 Aeron Media Driver 的封装
///
/// Media Driver 是 Aeron 的核心组件，负责管理 UDP 传输。
/// 它在一个独立线程中以 busy-spin 方式运行，通过共享内存（mmap 文件）
/// 与 Aeron 客户端通信。
///
/// 字段说明：
/// - stop:   原子布尔标志，设为 true 时通知 Driver 线程退出
/// - handle: Driver 线程的 JoinHandle，用于等待线程结束
/// - dir:    Driver 使用的共享内存目录路径（如 /tmp/aeron-bench-xxx）
pub struct EmbeddedDriver {
    pub stop: Arc<AtomicBool>,
    pub handle: Option<JoinHandle<Result<(), AeronCError>>>,
    pub dir: String,
}

impl EmbeddedDriver {
    /// 启动内嵌 Media Driver（自动生成唯一临时目录）
    ///
    /// 目录格式：/tmp/aeron-bench-{pid}-{counter}
    /// 使用原子计数器确保同一进程内多次调用也不会冲突
    pub fn launch() -> Result<Self, Box<dyn std::error::Error>> {
        use std::sync::atomic::Ordering;
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!("aeron-bench-{}-{}", std::process::id(), id))
            .to_string_lossy()
            .to_string();

        Self::launch_with_dir(&dir)
    }

    /// 使用指定目录启动内嵌 Media Driver
    ///
    /// 流程：创建 DriverContext → 设置共享内存目录 → 在后台线程启动 Driver
    pub fn launch_with_dir(dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let c_dir = CString::new(dir.to_string())?;
        let driver_ctx = AeronDriverContext::new()?;
        driver_ctx.set_dir(&c_dir)?;
        // launch_embedded 会在新线程中启动 Driver，返回 (stop_flag, thread_handle)
        let (stop, handle) = AeronDriver::launch_embedded(driver_ctx, false);

        Ok(Self {
            stop,
            handle: Some(handle),
            dir: dir.to_string(),
        })
    }

    /// 创建一个 Aeron 客户端并连接到当前 Driver
    ///
    /// 客户端通过共享内存目录（dir）找到 Driver 并建立通信。
    /// 一个 Driver 可以有多个客户端（如 bench 模式下 server 和 client 各一个）。
    pub fn connect(&self) -> Result<Aeron, Box<dyn std::error::Error>> {
        let c_dir = CString::new(self.dir.clone())?;
        let ctx = AeronContext::new()?;
        ctx.set_dir(&c_dir)?;
        let aeron = Aeron::new(&ctx)?;
        aeron.start()?;
        Ok(aeron)
    }

    /// 发送停止信号给 Driver 线程
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Drop 实现：对象销毁时自动停止 Driver 并等待线程结束
impl Drop for EmbeddedDriver {
    fn drop(&mut self) {
        self.stop();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 创建一个 Publication（发布端）并等待连接建立
///
/// Publication 用于向指定 endpoint + stream_id 发送消息。
/// Aeron URI 格式：aeron:udp?endpoint=host:port
///
/// 参数：
/// - aeron:     Aeron 客户端实例
/// - endpoint:  目标 UDP 地址，如 "10.0.0.100:20121"
/// - stream_id: 流 ID，用于在同一 endpoint 上复用多个逻辑通道
pub fn add_publication(
    aeron: &Aeron,
    endpoint: &str,
    stream_id: i32,
) -> Result<AeronPublication, Box<dyn std::error::Error>> {
    add_publication_inner(aeron, endpoint, stream_id, true)
}

/// 创建 Publication，可选择是否等待对端连接
pub fn add_publication_no_wait(
    aeron: &Aeron,
    endpoint: &str,
    stream_id: i32,
) -> Result<AeronPublication, Box<dyn std::error::Error>> {
    add_publication_inner(aeron, endpoint, stream_id, false)
}

fn add_publication_inner(
    aeron: &Aeron,
    endpoint: &str,
    stream_id: i32,
    wait_connected: bool,
) -> Result<AeronPublication, Box<dyn std::error::Error>> {
    let uri = format!("aeron:udp?endpoint={}", endpoint);
    let c_uri = CString::new(uri)?;
    let publication = aeron.add_publication(&c_uri, stream_id, CONNECT_TIMEOUT)?;

    if wait_connected {
        let start = Instant::now();
        while !publication.is_connected() {
            if start.elapsed() > CONNECT_TIMEOUT {
                return Err("Publication connection timeout".into());
            }
            std::hint::spin_loop();
        }
    }
    Ok(publication)
}

/// 创建一个 Subscription（订阅端）并等待连接建立
///
/// Subscription 用于从指定 endpoint + stream_id 接收消息。
/// 连接建立后，可通过 poll() 拉取收到的消息。
///
/// 参数同 add_publication
pub fn add_subscription(
    aeron: &Aeron,
    endpoint: &str,
    stream_id: i32,
) -> Result<AeronSubscription, Box<dyn std::error::Error>> {
    add_subscription_inner(aeron, endpoint, stream_id, true)
}

/// 创建 Subscription，不等待对端连接
pub fn add_subscription_no_wait(
    aeron: &Aeron,
    endpoint: &str,
    stream_id: i32,
) -> Result<AeronSubscription, Box<dyn std::error::Error>> {
    add_subscription_inner(aeron, endpoint, stream_id, false)
}

fn add_subscription_inner(
    aeron: &Aeron,
    endpoint: &str,
    stream_id: i32,
    wait_connected: bool,
) -> Result<AeronSubscription, Box<dyn std::error::Error>> {
    let uri = format!("aeron:udp?endpoint={}", endpoint);
    let c_uri = CString::new(uri)?;
    let subscription =
        aeron.add_subscription::<rusteron_client::AeronAvailableImageLogger, rusteron_client::AeronUnavailableImageLogger>(
            &c_uri,
            stream_id,
            None,
            None,
            CONNECT_TIMEOUT,
        )?;

    if wait_connected {
        let start = Instant::now();
        while !subscription.is_connected() {
            if start.elapsed() > CONNECT_TIMEOUT {
                return Err("Subscription connection timeout".into());
            }
            std::hint::spin_loop();
        }
    }
    Ok(subscription)
}

/// 向 Publication 发送数据，遇到背压时自旋重试
///
/// Aeron 的 offer() 在发送缓冲区满时会返回负值（背压信号），
/// 此函数会持续重试直到发送成功，适用于基准测试场景。
/// 注意：生产环境中应添加超时或退避策略，避免无限自旋。
pub fn offer_with_retry(publication: &AeronPublication, buf: &[u8]) {
    loop {
        let result =
            publication.offer::<rusteron_client::AeronReservedValueSupplierLogger>(buf, None);
        if result > 0 {
            return; // 发送成功，返回
        }
        // 背压：发送缓冲区已满，自旋等待后重试
        std::hint::spin_loop();
    }
}
