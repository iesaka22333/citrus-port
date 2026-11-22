// ============================================================================
// TX 编码层：文本 + 编码 -> 原始字节。
//
// 只负责把用户输入的文本按指定编码转换成 bytes，不涉及串口 I/O、
// 换行拼接、HEX 解析或 UI。HEX 发送路径永远不会经过这里。
//
// Encoding 是稳定的核心枚举，UI 文案（中文/英文/其他语言）不能反向
// 影响后端；后端只依赖 enum，不依赖显示文本。
// ============================================================================

use crate::serial::error::SerialError;

/// 后端核心编码枚举，与 UI 显示文案解耦。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Gbk,
    Ascii,
}

impl Encoding {
    /// 根据 UI 传入的编码字符串（`UTF-8` / `GBK` / `ASCII`）解析，
    /// 未知值回退为 UTF-8。仅作为兼容入口，内部始终使用枚举。
    pub fn from_str(s: &str) -> Self {
        match s {
            "GBK" => Encoding::Gbk,
            "ASCII" => Encoding::Ascii,
            _ => Encoding::Utf8,
        }
    }

    /// 返回对应的 encoding_rs 编码（ASCII 无 streaming 编码，用 UTF-8 占位，
    /// 由解码层单独处理 ASCII 分支，不使用此返回值）。
    pub(crate) fn encoding_rs(&self) -> &'static encoding_rs::Encoding {
        match self {
            Encoding::Utf8 => encoding_rs::UTF_8,
            Encoding::Gbk => encoding_rs::GBK,
            Encoding::Ascii => encoding_rs::UTF_8,
        }
    }
}

/// 将文本按指定编码转换为原始字节。
///
/// - UTF-8：Rust `String` 天然是 UTF-8，直接取字节。
/// - GBK：使用 encoding_rs 的 GBK 编码，真正输出 C4 E3 BA C3 等 GBK 字节。
/// - ASCII：0x00~0x7F 正常编码，遇到非 ASCII 字符返回明确错误而非静默替换。
pub fn encode_text(text: &str, encoding: Encoding) -> Result<Vec<u8>, SerialError> {
    match encoding {
        Encoding::Utf8 => {
            // Rust 字符串本身就是 UTF-8，无需转换，零开销且语义精确。
            Ok(text.as_bytes().to_vec())
        }
        Encoding::Gbk => {
            // GBK 编码：无法表示的字符由 encoding_rs 用替换字符处理，
            // 属于标准的容错策略，不视为致命错误。
            let (bytes, _actual, _had_errors) = encoding_rs::GBK.encode(text);
            Ok(bytes.into_owned())
        }
        Encoding::Ascii => {
            let mut out = Vec::with_capacity(text.len());
            for c in text.chars() {
                if c.is_ascii() {
                    out.push(c as u8);
                } else {
                    // 明确、稳定的错误策略：非 ASCII 字符不静默转成 UTF-8。
                    return Err(SerialError::Encoding(format!("无法以 ASCII 编码字符: {c}")));
                }
            }
            Ok(out)
        }
    }
}
