//! rusteron-bench 库入口
//! 本 crate 实现了一个基于 Aeron 的 UDP 基准测试工具，
//! 包含延迟（Ping-Pong）和吞吐量（单向发送）两个测试阶段。

pub mod bench;        // 单进程模式：在同一进程内同时运行 server 和 client
pub mod client;       // 客户端：发送 Ping 测延迟，发送 Data 测吞吐量
pub mod driver;       // Aeron Media Driver 封装：启动、连接、发布/订阅
pub mod protocol;     // 通信协议（手写）：64 字节定长消息的序列化/反序列化
pub mod protocol_sbe; // 通信协议（SBE）：基于 XML schema 生成的类型安全编解码器
pub mod server;       // 服务端：回复 Pong、计数 Data、响应 Control 指令
pub mod stats;        // 统计与输出：延迟直方图 + 吞吐量报告
