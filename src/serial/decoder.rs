// ============================================================================
// 流式接收解码器
//
// 解决串口 `read()` 可能把多字节字符（UTF-8 / GBK）拆分到不同 chunk 的问题。
//
// 本结构是有生命周期的状态对象，内部持有 encoding_rs 的 streaming Decoder：
// - 未完成的字节序列会被 decoder 缓存到下一次 feed。
// - 畸形序列按 U+FFFD 替换，不丢数据。
// - 切换编码时重新创建 decoder，避免旧编码残留字节污染新编码。
// ============================================================================

use crate::serial::encoder::Encoding;

/// 有生命周期的流式解码状态。
pub struct ReceiveDecoder {
    encoding: Encoding,
    decoder: encoding_rs::Decoder,
}

impl ReceiveDecoder {
    /// 以初始编码创建解码器。
    pub fn new(encoding: Encoding) -> Self {
        Self {
            encoding,
            decoder: encoding.encoding_rs().new_decoder(),
        }
    }

    /// 重新创建当前编码对应的 decoder，但保留当前编码。
    ///
    /// 用于连接建立/断开、编码切换、HEX<->文本切换时，丢弃可能残留的
    /// partial multibyte sequence，避免旧状态污染新数据。
    pub fn reset(&mut self) {
        self.decoder = self.encoding.encoding_rs().new_decoder();
    }

    /// 喂入一段字节，返回本次可完整解码出的文本。
    ///
    /// 始终使用 `decode_to_string(..., false)`，因为串口是持续数据流，
    /// 不能把每个 read chunk 当作完整字符串做尾字节 flush。
    ///
    /// 当 `encoding` 与当前不一致时自动切换并重置内部状态。
    pub fn feed(&mut self, data: &[u8], encoding: &str) -> String {
        let enc = Encoding::from_str(encoding);
        if enc != self.encoding {
            self.encoding = enc;
            self.decoder = enc.encoding_rs().new_decoder();
        }

        match self.encoding {
            // ASCII 单字节：0x00~0x7F 映射为对应字符；
            // 0x80~0xFF 使用统一替代字符 （U+FFFD），不做 UTF-8 解码，也不 panic。
            Encoding::Ascii => data
                .iter()
                .map(|&b| if b.is_ascii() { b as char } else { '\u{FFFD}' })
                .collect(),
            // UTF-8 / GBK 走 encoding_rs streaming decoder。
            Encoding::Utf8 | Encoding::Gbk => {
                let mut out = String::with_capacity(data.len());
                let (_result, _read, _had_errors) =
                    self.decoder.decode_to_string(data, &mut out, false);
                out
            }
        }
    }
}
