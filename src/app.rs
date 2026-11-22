// ============================================================================
// App：串口后端与 Slint UI 之间的桥接层
//
// 职责：
// - 注册 UI 回调（toggle / send / clear / save）。
// - 将 worker 通过 Event channel 发来的事件，经 `invoke_from_event_loop`
//   安全地回投到 UI 线程执行，绝不跨线程直接操作 Slint 对象。
// ============================================================================

use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, SharedString, Weak};

use crate::MainWindow;
use crate::serial::{
    self, Encoding, Event, PortInfo, ReceiveDecoder, SerialConfig, SerialError, SerialManager,
    encode_text,
};

/// RX 显示缓冲上限（字节）。防止长时间高速接收导致内存无限增长。
const MAX_RX_BYTES: usize = 5 * 1024 * 1024;

/// 应用层维护的接收文本缓冲，带上限裁剪。
struct RxBuffer {
    text: String,
}

impl RxBuffer {
    fn new() -> Self {
        Self {
            text: String::new(),
        }
    }

    fn append(&mut self, s: &str) {
        self.text.push_str(s);
        self.trim_to_max();
    }

    fn append_line(&mut self, line: &str) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        self.text.push_str(line);
        self.text.push('\n');
        self.trim_to_max();
    }

    fn clear(&mut self) {
        self.text.clear();
    }

    /// 超出上限时，从头部裁剪，保留最近的 `MAX_RX_BYTES` 字节，
    /// 并调整到完整字符边界，避免从多字节字符中间截断。
    fn trim_to_max(&mut self) {
        if self.text.len() <= MAX_RX_BYTES {
            return;
        }
        let mut from = self.text.len() - MAX_RX_BYTES;
        while from > 0 && !self.text.is_char_boundary(from) {
            from -= 1;
        }
        self.text.drain(..from);
    }
}

/// 应用层 UI 线程状态：解码器 + 接收缓冲 + 待显示 TX 队列。
///
/// 这些状态只在 UI 线程被访问；因 `invoke_from_event_loop` 的闭包必须满足
/// `Send`，故仍用 `Mutex` 包裹。把所有状态合并为一把锁，避免多把锁带来的
/// 额外竞争与复杂度（对应「降低 Arc<Mutex> 状态」的要求）。
struct AppState {
    decoder: ReceiveDecoder,
    rx: RxBuffer,
    /// 待显示 TX 队列：发送时记录显示文本，收到 `Event::Sent` 后按序弹出显示，
    /// 从而保证「只有 write_all 成功后才显示发送记录」。
    pending_tx: VecDeque<String>,
    /// 上一次 Received 使用的显示模式，用于检测 HEX<->文本切换时重置解码器。
    prev_hex: Option<bool>,
}

impl AppState {
    fn new() -> Self {
        Self {
            decoder: ReceiveDecoder::new(Encoding::Utf8),
            rx: RxBuffer::new(),
            pending_tx: VecDeque::new(),
            prev_hex: None,
        }
    }
}

pub struct App;

impl App {
    pub fn run(ui: MainWindow) -> Result<(), Box<dyn std::error::Error>> {
        let (manager, event_rx) =
            SerialManager::start().map_err(|e| format!("串口后端启动失败: {e}"))?;
        let manager = Rc::new(manager);

        let ui_handle = ui.as_weak();

        // 初始化端口列表
        if let Some(ui) = ui_handle.upgrade() {
            if let Ok(ports) = serial::list_ports() {
                apply_ports(&ui, &ports);
            }
        }

        // 应用层状态统一放在一个 Mutex 内，减少锁数量。
        let state = Arc::new(Mutex::new(AppState::new()));

        // 注册 UI 回调
        setup_callbacks(&ui, manager.clone(), state.clone());

        // 事件泵线程：把 worker 事件投递到 UI event loop
        spawn_event_pump(event_rx, ui_handle, state);

        ui.run()?;

        Ok(())
    }
}

fn setup_callbacks(ui: &MainWindow, manager: Rc<SerialManager>, state: Arc<Mutex<AppState>>) {
    let ui_handle = ui.as_weak();

    // ------------------------------------------------------------------
    // 打开 / 关闭串口
    // ------------------------------------------------------------------
    ui.on_toggle_port_clicked({
        let ui_handle = ui_handle.clone();
        let manager = manager.clone();
        let state = state.clone();
        move || {
            let Some(ui) = ui_handle.upgrade() else {
                return;
            };
            // 连接已在进行中时，忽略重复点击，避免多个 Open 命令堆积。
            if ui.get_is_connecting() {
                return;
            }
            if ui.get_is_connected() {
                manager.close();
            } else {
                match serial_config_from_ui(&ui, &manager) {
                    Ok(cfg) => {
                        // 立即进入连接中状态，提供即时视觉反馈并禁用按钮。
                        ui.set_is_connecting(true);
                        // 连接前重置解码器，避免旧 partial multibyte 状态污染新连接。
                        state.lock().unwrap().decoder.reset();
                        manager.open(cfg);
                    }
                    Err(e) => {
                        append_error(&ui, &state, &format!("{e}"));
                        ui.set_is_connected(false);
                    }
                }
            }
        }
    });

    // ------------------------------------------------------------------
    // 发送数据
    // ------------------------------------------------------------------
    ui.on_send_clicked({
        let ui_handle = ui_handle.clone();
        let manager = manager.clone();
        let state = state.clone();
        move || {
            let Some(ui) = ui_handle.upgrade() else {
                return;
            };
            if !ui.get_is_connected() {
                append_error(&ui, &state, "未连接串口，无法发送");
                return;
            }

            let tx = ui.get_tx_content().to_string();
            let hex_send = ui.get_hex_send();

            // 发送路径：HEX 解析 与 文本编码 严格分离，两条路径最终都汇合为
            // 原始 bytes（此时不含换行）。
            let data = if hex_send {
                match crate::serial::format::parse_hex(&tx) {
                    Ok(b) => b,
                    Err(e) => {
                        append_error(&ui, &state, &format!("{e}"));
                        return;
                    }
                }
            } else {
                // 文本路径走 encoder 层，按当前编码真正转换为 bytes（GBK 正确编码）。
                let encoding = Encoding::from_str(&ui.get_current_encoding().to_string());
                match encode_text(&tx, encoding) {
                    Ok(b) => b,
                    Err(e) => {
                        append_error(&ui, &state, &format!("{e}"));
                        return;
                    }
                }
            };

            // 换行字节在编码/解析之后才追加，绝不把换行字符串拼进文本再编码。
            let newline = ui.get_current_newline().to_string();
            let mut data = data;
            data.extend_from_slice(crate::serial::format::newline_bytes(&newline));

            if data.is_empty() {
                return;
            }

            // 构造 TX 显示文本（仅当 write_all 成功、收到 Event::Sent 后才显示）：
            // - HEX 发送：显示实际发送字节（含换行）的 HEX 表示。
            // - 文本发送：显示用户输入的原始文本（不把 GBK bytes 转回乱码）。
            let tx_display = if hex_send {
                crate::serial::format::hex_string(&data)
            } else {
                tx.clone()
            };

            // 将显示文本入队，待 Event::Sent 后按序弹出。此时不直接 append_tx，
            // 避免把「提交发送」伪装成「硬件确认发送成功」。
            state.lock().unwrap().pending_tx.push_back(tx_display);

            manager.send(data);
        }
    });

    // ------------------------------------------------------------------
    // 清空接收区
    // ------------------------------------------------------------------
    ui.on_clear_rx_clicked({
        let ui_handle = ui_handle.clone();
        let state = state.clone();
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                let mut st = state.lock().unwrap();
                st.rx.clear();
                ui.set_rx_content("".into());
            }
        }
    });

    // ------------------------------------------------------------------
    // 保存接收区（预留接口）
    // ------------------------------------------------------------------
    ui.on_save_rx_clicked({
        let _ui_handle = ui_handle.clone();
        move || {
            // TODO: 后续阶段实现保存接收区到文件，避免引入大型 GUI 文件对话框框架。
        }
    });
}

/// 从 UI 读取串口参数并构造配置（含显示名 -> 实际端口名的解析，优先走缓存）。
fn serial_config_from_ui(
    ui: &MainWindow,
    manager: &SerialManager,
) -> Result<SerialConfig, SerialError> {
    let display = ui.get_current_port().to_string();
    let port = manager.resolve_port_name(&display);
    let baud = ui.get_current_baud_rate().to_string();
    let data = ui.get_current_data_bits().to_string();
    let parity = ui.get_current_parity().to_string();
    let stop = ui.get_current_stop_bits().to_string();
    let flow = ui.get_current_flow_control().to_string();
    SerialConfig::from_strings(port, &baud, &data, &parity, &stop, &flow)
}

/// 事件泵线程：把 worker 事件通过 `invoke_from_event_loop` 回投到 UI 线程。
///
/// 事件泵本身不持有任何业务状态，只负责转发；真正的状态（decoder/rx/pending_tx）
/// 都在 UI 线程被访问。
fn spawn_event_pump(
    event_rx: crossbeam_channel::Receiver<Event>,
    ui_handle: Weak<MainWindow>,
    state: Arc<Mutex<AppState>>,
) {
    let _ = std::thread::Builder::new()
        .name("citrus-event-pump".into())
        .spawn(move || {
            while let Ok(event) = event_rx.recv() {
                let weak = ui_handle.clone();
                let state = state.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        handle_event(&ui, event, &state);
                    }
                });
            }
        });
}

/// 在 UI 线程处理 worker 事件，更新 Slint 属性。
fn handle_event(ui: &MainWindow, event: Event, state: &Mutex<AppState>) {
    match event {
        Event::Connected => {
            ui.set_is_connected(true);
            ui.set_is_connecting(false);
        }
        Event::Disconnected => {
            // 断开时重置解码器，避免旧 partial multibyte 状态污染下一次连接。
            state.lock().unwrap().decoder.reset();
            ui.set_is_connected(false);
            ui.set_is_connecting(false);
        }
        Event::Received(bytes) => {
            let mut st = state.lock().unwrap();
            let hex = ui.get_hex_display();
            let with_ts = ui.get_show_timestamp();
            let encoding = ui.get_current_encoding().to_string();

            // HEX<->文本 模式切换时重置解码器，清除残留 partial multibyte。
            if st.prev_hex != Some(hex) {
                st.decoder.reset();
                st.prev_hex = Some(hex);
            }

            let text = if hex {
                crate::serial::format::hex_string(&bytes)
            } else {
                st.decoder.feed(&bytes, &encoding)
            };

            if with_ts {
                st.rx.append(&crate::serial::format::timestamp_prefix());
            }
            // 仅在行首追加 RX 箭头标记，保持流式接收的连续性不受影响。
            let at_line_start = st.rx.text.is_empty() || st.rx.text.ends_with('\n');
            if at_line_start {
                st.rx.append("← ");
            }
            st.rx.append(&text);
            ui.set_rx_content(st.rx.text.clone().into());
        }
        Event::Sent(bytes) => {
            // 只有 worker write_all 成功后才到这一步，按序弹出待显示文本。
            let mut st = state.lock().unwrap();
            let display = match st.pending_tx.pop_front() {
                Some(d) => d,
                // 兜底：理论上不会发生，若发生则退化为显示实际字节的 HEX。
                None => crate::serial::format::hex_string(&bytes),
            };
            append_tx(ui, &mut st, &display);
        }
        Event::Error(msg) => {
            ui.set_is_connecting(false);
            append_error(ui, state, &msg);
        }
        Event::PortEnumerationError(msg) => {
            append_error(ui, state, &msg);
        }
        Event::PortsChanged(ports) => {
            apply_ports(ui, &ports);
        }
    }
}

// ---------------------------------------------------------------------------
// UI 更新辅助函数
// ---------------------------------------------------------------------------

/// 以 `[错误] ...` 形式将错误追加到接收区，并同步 UI 显示。
fn append_error(ui: &MainWindow, rx: &Mutex<AppState>, msg: &str) {
    let mut st = rx.lock().unwrap();
    st.rx.append_line(&format!("[错误] {msg}"));
    ui.set_rx_content(st.rx.text.clone().into());
}

/// 以 `→ 内容` 形式（可选时间戳前缀）将发送记录追加到接收区。
/// 只在 `Event::Sent`（write_all 成功）之后被调用，代表硬件已确认发送。
fn append_tx(ui: &MainWindow, st: &mut AppState, content: &str) {
    let mut line = String::new();
    if ui.get_show_timestamp() {
        line.push_str(&crate::serial::format::timestamp_prefix());
    }
    line.push_str("→ ");
    line.push_str(content);
    st.rx.append_line(&line);
    ui.set_rx_content(st.rx.text.clone().into());
}

fn apply_ports(ui: &MainWindow, ports: &[PortInfo]) {
    let items: Vec<SharedString> = ports
        .iter()
        .map(|p| SharedString::from(p.display.as_str()))
        .collect();
    let model = std::rc::Rc::new(slint::VecModel::from(items));
    ui.set_available_ports(model.into());
}
