// ============================================================================
// 数据格式化与转换工具
//
// 这些函数属于「数据处理层」，只负责字节 <-> 文本 / HEX / 时间戳之间的转换，
// 不涉及任何串口 I/O 与 Slint UI 操作。
// ============================================================================

use crate::serial::error::SerialError;

/// 将字节序列格式化成大写、空格分隔的十六进制文本，如 `48 65 6C 6C 6F 0D 0A`。
pub fn hex_string(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 3);
    for (i, b) in data.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// 将十六进制文本解析为原始字节。
///
/// - 允许空格与任意空白分隔。
/// - 大小写十六进制均可。
/// - 奇数个十六进制字符、非法字符返回明确错误，不 panic。
pub fn parse_hex(text: &str) -> Result<Vec<u8>, SerialError> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if cleaned.len() % 2 != 0 {
        return Err(SerialError::InvalidHex(
            "十六进制数据长度必须为偶数".to_string(),
        ));
    }

    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let mut chars = cleaned.chars();
    while let (Some(hi), Some(lo)) = (chars.next(), chars.next()) {
        let hi = match hi.to_digit(16) {
            Some(v) => v as u8,
            None => return Err(SerialError::InvalidHex(format!("非法十六进制字符: {hi}"))),
        };
        let lo = match lo.to_digit(16) {
            Some(v) => v as u8,
            None => return Err(SerialError::InvalidHex(format!("非法十六进制字符: {lo}"))),
        };
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

/// 根据 UI 的换行选项返回对应的字节序列。
///
/// UI 中 `current_newline` 的实际字符串值为字面量：
/// `\r\n`、`\n`、`\r`、`NONE`（反斜杠字符，而非真实控制字符）。
pub fn newline_bytes(s: &str) -> &'static [u8] {
    match s {
        "\\r\\n" => b"\r\n",
        "\\n" => b"\n",
        "\\r" => b"\r",
        _ => b"",
    }
}

/// 生成 `[HH:MM:SS.mmm] ` 形式的时间戳前缀。
pub fn timestamp_prefix() -> String {
    let now = chrono::Local::now();
    format!("[{}] ", now.format("%H:%M:%S%.3f"))
}
