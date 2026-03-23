//! SBE（Simple Binary Encoding）协议实现
//!
//! 基于 schemas/bench.xml 生成的类型 + 手动修正的偏移量。
//! 与 protocol.rs（手写协议）形成对比：
//!
//! | 对比项         | protocol.rs（手写）       | protocol_sbe.rs（SBE）         |
//! |----------------|--------------------------|-------------------------------|
//! | 消息类型       | 单一 BenchMessage         | PingPong / Data / Control 分离 |
//! | 序列化         | 手动管理字节偏移           | 类型安全的 encoder/decoder     |
//! | 消息头         | 无                        | 8 字节 MessageHeader           |
//! | 版本兼容       | 无                        | schema version 内置            |
//! | 枚举处理       | 手写 from_u8              | 自动生成 From<u8>              |
//! | 扩展性         | 改一个字段要改所有偏移      | 改 XML schema 重新生成         |
//!
//! 消息格式（所有消息共享 8 字节 MessageHeader）：
//!
//! MessageHeader (8 bytes):
//! | 偏移 | 长度 | 字段          | 说明                    |
//! |------|------|---------------|------------------------|
//! | 0    | 2    | blockLength   | 消息体长度              |
//! | 2    | 2    | templateId    | 消息类型 ID             |
//! | 4    | 2    | schemaId      | Schema ID               |
//! | 6    | 2    | version       | Schema 版本             |
//!
//! PingPong (templateId=1, blockLength=13):
//! | 偏移 | 长度 | 字段         |
//! |------|------|-------------|
//! | 8    | 1    | msgType     |
//! | 9    | 4    | sequence    |
//! | 13   | 8    | timestampNs |
//!
//! DataMessage (templateId=2, blockLength=12):
//! | 偏移 | 长度 | 字段         |
//! |------|------|-------------|
//! | 8    | 4    | sequence    |
//! | 12   | 8    | timestampNs |
//!
//! ControlMessage (templateId=3, blockLength=13):
//! | 偏移 | 长度 | 字段         |
//! |------|------|-------------|
//! | 8    | 1    | controlCode |
//! | 9    | 4    | sequence    |
//! | 13   | 8    | value       |

// 引入从 XML schema 生成的枚举类型
mod generated {
    include!(concat!(env!("OUT_DIR"), "/bench_sbe.rs"));
}

// 重导出生成的枚举（这些是正确的）
pub use generated::{ControlCode, MsgType, SCHEMA_ID, SCHEMA_VERSION};

use ironsbe_core::buffer::{ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;

/// SBE 消息缓冲区大小（header 8 + body max 21 = 29，对齐到 32）
pub const SBE_BUF_SIZE: usize = 32;

// =====================================================================
// PingPong 消息 (templateId = 1)
// =====================================================================

const PINGPONG_TEMPLATE_ID: u16 = 1;
const PINGPONG_BLOCK_LENGTH: u16 = 13; // 1 + 4 + 8

/// PingPong 编码器 — 类型安全的消息构造
///
/// 对比手写协议：
/// ```ignore
/// // 手写（protocol.rs）— 容易写错偏移
/// buf[0] = msg.msg_type as u8;
/// buf[4..8].copy_from_slice(&msg.sequence.to_le_bytes());
///
/// // SBE — 类型安全，不可能写错偏移
/// encoder.set_msg_type(MsgType::Ping).set_sequence(42);
/// ```
pub struct PingPongEncoder<'a> {
    buffer: &'a mut [u8],
    offset: usize,
}

impl<'a> PingPongEncoder<'a> {
    /// 包装缓冲区并写入 MessageHeader
    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {
        let header = MessageHeader {
            block_length: PINGPONG_BLOCK_LENGTH,
            template_id: PINGPONG_TEMPLATE_ID,
            schema_id: SCHEMA_ID,
            version: SCHEMA_VERSION,
        };
        header.encode(buffer, offset);
        Self { buffer, offset }
    }

    /// 编码后的总长度（header + body）
    pub const fn encoded_length(&self) -> usize {
        MessageHeader::ENCODED_LENGTH + PINGPONG_BLOCK_LENGTH as usize
    }

    #[inline(always)]
    pub fn set_msg_type(&mut self, value: MsgType) -> &mut Self {
        self.buffer
            .put_u8(self.offset + MessageHeader::ENCODED_LENGTH, u8::from(value));
        self
    }

    #[inline(always)]
    pub fn set_sequence(&mut self, value: u32) -> &mut Self {
        self.buffer
            .put_u32_le(self.offset + MessageHeader::ENCODED_LENGTH + 1, value);
        self
    }

    #[inline(always)]
    pub fn set_timestamp_ns(&mut self, value: u64) -> &mut Self {
        self.buffer
            .put_u64_le(self.offset + MessageHeader::ENCODED_LENGTH + 5, value);
        self
    }
}

/// PingPong 解码器 — 零拷贝读取
pub struct PingPongDecoder<'a> {
    buffer: &'a [u8],
    offset: usize,
}

impl<'a> PingPongDecoder<'a> {
    pub fn wrap(buffer: &'a [u8], offset: usize) -> Self {
        Self { buffer, offset }
    }

    #[inline(always)]
    pub fn msg_type(&self) -> MsgType {
        MsgType::from(self.buffer.get_u8(self.offset + MessageHeader::ENCODED_LENGTH))
    }

    #[inline(always)]
    pub fn sequence(&self) -> u32 {
        self.buffer
            .get_u32_le(self.offset + MessageHeader::ENCODED_LENGTH + 1)
    }

    #[inline(always)]
    pub fn timestamp_ns(&self) -> u64 {
        self.buffer
            .get_u64_le(self.offset + MessageHeader::ENCODED_LENGTH + 5)
    }
}

// =====================================================================
// DataMessage (templateId = 2)
// =====================================================================

const DATA_TEMPLATE_ID: u16 = 2;
const DATA_BLOCK_LENGTH: u16 = 12; // 4 + 8

pub struct DataMessageEncoder<'a> {
    buffer: &'a mut [u8],
    offset: usize,
}

impl<'a> DataMessageEncoder<'a> {
    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {
        let header = MessageHeader {
            block_length: DATA_BLOCK_LENGTH,
            template_id: DATA_TEMPLATE_ID,
            schema_id: SCHEMA_ID,
            version: SCHEMA_VERSION,
        };
        header.encode(buffer, offset);
        Self { buffer, offset }
    }

    pub const fn encoded_length(&self) -> usize {
        MessageHeader::ENCODED_LENGTH + DATA_BLOCK_LENGTH as usize
    }

    #[inline(always)]
    pub fn set_sequence(&mut self, value: u32) -> &mut Self {
        self.buffer
            .put_u32_le(self.offset + MessageHeader::ENCODED_LENGTH, value);
        self
    }

    #[inline(always)]
    pub fn set_timestamp_ns(&mut self, value: u64) -> &mut Self {
        self.buffer
            .put_u64_le(self.offset + MessageHeader::ENCODED_LENGTH + 4, value);
        self
    }
}

pub struct DataMessageDecoder<'a> {
    buffer: &'a [u8],
    offset: usize,
}

impl<'a> DataMessageDecoder<'a> {
    pub fn wrap(buffer: &'a [u8], offset: usize) -> Self {
        Self { buffer, offset }
    }

    #[inline(always)]
    pub fn sequence(&self) -> u32 {
        self.buffer
            .get_u32_le(self.offset + MessageHeader::ENCODED_LENGTH)
    }

    #[inline(always)]
    pub fn timestamp_ns(&self) -> u64 {
        self.buffer
            .get_u64_le(self.offset + MessageHeader::ENCODED_LENGTH + 4)
    }
}

// =====================================================================
// ControlMessage (templateId = 3)
// =====================================================================

const CONTROL_TEMPLATE_ID: u16 = 3;
const CONTROL_BLOCK_LENGTH: u16 = 13; // 1 + 4 + 8

pub struct ControlMessageEncoder<'a> {
    buffer: &'a mut [u8],
    offset: usize,
}

impl<'a> ControlMessageEncoder<'a> {
    pub fn wrap(buffer: &'a mut [u8], offset: usize) -> Self {
        let header = MessageHeader {
            block_length: CONTROL_BLOCK_LENGTH,
            template_id: CONTROL_TEMPLATE_ID,
            schema_id: SCHEMA_ID,
            version: SCHEMA_VERSION,
        };
        header.encode(buffer, offset);
        Self { buffer, offset }
    }

    pub const fn encoded_length(&self) -> usize {
        MessageHeader::ENCODED_LENGTH + CONTROL_BLOCK_LENGTH as usize
    }

    #[inline(always)]
    pub fn set_control_code(&mut self, value: ControlCode) -> &mut Self {
        self.buffer
            .put_u8(self.offset + MessageHeader::ENCODED_LENGTH, u8::from(value));
        self
    }

    #[inline(always)]
    pub fn set_sequence(&mut self, value: u32) -> &mut Self {
        self.buffer
            .put_u32_le(self.offset + MessageHeader::ENCODED_LENGTH + 1, value);
        self
    }

    #[inline(always)]
    pub fn set_value(&mut self, value: u64) -> &mut Self {
        self.buffer
            .put_u64_le(self.offset + MessageHeader::ENCODED_LENGTH + 5, value);
        self
    }
}

pub struct ControlMessageDecoder<'a> {
    buffer: &'a [u8],
    offset: usize,
}

impl<'a> ControlMessageDecoder<'a> {
    pub fn wrap(buffer: &'a [u8], offset: usize) -> Self {
        Self { buffer, offset }
    }

    #[inline(always)]
    pub fn control_code(&self) -> ControlCode {
        ControlCode::from(self.buffer.get_u8(self.offset + MessageHeader::ENCODED_LENGTH))
    }

    #[inline(always)]
    pub fn sequence(&self) -> u32 {
        self.buffer
            .get_u32_le(self.offset + MessageHeader::ENCODED_LENGTH + 1)
    }

    #[inline(always)]
    pub fn value(&self) -> u64 {
        self.buffer
            .get_u64_le(self.offset + MessageHeader::ENCODED_LENGTH + 5)
    }
}

// =====================================================================
// 消息分发 — 通过 MessageHeader 的 templateId 自动路由
// =====================================================================

/// 解码消息头，返回 templateId 用于消息分发
///
/// 对比手写协议中的 `match msg.msg_type`，SBE 通过 header 的 templateId
/// 在解码消息体之前就能确定消息类型，更安全。
pub fn decode_header(buffer: &[u8]) -> MessageHeader {
    MessageHeader::wrap(buffer, 0)
}

/// 从 header 中获取消息类型
///
/// ```ignore
/// let header = decode_header(&buffer);
/// match header.template_id {
///     1 => { let msg = PingPongDecoder::wrap(&buffer, 0); ... }
///     2 => { let msg = DataMessageDecoder::wrap(&buffer, 0); ... }
///     3 => { let msg = ControlMessageDecoder::wrap(&buffer, 0); ... }
///     _ => { /* unknown message, forward compatible */ }
/// }
/// ```
pub const TEMPLATE_PINGPONG: u16 = PINGPONG_TEMPLATE_ID;
pub const TEMPLATE_DATA: u16 = DATA_TEMPLATE_ID;
pub const TEMPLATE_CONTROL: u16 = CONTROL_TEMPLATE_ID;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pingpong_roundtrip() {
        let mut buf = [0u8; SBE_BUF_SIZE];
        let mut encoder = PingPongEncoder::wrap(&mut buf, 0);
        encoder
            .set_msg_type(MsgType::Ping)
            .set_sequence(42)
            .set_timestamp_ns(123_456_789);

        let header = decode_header(&buf);
        let tid = { header.template_id };
        let sid = { header.schema_id };
        let ver = { header.version };
        assert_eq!(tid, TEMPLATE_PINGPONG);
        assert_eq!(sid, SCHEMA_ID);
        assert_eq!(ver, SCHEMA_VERSION);

        let decoder = PingPongDecoder::wrap(&buf, 0);
        assert_eq!(decoder.msg_type(), MsgType::Ping);
        assert_eq!(decoder.sequence(), 42);
        assert_eq!(decoder.timestamp_ns(), 123_456_789);
    }

    #[test]
    fn pong_roundtrip() {
        let mut buf = [0u8; SBE_BUF_SIZE];
        let mut encoder = PingPongEncoder::wrap(&mut buf, 0);
        encoder
            .set_msg_type(MsgType::Pong)
            .set_sequence(99)
            .set_timestamp_ns(u64::MAX);

        let decoder = PingPongDecoder::wrap(&buf, 0);
        assert_eq!(decoder.msg_type(), MsgType::Pong);
        assert_eq!(decoder.sequence(), 99);
        assert_eq!(decoder.timestamp_ns(), u64::MAX);
    }

    #[test]
    fn data_message_roundtrip() {
        let mut buf = [0u8; SBE_BUF_SIZE];
        let mut encoder = DataMessageEncoder::wrap(&mut buf, 0);
        encoder.set_sequence(1000).set_timestamp_ns(555_555);

        let header = decode_header(&buf);
        let tid = { header.template_id };
        assert_eq!(tid, TEMPLATE_DATA);

        let decoder = DataMessageDecoder::wrap(&buf, 0);
        assert_eq!(decoder.sequence(), 1000);
        assert_eq!(decoder.timestamp_ns(), 555_555);
    }

    #[test]
    fn control_message_roundtrip() {
        let mut buf = [0u8; SBE_BUF_SIZE];
        let mut encoder = ControlMessageEncoder::wrap(&mut buf, 0);
        encoder
            .set_control_code(ControlCode::ReportResponse)
            .set_sequence(7)
            .set_value(38_421_000);

        let header = decode_header(&buf);
        let tid = { header.template_id };
        assert_eq!(tid, TEMPLATE_CONTROL);

        let decoder = ControlMessageDecoder::wrap(&buf, 0);
        assert_eq!(decoder.control_code(), ControlCode::ReportResponse);
        assert_eq!(decoder.sequence(), 7);
        assert_eq!(decoder.value(), 38_421_000);
    }

    #[test]
    fn all_control_codes() {
        for code in [
            ControlCode::None,
            ControlCode::StartThroughput,
            ControlCode::StopThroughput,
            ControlCode::ReportRequest,
            ControlCode::ReportResponse,
        ] {
            let mut buf = [0u8; SBE_BUF_SIZE];
            let mut encoder = ControlMessageEncoder::wrap(&mut buf, 0);
            encoder.set_control_code(code);

            let decoder = ControlMessageDecoder::wrap(&buf, 0);
            assert_eq!(decoder.control_code(), code);
        }
    }

    #[test]
    fn message_dispatch_by_template_id() {
        // 演示：SBE 通过 header templateId 分发消息
        let mut buf = [0u8; SBE_BUF_SIZE];

        // 编码一个 PingPong
        PingPongEncoder::wrap(&mut buf, 0).set_msg_type(MsgType::Ping);

        // 解码时先读 header
        let header = decode_header(&buf);
        let result = match header.template_id {
            TEMPLATE_PINGPONG => {
                let d = PingPongDecoder::wrap(&buf, 0);
                format!("PingPong: {:?}", d.msg_type())
            }
            TEMPLATE_DATA => "Data".to_string(),
            TEMPLATE_CONTROL => "Control".to_string(),
            _ => "Unknown".to_string(),
        };
        assert_eq!(result, "PingPong: Ping");
    }
}
