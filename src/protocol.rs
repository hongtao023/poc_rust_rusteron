//! 基准测试通信协议
//!
//! 定义了固定 64 字节的消息格式，用于 client 和 server 之间的通信。
//! 消息布局（小端序）：
//!
//! | 偏移  | 长度  | 字段          | 说明                           |
//! |-------|-------|---------------|-------------------------------|
//! | 0     | 1     | msg_type      | 消息类型（Ping/Pong/Data/Control）|
//! | 1     | 1     | control_code  | 控制码（仅 Control 类型使用）     |
//! | 2-3   | 2     | padding       | 对齐填充                        |
//! | 4-7   | 4     | sequence      | 序列号（u32 小端）               |
//! | 8-15  | 8     | timestamp_ns  | 时间戳，纳秒（u64 小端）          |
//! | 16-63 | 48    | payload       | 载荷数据                        |

/// 消息类型枚举
/// - Ping:    客户端发出的延迟探测请求
/// - Pong:    服务端对 Ping 的回复
/// - Data:    吞吐量测试的数据包（单向，client→server）
/// - Control: 控制指令（开始/停止吞吐量测试、请求/返回统计报告）
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    Ping = 1,
    Pong = 2,
    Data = 3,
    Control = 4,
}

impl MsgType {
    /// 从 u8 字节解析消息类型，未知值默认为 Data
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => MsgType::Ping,
            2 => MsgType::Pong,
            3 => MsgType::Data,
            4 => MsgType::Control,
            _ => MsgType::Data,
        }
    }
}

/// 控制码枚举（仅在 MsgType::Control 消息中使用）
/// - StartThroughput: 通知 server 开始计数吞吐量
/// - StopThroughput:  通知 server 停止计数
/// - ReportRequest:   请求 server 返回吞吐量统计
/// - ReportResponse:  server 返回的统计结果
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCode {
    None = 0,
    StartThroughput = 1,
    StopThroughput = 2,
    ReportRequest = 3,
    ReportResponse = 4,
}

impl ControlCode {
    /// 从 u8 字节解析控制码，未知值默认为 None
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => ControlCode::StartThroughput,
            2 => ControlCode::StopThroughput,
            3 => ControlCode::ReportRequest,
            4 => ControlCode::ReportResponse,
            _ => ControlCode::None,
        }
    }
}

/// 固定消息大小：64 字节
pub const MESSAGE_SIZE: usize = 64;

/// 64 字节定长基准测试消息
///
/// 使用固定大小是为了避免动态分配，方便直接写入 Aeron 的发送缓冲区。
/// 48 字节的 payload 在当前实现中未使用，预留给未来扩展（如不同大小的负载测试）。
#[derive(Debug, Clone)]
pub struct BenchMessage {
    pub msg_type: MsgType,          // 消息类型
    pub control_code: ControlCode,  // 控制码
    pub sequence: u32,              // 序列号
    pub timestamp_ns: u64,          // 时间戳（纳秒）
    pub payload: [u8; 48],          // 载荷（48 字节）
}

impl BenchMessage {
    /// 创建一个普通消息（Ping/Pong/Data），控制码为 None
    pub fn new(msg_type: MsgType, sequence: u32) -> Self {
        Self {
            msg_type,
            control_code: ControlCode::None,
            sequence,
            timestamp_ns: 0,
            payload: [0u8; 48],
        }
    }

    /// 创建一个控制消息，消息类型自动设为 Control
    pub fn control(code: ControlCode, sequence: u32) -> Self {
        Self {
            msg_type: MsgType::Control,
            control_code: code,
            sequence,
            timestamp_ns: 0,
            payload: [0u8; 48],
        }
    }

    /// 将消息序列化写入 64 字节缓冲区（小端字节序）
    ///
    /// 布局：[0] msg_type | [1] control_code | [2-3] 填充 |
    ///       [4-7] sequence(LE) | [8-15] timestamp_ns(LE) | [16-63] payload
    pub fn write_to(&self, buf: &mut [u8]) {
        assert!(buf.len() >= MESSAGE_SIZE);
        buf[0] = self.msg_type as u8;
        buf[1] = self.control_code as u8;
        buf[2] = 0; // 填充字节
        buf[3] = 0; // 填充字节
        buf[4..8].copy_from_slice(&self.sequence.to_le_bytes());
        buf[8..16].copy_from_slice(&self.timestamp_ns.to_le_bytes());
        buf[16..64].copy_from_slice(&self.payload);
    }

    /// 从 64 字节缓冲区反序列化读取消息
    pub fn read_from(buf: &[u8]) -> Self {
        assert!(buf.len() >= MESSAGE_SIZE);
        Self {
            msg_type: MsgType::from_u8(buf[0]),
            control_code: ControlCode::from_u8(buf[1]),
            sequence: u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            timestamp_ns: u64::from_le_bytes([
                buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15],
            ]),
            payload: {
                let mut p = [0u8; 48];
                p.copy_from_slice(&buf[16..64]);
                p
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_type_round_trip() {
        assert_eq!(MsgType::from_u8(1), MsgType::Ping);
        assert_eq!(MsgType::from_u8(2), MsgType::Pong);
        assert_eq!(MsgType::from_u8(3), MsgType::Data);
        assert_eq!(MsgType::from_u8(4), MsgType::Control);
        // Unknown maps to Data
        assert_eq!(MsgType::from_u8(0), MsgType::Data);
        assert_eq!(MsgType::from_u8(255), MsgType::Data);
    }

    #[test]
    fn control_code_round_trip() {
        assert_eq!(ControlCode::from_u8(0), ControlCode::None);
        assert_eq!(ControlCode::from_u8(1), ControlCode::StartThroughput);
        assert_eq!(ControlCode::from_u8(2), ControlCode::StopThroughput);
        assert_eq!(ControlCode::from_u8(3), ControlCode::ReportRequest);
        assert_eq!(ControlCode::from_u8(4), ControlCode::ReportResponse);
        // Unknown maps to None
        assert_eq!(ControlCode::from_u8(255), ControlCode::None);
    }

    #[test]
    fn bench_message_serialize_round_trip_ping() {
        let msg = BenchMessage::new(MsgType::Ping, 42);
        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);

        let decoded = BenchMessage::read_from(&buf);
        assert_eq!(decoded.msg_type, MsgType::Ping);
        assert_eq!(decoded.control_code, ControlCode::None);
        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.timestamp_ns, 0);
        assert_eq!(decoded.payload, [0u8; 48]);
    }

    #[test]
    fn bench_message_serialize_round_trip_control() {
        let mut msg = BenchMessage::control(ControlCode::ReportResponse, 99);
        msg.timestamp_ns = 123_456_789;

        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);

        let decoded = BenchMessage::read_from(&buf);
        assert_eq!(decoded.msg_type, MsgType::Control);
        assert_eq!(decoded.control_code, ControlCode::ReportResponse);
        assert_eq!(decoded.sequence, 99);
        assert_eq!(decoded.timestamp_ns, 123_456_789);
    }

    #[test]
    fn bench_message_preserves_payload() {
        let mut msg = BenchMessage::new(MsgType::Data, 1);
        for i in 0..48 {
            msg.payload[i] = i as u8;
        }

        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);

        let decoded = BenchMessage::read_from(&buf);
        for i in 0..48 {
            assert_eq!(decoded.payload[i], i as u8, "payload mismatch at index {}", i);
        }
    }

    #[test]
    fn bench_message_preserves_timestamp() {
        let mut msg = BenchMessage::new(MsgType::Pong, 0);
        msg.timestamp_ns = u64::MAX;

        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);

        let decoded = BenchMessage::read_from(&buf);
        assert_eq!(decoded.timestamp_ns, u64::MAX);
    }

    #[test]
    fn bench_message_preserves_max_sequence() {
        let msg = BenchMessage::new(MsgType::Ping, u32::MAX);
        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);

        let decoded = BenchMessage::read_from(&buf);
        assert_eq!(decoded.sequence, u32::MAX);
    }

    #[test]
    fn message_size_is_64_bytes() {
        assert_eq!(MESSAGE_SIZE, 64);
        let msg = BenchMessage::new(MsgType::Ping, 0);
        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);
        // Buffer is exactly 64 bytes
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn wire_format_layout() {
        let mut msg = BenchMessage::new(MsgType::Ping, 0x04030201);
        msg.timestamp_ns = 0x0807060504030201;
        let mut buf = [0u8; MESSAGE_SIZE];
        msg.write_to(&mut buf);

        // msg_type at byte 0
        assert_eq!(buf[0], 1); // Ping
        // control_code at byte 1
        assert_eq!(buf[1], 0); // None
        // padding at bytes 2-3
        assert_eq!(buf[2], 0);
        assert_eq!(buf[3], 0);
        // sequence at bytes 4-7 (little-endian)
        assert_eq!(buf[4], 0x01);
        assert_eq!(buf[5], 0x02);
        assert_eq!(buf[6], 0x03);
        assert_eq!(buf[7], 0x04);
        // timestamp_ns at bytes 8-15 (little-endian)
        assert_eq!(buf[8], 0x01);
        assert_eq!(buf[15], 0x08);
    }

    #[test]
    #[should_panic]
    fn write_to_panics_on_short_buffer() {
        let msg = BenchMessage::new(MsgType::Ping, 0);
        let mut buf = [0u8; 32];
        msg.write_to(&mut buf);
    }

    #[test]
    #[should_panic]
    fn read_from_panics_on_short_buffer() {
        let buf = [0u8; 32];
        BenchMessage::read_from(&buf);
    }
}
