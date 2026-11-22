//! Windows COM 口回归治具。
//!
//! 需要一对已正确交叉连接或由虚拟串口工具创建的 COM 口，
//! 通过环境变量 `CITRUS_TEST_PORT_A` 和 `CITRUS_TEST_PORT_B` 指定。
//! 所有用例都标记为 `#[ignore]`，未配置端口时不会影响 `cargo test`。
//!
//! 运行：
//! `cargo test --locked --offline --test com_regression -- --ignored --nocapture`
//!
//! 注意：关闭对端 COM 句柄通常不等同于设备移除。断开事件需用实际拔插
//! 或支持移除的设备场景验证，因此本目标不提供与 PTY 对端关闭等价的用例。
#![cfg(windows)]

mod common;

use std::env;

use citrus_port::serial::SerialManager;
use serialport::SerialPort;

const PORT_A_ENV: &str = "CITRUS_TEST_PORT_A";
const PORT_B_ENV: &str = "CITRUS_TEST_PORT_B";

/// 读取并校验一对专用 COM 口。
/// 缺失、为空或两端相同都给出明确错误，而不是静默跳过。
fn endpoints() -> (String, String) {
    let a = env::var(PORT_A_ENV)
        .unwrap_or_else(|_| panic!("未设置环境变量 {PORT_A_ENV}，无法运行 COM 口回归"));
    let b = env::var(PORT_B_ENV)
        .unwrap_or_else(|_| panic!("未设置环境变量 {PORT_B_ENV}，无法运行 COM 口回归"));
    let a = a.trim().to_string();
    let b = b.trim().to_string();
    assert!(!a.is_empty(), "{PORT_A_ENV} 不能为空");
    assert!(!b.is_empty(), "{PORT_B_ENV} 不能为空");
    assert_ne!(a, b, "{PORT_A_ENV} 与 {PORT_B_ENV} 不能是同一个端口");
    (a, b)
}

/// 打开对端 COM 口。参数与 `SerialManager` 使用的配置保持一致。
fn open_peer(port: &str) -> Box<dyn SerialPort> {
    serialport::new(port, 115200)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .timeout(common::PEER_TIMEOUT)
        .open()
        .unwrap_or_else(|e| panic!("无法打开对端 COM 口 {port}: {e}"))
}

#[test]
#[ignore = "requires CITRUS_TEST_PORT_A/B"]
fn com_bidirectional_bytes_round_trip() {
    let (a, b) = endpoints();
    let mut peer = open_peer(&b);
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::bidirectional_round_trip(&manager, &events, peer.as_mut(), &a);
}

#[test]
#[ignore = "requires CITRUS_TEST_PORT_A/B"]
fn com_repeated_close_reopen_keeps_working() {
    let (a, b) = endpoints();
    let mut peer = open_peer(&b);
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::repeated_close_reopen(&manager, &events, peer.as_mut(), &a, 3);
}

#[test]
#[ignore = "requires CITRUS_TEST_PORT_A/B"]
fn com_app_send_request_paths_round_trip() {
    let (a, b) = endpoints();
    let mut peer = open_peer(&b);
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::app_send_request_paths(&manager, &events, peer.as_mut(), &a);
}

/// 性能基线：按端口类型单独记录。运行方式与 Unix PTY 相同。
#[test]
#[ignore = "requires CITRUS_TEST_PORT_A/B"]
fn perf_com_backend() {
    const FRAME: usize = 512;
    const FRAMES: usize = 400;

    let (a, b) = endpoints();
    let mut peer = open_peer(&b);
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::perf_backend(&manager, &events, peer.as_mut(), &a, FRAME, FRAMES, "COM");
}
