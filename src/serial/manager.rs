// ============================================================================
// 串口管理器：单一 worker 线程拥有 SerialPort
//
// 架构：
//
//   UI / App  --Command channel-->  串口 worker 线程 (持有 SerialPort)
//                                    ├── Open / Close / Send / Read
//                                    └── Poll 线程 (每 3s 枚举一次端口)
//                                    --Event channel-->  App -> UI event loop
//
// SerialPort 所有权只存在于 worker 线程，UI 线程绝不执行阻塞式串口 I/O。
// ============================================================================

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::serial::config::SerialConfig;
use crate::serial::error::SerialError;
use crate::serial::event::{Command, Event, PortInfo};

/// 串口后端管理器：持有命令发送端与 worker / poll 线程句柄。
pub struct SerialManager {
    command_tx: Sender<Command>,
    poll_shutdown_tx: Sender<()>,
    /// 最近一次成功枚举的端口缓存，供端口名解析避免重复枚举。
    ports_cache: Arc<Mutex<Vec<PortInfo>>>,
    worker: Option<JoinHandle<()>>,
    poll: Option<JoinHandle<()>>,
}

/// 串口 read 缓冲区大小。
const READ_BUF_SIZE: usize = 4096;
/// 串口 read 超时（ms）：worker 借此周期性醒来检查命令与 flush。
const READ_TIMEOUT_MS: u64 = 10;
/// 接收批处理触发阈值（字节）：达到即 flush 一次。
const FLUSH_BYTES: usize = 8192;
/// 接收批处理触发阈值（时间）：首个字节停留超过此时长即 flush。
const FLUSH_INTERVAL_MS: u64 = 16;
/// 每个 worker 循环最多 drain 的 read 次数，避免高速数据饿死命令处理。
const CHUNKS_PER_TICK: usize = 8;
/// 端口枚举轮询周期。
const POLL_PERIOD: Duration = Duration::from_secs(3);

impl SerialManager {
    /// 启动串口 worker 线程与端口轮询线程，返回管理器与事件接收端。
    pub fn start() -> Result<(Self, Receiver<Event>), std::io::Error> {
        let (command_tx, command_rx) = crossbeam_channel::unbounded();
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (poll_shutdown_tx, poll_shutdown_rx) = crossbeam_channel::bounded(1);

        let ports_cache = Arc::new(Mutex::new(Vec::<PortInfo>::new()));
        // 启动时立即枚举一次作为缓存基线（失败则留空）。
        if let Ok(ports) = list_ports() {
            *ports_cache.lock().unwrap() = ports;
        }

        let worker_event_tx = event_tx.clone();
        let worker = thread::Builder::new()
            .name("citrus-serial-worker".into())
            .spawn(move || worker_loop(command_rx, worker_event_tx))?;

        let poll = thread::Builder::new()
            .name("citrus-serial-poll".into())
            .spawn({
                let ports_cache = ports_cache.clone();
                move || poll_loop(event_tx, poll_shutdown_rx, ports_cache)
            })?;

        Ok((
            Self {
                command_tx,
                poll_shutdown_tx,
                ports_cache,
                worker: Some(worker),
                poll: Some(poll),
            },
            event_rx,
        ))
    }

    /// 向 worker 发送打开命令（非阻塞）。
    pub fn open(&self, cfg: SerialConfig) {
        let _ = self.command_tx.send(Command::Open(cfg));
    }

    /// 向 worker 发送关闭命令（非阻塞）。
    pub fn close(&self) {
        let _ = self.command_tx.send(Command::Close);
    }

    /// 向 worker 发送数据（非阻塞）。
    pub fn send(&self, data: Vec<u8>) {
        let _ = self.command_tx.send(Command::Send(data));
    }

    /// 将 UI 的端口显示文本解析为实际端口名，优先从已知端口缓存映射，
    /// 避免连接时重复枚举整个系统。
    pub fn resolve_port_name(&self, current_port: &str) -> String {
        let cache = self.ports_cache.lock().unwrap();
        resolve_port_name_from(current_port, &cache)
    }
}

impl Drop for SerialManager {
    fn drop(&mut self) {
        // Command::Shutdown 作为 worker 生命周期唯一控制源，
        // 无需额外的 AtomicBool 标志。
        let _ = self.command_tx.send(Command::Shutdown);
        let _ = self.poll_shutdown_tx.send(());
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some(p) = self.poll.take() {
            let _ = p.join();
        }
    }
}

/// 枚举当前系统中可用的串口，返回用于 UI 显示的端口信息。
///
/// 失败时返回错误（由调用方区分"确实没有端口"与"枚举失败"），不 panic。
pub fn list_ports() -> Result<Vec<PortInfo>, SerialError> {
    let ports =
        serialport::available_ports().map_err(|e| SerialError::PortEnumeration(e.to_string()))?;
    Ok(ports
        .into_iter()
        .map(|info| {
            let name = info.port_name.clone();
            let display = match &info.port_type {
                serialport::SerialPortType::UsbPort(usb) => {
                    let product = usb.product.clone().filter(|s| !s.trim().is_empty());
                    match product {
                        Some(p) => format!("{name} — {p}"),
                        None => name.clone(),
                    }
                }
                _ => name.clone(),
            };
            PortInfo { name, display }
        })
        .collect())
}

/// 从已知端口列表中解析实际端口名，作为避免重复枚举的快速路径。
fn resolve_port_name_from(current_port: &str, known: &[PortInfo]) -> String {
    for p in known {
        if p.name == current_port || p.display == current_port {
            return p.name.clone();
        }
    }
    // 兜底：取第一个空白分隔的 token 作为端口名（仅在缓存未命中时使用）。
    current_port
        .split_whitespace()
        .next()
        .unwrap_or(current_port)
        .to_string()
}

/// 串口 worker 主循环：串行处理命令 + 读取串口数据 + 接收批处理。
///
/// 生命周期控制源只有 `Command::Shutdown`，收到后即退出（不再使用 AtomicBool）。
/// 事件顺序保证：Open / Close / 读错误 / 写错误 前都会先 flush pending，
/// 确保 UI 数据顺序符合真实串口时间线（先 Received 后 Disconnected）。
fn worker_loop(command_rx: Receiver<Command>, event_tx: Sender<Event>) {
    let mut port: Option<Box<dyn serialport::SerialPort>> = None;
    let mut pending: Vec<u8> = Vec::with_capacity(FLUSH_BYTES);
    let mut pending_since: Option<Instant> = None;

    loop {
        // 1. 非阻塞地循环处理排队命令，优先响应 Shutdown。
        let mut shutdown = false;
        for cmd in command_rx.try_iter() {
            match cmd {
                Command::Shutdown => {
                    shutdown = true;
                    break;
                }
                Command::Open(cfg) => {
                    // 打开前后都先 flush 残余 RX，避免旧数据排在断开事件之后。
                    flush_pending(&mut pending, &mut pending_since, &event_tx);
                    // 若已有连接，先释放旧端口并发出断开事件。
                    if port.take().is_some() {
                        let _ = event_tx.send(Event::Disconnected);
                    }
                    match open_port(&cfg) {
                        Ok(p) => {
                            port = Some(p);
                            let _ = event_tx.send(Event::Connected);
                        }
                        Err(e) => {
                            let _ = event_tx
                                .send(Event::Error(format!("打开串口 {} 失败: {e}", cfg.port)));
                        }
                    }
                }
                Command::Close => {
                    // 先 flush 残余 RX，确保发出 Disconnected 前残余数据已送出。
                    flush_pending(&mut pending, &mut pending_since, &event_tx);
                    if port.take().is_some() {
                        let _ = event_tx.send(Event::Disconnected);
                    }
                }
                Command::Send(data) => match port.as_mut() {
                    Some(p) => {
                        match p.write_all(&data) {
                            Ok(()) => {
                                // 只有 write_all 成功后才发 Sent，绝不提前伪装成功。
                                let _ = event_tx.send(Event::Sent(data));
                            }
                            Err(e) => {
                                // 写失败：先 flush 残余 RX，再报错并断开。
                                flush_pending(&mut pending, &mut pending_since, &event_tx);
                                port = None;
                                let _ = event_tx.send(Event::Error(format!("发送失败: {e}")));
                                let _ = event_tx.send(Event::Disconnected);
                            }
                        }
                    }
                    None => {
                        let _ = event_tx.send(Event::Error("未连接串口，无法发送".to_string()));
                    }
                },
            }
        }
        if shutdown {
            break;
        }

        // 2. 到达时间阈值的 pending 数据先 flush。
        if !pending.is_empty() {
            if let Some(t) = pending_since {
                if t.elapsed() >= Duration::from_millis(FLUSH_INTERVAL_MS) {
                    flush_pending(&mut pending, &mut pending_since, &event_tx);
                }
            }
        }

        // 3. 若串口已打开，drain 读取数据（聚合到 pending，按批 flush）。
        if let Some(p) = port.as_mut() {
            for _ in 0..CHUNKS_PER_TICK {
                let mut buf = [0u8; READ_BUF_SIZE];
                match p.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if pending_since.is_none() {
                            pending_since = Some(Instant::now());
                        }
                        pending.extend_from_slice(&buf[..n]);
                        if pending.len() >= FLUSH_BYTES {
                            flush_pending(&mut pending, &mut pending_since, &event_tx);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                    Err(e) => {
                        // 读取失败（如设备拔出）：先 flush 残余 RX，再报错并断开。
                        flush_pending(&mut pending, &mut pending_since, &event_tx);
                        port = None;
                        let _ = event_tx.send(Event::Error(format!("读取失败: {e}")));
                        let _ = event_tx.send(Event::Disconnected);
                        break;
                    }
                }
            }
        } else {
            // 无端口时轻微让出 CPU，等待命令到来。
            thread::sleep(Duration::from_millis(1));
        }
    }

    // 退出前 flush 残余数据并释放端口。
    flush_pending(&mut pending, &mut pending_since, &event_tx);
    drop(port);
}

fn flush_pending(
    pending: &mut Vec<u8>,
    pending_since: &mut Option<Instant>,
    event_tx: &Sender<Event>,
) {
    if pending.is_empty() {
        *pending_since = None;
        return;
    }
    let _ = event_tx.send(Event::Received(std::mem::take(pending)));
    *pending_since = None;
}

fn open_port(cfg: &SerialConfig) -> Result<Box<dyn serialport::SerialPort>, SerialError> {
    let builder = serialport::new(&cfg.port, cfg.baud_rate)
        .data_bits(cfg.data_bits)
        .parity(cfg.parity)
        .stop_bits(cfg.stop_bits)
        .flow_control(cfg.flow_control)
        .timeout(Duration::from_millis(READ_TIMEOUT_MS));
    let port = builder.open()?;
    Ok(port)
}

/// 端口轮询线程：等待最多 3 秒 → 枚举端口；收到 shutdown 信号则立即退出。
fn poll_loop(
    event_tx: Sender<Event>,
    shutdown_rx: Receiver<()>,
    ports_cache: Arc<Mutex<Vec<PortInfo>>>,
) {
    let mut last: Vec<PortInfo> = ports_cache.lock().unwrap().clone();

    loop {
        match shutdown_rx.recv_timeout(POLL_PERIOD) {
            // 收到 shutdown：立即退出。
            Ok(()) => break,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                match list_ports() {
                    Ok(ports) => {
                        if ports != last {
                            let _ = event_tx.send(Event::PortsChanged(ports.clone()));
                            last = ports.clone();
                        }
                        // 更新端口缓存供端口名解析使用。
                        *ports_cache.lock().unwrap() = ports;
                    }
                    Err(e) => {
                        // 枚举失败 ≠ 没有端口，发送明确错误，
                        // 不清空已有端口状态。
                        let _ = event_tx.send(Event::PortEnumerationError(format!("{e}")));
                    }
                }
            }
        }
    }
}
