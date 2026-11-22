//! Unix PTY 回归治具。
//!
//! 用 `serialport::TTYPort::pair()` 在进程内创建一对已连接的伪终端：
//! 一端交给 `SerialManager`，另一端由测试代码当作「对端设备」。
//! 不依赖外部 `socat`，普通 `cargo test` 即可运行。
#![cfg(unix)]

mod common;

use citrus_port::serial::SerialManager;
use serialport::{SerialPort, TTYPort};

use common::{config, wait_connected, wait_disconnected_ignoring_errors};

/// 创建 PTY 对，返回 master 端与 slave 的实际设备路径。
/// slave 这里先关闭；`SerialManager` 会按路径重新打开，和真实设备一致。
fn pty_pair() -> (TTYPort, String) {
    let (mut master, slave) = TTYPort::pair().expect("创建 PTY 对失败");
    master
        .set_timeout(common::PEER_TIMEOUT)
        .expect("设置 PTY master 超时失败");
    let name = slave.name().expect("PTY slave 没有设备路径");
    (master, name)
}

#[test]
fn pty_bidirectional_bytes_round_trip() {
    let (mut master, name) = pty_pair();
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::bidirectional_round_trip(&manager, &events, &mut master, &name);
}

#[test]
fn pty_repeated_close_reopen_keeps_working() {
    let (mut master, name) = pty_pair();
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::repeated_close_reopen(&manager, &events, &mut master, &name, 3);
}

#[test]
fn pty_app_send_request_paths_round_trip() {
    let (mut master, name) = pty_pair();
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::app_send_request_paths(&manager, &events, &mut master, &name);
}

#[test]
fn pty_peer_close_reports_disconnect() {
    let (master, name) = pty_pair();
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");

    manager.open(config(&name));
    wait_connected(&events);

    // 对端拔出/关闭：worker 的 read 会失败，应上报错误并断开。
    drop(master);
    wait_disconnected_ignoring_errors(&events);
}

/// 性能基线：PTY 双向字节流的延迟分位数、吞吐与停压后的追平时间。
/// 运行：`cargo test --locked --offline --test pty_regression -- --ignored --nocapture`
#[test]
#[ignore = "performance sample; run manually"]
fn perf_pty_backend() {
    const FRAME: usize = 512;
    const FRAMES: usize = 400;

    let (mut master, name) = pty_pair();
    let (manager, events) = SerialManager::start().expect("启动串口后端失败");
    common::perf_backend(&manager, &events, &mut master, &name, FRAME, FRAMES, "PTY");
}
