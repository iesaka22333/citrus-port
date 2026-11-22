// ============================================================================
// 事件与命令定义：串口后端与 App 层之间的通信协议。
//
// 这里只定义「数据契约」，不含任何 I/O、编码、UI 逻辑。
// Command 用于 App -> worker，Event 用于 worker -> App。
// ============================================================================

use crate::serial::config::SerialConfig;

/// 发送给串口 worker 线程的命令。
pub enum Command {
    /// 打开指定配置的串口
    Open(SerialConfig),
    /// 关闭当前串口
    Close,
    /// 发送原始字节
    Send(Vec<u8>),
    /// 设置 RTS 电平（true = 有效）
    SetRts(bool),
    /// 设置 Break 状态（true = 发送 Break）
    SetBreak(bool),
    /// 终止 worker 线程
    Shutdown,
}

/// worker 线程发回给 App 的事件。
#[derive(Debug, Clone)]
pub enum Event {
    /// 串口已成功打开
    Connected,
    /// 串口已关闭 / 断开
    Disconnected,
    /// 收到数据（已按批处理聚合，为原始 bytes）
    Received(Vec<u8>),
    /// 数据已成功写入串口（写入成功后才发出，原始 bytes）
    Sent(Vec<u8>),
    /// 错误信息
    Error(String),
    /// 可用端口列表发生变化（与「枚举失败」严格区分）
    PortsChanged(Vec<PortInfo>),
    /// 端口枚举失败（区别于"真的没有端口"）
    PortEnumerationError(String),
    /// CTS 输入状态发生变化（由 worker 周期读取）
    CtsState(bool),
}

/// 端口信息：区分实际端口名与用于 UI 显示的文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// 实际端口名，如 "COM3"
    pub name: String,
    /// 用于 UI 显示的文本，如 "COM3 — USB-SERIAL CH340"
    pub display: String,
}
