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
use std::time::Duration;

use slint::{ComponentHandle, SharedString, Timer, TimerMode, Weak};

use crate::MainWindow;
use crate::PortOption;
use crate::serial::{
    self, Encoding, Event, PortInfo, ReceiveDecoder, SerialConfig, SerialError, SerialManager,
    encode_text,
};

/// RX 显示缓冲上限（字节）。防止长时间高速接收导致内存无限增长。
const MAX_RX_BYTES: usize = 5 * 1024 * 1024;
/// 触顶后一次性回收到 `MAX_RX_BYTES - RX_TRIM_SLACK_BYTES`，
/// 避免每批新数据都对接近 5 MiB 的字符串做一次前缀搬移。
const RX_TRIM_SLACK_BYTES: usize = 256 * 1024;

/// RX 显示刷新的最小间隔（毫秒）。连续收包时把多次属性更新合并为一次，
/// 避免每个 Received 事件都复制整份日志并触发文本重排。
const RX_UI_FLUSH_MS: u64 = 33;

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
    ///
    /// 为避免每批数据都搬移接近 5 MiB 的内容，触顶后会多回收
    /// `RX_TRIM_SLACK_BYTES`，把 O(n) 搬移摊薄到后续若干批次。
    fn trim_to_max(&mut self) {
        if self.text.len() <= MAX_RX_BYTES {
            return;
        }
        let target = MAX_RX_BYTES.saturating_sub(RX_TRIM_SLACK_BYTES);
        let mut from = self.text.len().saturating_sub(target);
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
    /// 接收缓冲已变化、尚待刷新到 Slint 属性（由定时器批量刷新）。
    rx_dirty: bool,
    /// 累计接收字节数（原始字节，不做文本解码）。
    rx_bytes: usize,
    /// 累计发送成功字节数。
    tx_bytes: usize,
}

impl AppState {
    fn new() -> Self {
        Self {
            decoder: ReceiveDecoder::new(Encoding::Utf8),
            rx: RxBuffer::new(),
            rx_dirty: false,
            rx_bytes: 0,
            tx_bytes: 0,
            pending_tx: VecDeque::new(),
            prev_hex: None,
        }
    }

    /// 追加一批已解码的接收内容，并累计原始 RX 字节数。
    /// 仅修改状态，不触碰 Slint 属性，便于独立回归测试。
    fn append_received(&mut self, bytes: &[u8], hex: bool, with_ts: bool, encoding: &str) {
        // HEX<->文本 模式切换时重置解码器，清除残留 partial multibyte。
        if self.prev_hex != Some(hex) {
            self.decoder.reset();
            self.prev_hex = Some(hex);
        }

        let text = if hex {
            crate::serial::format::hex_string(bytes)
        } else {
            self.decoder.feed(bytes, encoding)
        };

        // 行首判断必须在追加时间戳之前完成，避免时间戳把「行首」状态吃掉。
        let at_line_start = self.rx.text.is_empty() || self.rx.text.ends_with('\n');
        if with_ts {
            self.rx.append(&crate::serial::format::timestamp_prefix());
        }
        if at_line_start {
            self.rx.append("← ");
        }
        // HEX 分批显示时，相邻批次的十六进制串之间补一个空格，
        // 避免 "01 02" + "03" 粘成 "01 0203"。
        if hex && !at_line_start && !text.is_empty() && !self.rx.text.ends_with(' ') {
            self.rx.append(" ");
        }
        self.rx.append(&text);
        self.rx_bytes += bytes.len();
        self.rx_dirty = true;
    }

    /// 处理一次成功写入：累计 TX 字节，并按序弹出待显示文本。
    fn on_sent(&mut self, bytes: &[u8]) -> String {
        self.tx_bytes += bytes.len();
        self.pending_tx.pop_front().unwrap_or_else(|| {
            // 兜底：理论上不会发生，若发生则退化为显示实际字节的 HEX。
            crate::serial::format::hex_string(bytes)
        })
    }

    /// 连接断开：重置流式解码器，并丢弃未确认的待显示发送项。
    fn on_disconnected(&mut self) {
        self.decoder.reset();
        self.pending_tx.clear();
    }
}

/// 发送请求：UI 按钮与回归治具共用的编码入口。
///
/// HEX 与文本两条路径严格互斥，换行字节在编码/解析之后追加，
/// 与按钮点击路径保持完全一致。
#[derive(Debug, Clone)]
pub struct SendRequest {
    pub text: String,
    pub hex: bool,
    pub encoding: Encoding,
    pub newline: String,
}

impl SendRequest {
    /// 构造发送请求。`newline` 使用 UI 中的字面量形式：
    /// `"\\r\\n"`、`"\\n"`、`"\\r"` 或 `"NONE"`。
    pub fn new(
        text: impl Into<String>,
        hex: bool,
        encoding: Encoding,
        newline: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            hex,
            encoding,
            newline: newline.into(),
        }
    }

    fn from_ui(ui: &MainWindow) -> Self {
        Self {
            text: ui.get_tx_content().to_string(),
            hex: ui.get_hex_send(),
            encoding: Encoding::from_str(ui.get_current_encoding().as_str()),
            newline: ui.get_current_newline().to_string(),
        }
    }

    /// 生成实际写入串口的 bytes（含换行）。
    pub fn encode(&self) -> Result<Vec<u8>, SerialError> {
        let mut data = if self.hex {
            crate::serial::format::parse_hex(&self.text)?
        } else {
            encode_text(&self.text, self.encoding)?
        };
        data.extend_from_slice(crate::serial::format::newline_bytes(&self.newline));
        Ok(data)
    }

    /// 发送成功后用于日志显示的文本。
    pub fn display(&self, data: &[u8]) -> String {
        if self.hex {
            crate::serial::format::hex_string(data)
        } else {
            self.text.clone()
        }
    }
}

pub struct App;

impl App {
    pub fn run(ui: MainWindow) -> Result<(), Box<dyn std::error::Error>> {
        let mut ctx = Self::attach(&ui)?;
        ctx.start();
        ui.run()?;
        Ok(())
    }

    /// 注册 UI 回调并准备后端、事件通道和刷新定时器。
    /// 真正的事件循环由 `run` 在 `attach` 之后启动，便于测试单独复用初始化。
    fn attach(ui: &MainWindow) -> Result<AppContext, Box<dyn std::error::Error>> {
        let (manager, event_rx) =
            SerialManager::start().map_err(|e| format!("串口后端启动失败: {e}"))?;
        let manager = Rc::new(manager);

        // 初始化端口列表
        if let Ok(ports) = serial::list_ports() {
            apply_ports(ui, &ports);
        }

        // 应用层状态统一放在一个 Mutex 内，减少锁数量。
        let state = Arc::new(Mutex::new(AppState::new()));

        // 注册 UI 回调
        setup_callbacks(ui, manager, state.clone());

        Ok(AppContext {
            ui_handle: ui.as_weak(),
            state,
            event_rx,
            flush_timer: Timer::default(),
        })
    }
}

/// 应用运行期上下文：持有事件接收端与刷新定时器，
/// 生命周期覆盖整个 Slint 事件循环。
struct AppContext {
    ui_handle: Weak<MainWindow>,
    state: Arc<Mutex<AppState>>,
    event_rx: crossbeam_channel::Receiver<Event>,
    flush_timer: Timer,
}

impl AppContext {
    /// 启动事件泵线程与接收区批量刷新定时器。
    fn start(&mut self) {
        spawn_event_pump(
            self.event_rx.clone(),
            self.ui_handle.clone(),
            self.state.clone(),
        );

        // 接收区刷新定时器：把连续 Received 造成的多次变化合并为低频刷新。
        let flush_ui = self.ui_handle.clone();
        let flush_state = self.state.clone();
        self.flush_timer.start(
            TimerMode::Repeated,
            Duration::from_millis(RX_UI_FLUSH_MS),
            move || {
                if let Some(ui) = flush_ui.upgrade() {
                    flush_rx_to_ui(&ui, &flush_state);
                }
            },
        );
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

            let request = SendRequest::from_ui(&ui);

            // 发送路径统一走 `SendRequest::encode`：HEX 解析与文本编码严格分离，
            // 换行字节在编码/解析之后才追加。
            let data = match request.encode() {
                Ok(data) => data,
                Err(e) => {
                    append_error(&ui, &state, &format!("{e}"));
                    return;
                }
            };

            if data.is_empty() {
                return;
            }

            // 构造 TX 显示文本（仅当 write_all 成功、收到 Event::Sent 后才显示）：
            // - HEX 发送：显示实际发送字节（含换行）的 HEX 表示。
            // - 文本发送：显示用户输入的原始文本（不把 GBK bytes 转回乱码）。
            let tx_display = request.display(&data);

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
                st.rx_dirty = false;
                st.rx_bytes = 0;
                st.tx_bytes = 0;
                ui.set_rx_byte_count(0);
                ui.set_tx_byte_count(0);
                ui.set_rx_content("".into());
            }
        }
    });

    // ------------------------------------------------------------------
    // 保存接收区（保存到用户主目录下的带时间戳日志文件）
    // ------------------------------------------------------------------
    ui.on_save_rx_clicked({
        let ui_handle = ui_handle.clone();
        let state = state.clone();
        move || {
            let Some(ui) = ui_handle.upgrade() else {
                return;
            };
            let text = state.lock().unwrap().rx.text.clone();
            if text.is_empty() {
                append_notice(&ui, &state, "接收区为空，无需保存");
                return;
            }
            let path = rx_save_path();
            match std::fs::write(&path, text.as_bytes()) {
                Ok(()) => append_notice(&ui, &state, &format!("接收区已保存到 {}", path.display())),
                Err(e) => append_error(&ui, &state, &format!("保存接收区失败: {e}")),
            }
        }
    });

    // ------------------------------------------------------------------
    // 控制线：RTS / Break（CTS 由 worker 周期读取后回报）
    // ------------------------------------------------------------------
    ui.on_set_rts_clicked({
        let manager = manager.clone();
        move |level| {
            manager.set_rts(level);
        }
    });

    ui.on_set_break_clicked({
        let manager = manager.clone();
        move |enabled| {
            manager.set_break(enabled);
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
/// 每次从 channel 取出事件后，会继续 drain 当前已排队的事件，并把相邻的
/// `Received` 合并成一个字节缓冲，显著减少 UI 线程回调次数与锁竞争。
fn spawn_event_pump(
    event_rx: crossbeam_channel::Receiver<Event>,
    ui_handle: Weak<MainWindow>,
    state: Arc<Mutex<AppState>>,
) {
    let _ = std::thread::Builder::new()
        .name("citrus-event-pump".into())
        .spawn(move || {
            while let Ok(first) = event_rx.recv() {
                let mut events = vec![first];
                events.extend(event_rx.try_iter());

                let weak = ui_handle.clone();
                let state = state.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = weak.upgrade() {
                        handle_events(&ui, events, &state);
                    }
                });
            }
        });
}

/// 按顺序处理一批事件：相邻 `Received` 合并为一个缓冲，其它事件保持原顺序。
/// 这样既减少解码与 UI 更新次数，也不会打乱数据与断开/错误事件的时间线。
fn handle_events(ui: &MainWindow, events: Vec<Event>, state: &Mutex<AppState>) {
    let mut iter = events.into_iter().peekable();
    while let Some(event) = iter.next() {
        match event {
            Event::Received(mut bytes) => {
                while matches!(iter.peek(), Some(Event::Received(_))) {
                    if let Some(Event::Received(more)) = iter.next() {
                        bytes.extend_from_slice(&more);
                    }
                }
                handle_received(ui, &bytes, state);
            }
            other => handle_event(ui, other, state),
        }
    }
}

/// 处理单个非接收事件，更新 Slint 属性。
fn handle_event(ui: &MainWindow, event: Event, state: &Mutex<AppState>) {
    match event {
        Event::Connected => {
            ui.set_is_connected(true);
            ui.set_is_connecting(false);
        }
        Event::Disconnected => {
            let mut st = state.lock().unwrap();
            // 重置流式解码器，并丢弃未确认的待显示发送项，
            // 避免重连后的下一条成功发送弹出旧内容。
            st.on_disconnected();
            drop(st);
            ui.set_is_connected(false);
            ui.set_is_connecting(false);
            ui.set_cts_enabled(false);
            ui.set_rts_enabled(false);
            ui.set_break_enabled(false);
            // 把断开前最后一批接收文本立即刷出，避免停在旧画面。
            flush_rx_to_ui(ui, state);
        }
        Event::Received(bytes) => handle_received(ui, &bytes, state),
        Event::Sent(bytes) => {
            // 只有 worker write_all 成功后才到这一步，按序弹出待显示文本。
            let mut st = state.lock().unwrap();
            let display = st.on_sent(&bytes);
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
        Event::CtsState(level) => {
            ui.set_cts_enabled(level);
        }
    }
}

/// 处理一批已合并的接收字节：解码后追加到缓冲，并标记为待刷新。
/// 不在这里直接写 Slint 属性，具体刷新由 `RX_UI_FLUSH_MS` 定时器完成。
fn handle_received(ui: &MainWindow, bytes: &[u8], state: &Mutex<AppState>) {
    let hex = ui.get_hex_display();
    let with_ts = ui.get_show_timestamp();
    let encoding = ui.get_current_encoding().to_string();
    state
        .lock()
        .unwrap()
        .append_received(bytes, hex, with_ts, &encoding);
}

// ---------------------------------------------------------------------------
// UI 更新辅助函数
// ---------------------------------------------------------------------------

/// 把接收缓冲的待刷新内容写到 Slint 属性。
/// 只有 `rx_dirty` 时才复制文本，且复制完成后立即清除标记。
fn flush_rx_to_ui(ui: &MainWindow, state: &Mutex<AppState>) {
    let mut st = state.lock().unwrap();
    if !st.rx_dirty {
        return;
    }
    st.rx_dirty = false;
    ui.set_rx_content(st.rx.text.as_str().into());
    ui.set_rx_byte_count(st.rx_bytes as i32);
    ui.set_tx_byte_count(st.tx_bytes as i32);
}

/// 以 `[提示] ...` 形式将提示追加到接收区，并同步 UI 显示。
fn append_notice(ui: &MainWindow, state: &Mutex<AppState>, msg: &str) {
    {
        let mut st = state.lock().unwrap();
        st.rx.append_line(&format!("[提示] {msg}"));
        st.rx_dirty = true;
    }
    flush_rx_to_ui(ui, state);
}

/// 接收区保存路径：用户主目录下带时间戳的日志文件。
fn rx_save_path() -> std::path::PathBuf {
    let dir = std::env::var_os("HOME")
        // Windows 通常没有 HOME，退回 USERPROFILE。
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let filename = format!(
        "citrusport-rx-{}.log",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    dir.join(filename)
}

/// 以 `[错误] ...` 形式将错误追加到接收区，并同步 UI 显示。
fn append_error(ui: &MainWindow, state: &Mutex<AppState>, msg: &str) {
    {
        let mut st = state.lock().unwrap();
        st.rx.append_line(&format!("[错误] {msg}"));
        st.rx_dirty = true;
    }
    flush_rx_to_ui(ui, state);
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
    st.rx_dirty = false;
    ui.set_rx_content(st.rx.text.as_str().into());
    ui.set_rx_byte_count(st.rx_bytes as i32);
    ui.set_tx_byte_count(st.tx_bytes as i32);
}

fn apply_ports(ui: &MainWindow, ports: &[PortInfo]) {
    // 显示名与实际路径分离：列表里展示 `display`，选中后把 `name` 写回输入框。
    // 手填的路径不会被枚举列表覆盖，直接交给后端打开。
    let items: Vec<PortOption> = ports
        .iter()
        .map(|p| PortOption {
            name: SharedString::from(p.name.as_str()),
            display: SharedString::from(p.display.as_str()),
        })
        .collect();
    let model = std::rc::Rc::new(slint::VecModel::from(items));
    ui.set_available_ports(model.into());
}

#[cfg(test)]
mod tests;
