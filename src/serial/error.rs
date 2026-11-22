use std::fmt;

/// 串口后端统一错误类型。
///
/// 所有外部串口相关错误最终都会收敛到这里，并以中文可读文本形式呈现给 UI，
/// 绝不使用 `unwrap` / `expect` / `panic!`。
#[derive(Debug)]
pub enum SerialError {
    /// 未选择串口端口
    EmptyPort,
    /// 无效波特率
    InvalidBaudRate(String),
    /// 无效数据位
    InvalidDataBits(String),
    /// 无效校验位
    InvalidParity(String),
    /// 无效停止位
    InvalidStopBits(String),
    /// 无效流控
    InvalidFlowControl(String),
    /// 十六进制格式错误
    InvalidHex(String),
    /// 文本编码错误（如 ASCII 无法表示的字符）
    Encoding(String),
    /// 端口枚举失败
    PortEnumeration(String),
    /// 底层串口错误（打开失败、被占用、设备拔出等）
    Port(serialport::Error),
}

impl fmt::Display for SerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SerialError::EmptyPort => write!(f, "未选择串口端口"),
            SerialError::InvalidBaudRate(v) => write!(f, "无效的波特率: {v}"),
            SerialError::InvalidDataBits(v) => write!(f, "无效的数据位: {v}"),
            SerialError::InvalidParity(v) => write!(f, "无效的校验位: {v}"),
            SerialError::InvalidStopBits(v) => write!(f, "无效的停止位: {v}"),
            SerialError::InvalidFlowControl(v) => write!(f, "无效的流控: {v}"),
            SerialError::InvalidHex(v) => write!(f, "HEX 格式错误: {v}"),
            SerialError::Encoding(v) => write!(f, "编码错误: {v}"),
            SerialError::PortEnumeration(e) => write!(f, "端口枚举失败: {e}"),
            SerialError::Port(e) => write!(f, "串口错误: {e}"),
        }
    }
}

impl std::error::Error for SerialError {}

impl From<serialport::Error> for SerialError {
    fn from(e: serialport::Error) -> Self {
        SerialError::Port(e)
    }
}
