//! Unix PTY 与 Windows COM 口共用的串口回归场景。
//!
//! 两个集成测试目标各自 `mod common;`，对端只要求实现 `Read + Write`：
//! Unix 用 `TTYPort::pair()` 的 master 端，Windows 用另一个 COM 口。
#![allow(dead_code)]

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};

use citrus_port::app::SendRequest;
use citrus_port::serial::{Encoding, Event, SerialConfig, SerialManager};

/// 单次 I/O 等待上限。
pub const IO_TIMEOUT: Duration = Duration::from_secs(3);
/// 对端端口的读超时，避免 `read` 永久阻塞。
pub const PEER_TIMEOUT: Duration = Duration::from_millis(50);

pub fn config(port: &str) -> SerialConfig {
    SerialConfig::from_strings(port.to_string(), "115200", "8", "None", "1", "None")
        .expect("测试串口配置应有效")
}

/// 生成确定性的测试负载，包含各种字节值。
pub fn test_payload(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| seed.wrapping_add((i as u8).wrapping_mul(37)))
        .collect()
}

/// 等待满足条件的事件；等待期间收到 `Error` 直接失败。
pub fn wait_event<F>(events: &Receiver<Event>, what: &str, predicate: F) -> Event
where
    F: Fn(&Event) -> bool,
{
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "等待 {what} 超时");
        match events.recv_timeout(remaining) {
            Ok(event) => {
                if predicate(&event) {
                    return event;
                }
                if let Event::Error(msg) = &event {
                    panic!("等待 {what} 时收到错误: {msg}");
                }
            }
            Err(RecvTimeoutError::Timeout) => panic!("等待 {what} 超时"),
            Err(RecvTimeoutError::Disconnected) => panic!("等待 {what} 时事件通道已关闭"),
        }
    }
}

pub fn wait_connected(events: &Receiver<Event>) {
    wait_event(events, "Connected", |e| matches!(e, Event::Connected));
}

pub fn wait_disconnected(events: &Receiver<Event>) {
    wait_event(events, "Disconnected", |e| matches!(e, Event::Disconnected));
}

/// 对端关闭/拔出时允许先收到 `Error`，只等待最终的 `Disconnected`。
pub fn wait_disconnected_ignoring_errors(events: &Receiver<Event>) {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "对端关闭后未收到 Disconnected");
        match events.recv_timeout(remaining) {
            Ok(Event::Disconnected) => return,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("对端关闭后未收到 Disconnected"),
            Err(RecvTimeoutError::Disconnected) => {
                panic!("等待 Disconnected 时事件通道已关闭")
            }
        }
    }
}

/// 收集至少 `len` 个接收字节；允许一批数据拆成多个 `Received`。
pub fn recv_exact(events: &Receiver<Event>, len: usize) -> Vec<u8> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "等待 RX {len} 字节超时，已收到 {}",
            out.len()
        );
        match events.recv_timeout(remaining) {
            Ok(Event::Received(mut bytes)) => out.append(&mut bytes),
            Ok(Event::Error(msg)) => panic!("等待 RX 时收到错误: {msg}"),
            Ok(Event::Disconnected) => panic!("等待 RX 时连接已断开"),
            // PortsChanged / CtsState / Sent 与本次接收无关。
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                panic!("等待 RX {len} 字节超时，已收到 {}", out.len())
            }
            Err(RecvTimeoutError::Disconnected) => panic!("等待 RX 时事件通道已关闭"),
        }
    }
    out
}

/// 从对端读取恰好 `len` 字节，直到截止时间。
pub fn read_exact_peer<P: Read + ?Sized>(peer: &mut P, len: usize) -> Vec<u8> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut out = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        match peer.read(&mut out[filled..]) {
            Ok(0) => panic!("对端提前 EOF"),
            Ok(n) => filled += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                assert!(
                    Instant::now() < deadline,
                    "从对端读取 {len} 字节超时，已收到 {filled}"
                );
            }
            Err(e) => panic!("从对端读取失败: {e}"),
        }
    }
    out
}

pub fn write_all_peer<P: Write + ?Sized>(peer: &mut P, data: &[u8]) {
    peer.write_all(data).expect("向对端写入失败");
    peer.flush().expect("flush 对端失败");
}

/// 双向字节校验：本端分批发送、对端大块回写，双方按字节核对。
pub fn bidirectional_round_trip<P: Read + Write + ?Sized>(
    manager: &SerialManager,
    events: &Receiver<Event>,
    peer: &mut P,
    port: &str,
) {
    manager.open(config(port));
    wait_connected(events);

    // 本端 -> 对端：分多批发送，跨越 worker 的 8 KiB 聚合阈值。
    let tx_payload = test_payload(10_000, 0);
    for chunk in tx_payload.chunks(977) {
        manager.send(chunk.to_vec());
    }
    let got_tx = read_exact_peer(peer, tx_payload.len());
    assert_eq!(got_tx, tx_payload, "本端发送的字节与对端收到的不一致");

    // 对端 -> 本端：一次写入大块数据，验证接收侧按批合并后仍按字节保序。
    let rx_payload = test_payload(12_345, 0x5A);
    write_all_peer(peer, &rx_payload);
    let got_rx = recv_exact(events, rx_payload.len());
    assert_eq!(got_rx, rx_payload, "对端发送的字节与本端收到的不同");

    manager.close();
    wait_disconnected(events);
}

/// 反复关闭/重开同一端口，每轮都校验一次双向收发。
pub fn repeated_close_reopen<P: Read + Write + ?Sized>(
    manager: &SerialManager,
    events: &Receiver<Event>,
    peer: &mut P,
    port: &str,
    cycles: u8,
) {
    for cycle in 0..cycles {
        manager.open(config(port));
        wait_connected(events);

        let tx = vec![cycle, cycle.wrapping_add(1), cycle.wrapping_add(2)];
        manager.send(tx.clone());
        assert_eq!(
            read_exact_peer(peer, tx.len()),
            tx,
            "第 {cycle} 轮重开后的发送字节不一致"
        );

        let rx = vec![cycle ^ 0xA5];
        write_all_peer(peer, &rx);
        assert_eq!(
            recv_exact(events, rx.len()),
            rx,
            "第 {cycle} 轮重开后的接收字节不一致"
        );

        manager.close();
        wait_disconnected(events);
    }
}

/// 应用层共用的发送入口（文本 / HEX / GBK + 换行）通过真实后端路径回归。
pub fn app_send_request_paths<P: Read + Write + ?Sized>(
    manager: &SerialManager,
    events: &Receiver<Event>,
    peer: &mut P,
    port: &str,
) {
    manager.open(config(port));
    wait_connected(events);

    let cases = [
        (
            SendRequest::new("AT", false, Encoding::Utf8, "\\r\\n"),
            b"AT\r\n".to_vec(),
            "AT",
        ),
        (
            SendRequest::new("48 69", true, Encoding::Utf8, "\\n"),
            vec![0x48, 0x69, 0x0A],
            "48 69 0A",
        ),
        (
            SendRequest::new("你好", false, Encoding::Gbk, "NONE"),
            vec![0xC4, 0xE3, 0xBA, 0xC3],
            "你好",
        ),
    ];

    for (request, expected, display) in cases {
        let data = request.encode().expect("发送编码失败");
        assert_eq!(data, expected);
        assert_eq!(request.display(&data), display);

        manager.send(data.clone());
        assert_eq!(
            read_exact_peer(peer, data.len()),
            data,
            "应用层发送入口写出的字节与对端收到的不一致"
        );
    }

    manager.close();
    wait_disconnected(events);
}

pub fn print_duration_stats(label: &str, mut samples: Vec<Duration>, bytes: usize, wall: Duration) {
    assert!(!samples.is_empty());
    samples.sort();
    let pick = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    let throughput = if wall.is_zero() {
        f64::INFINITY
    } else {
        bytes as f64 / wall.as_secs_f64() / 1024.0
    };
    println!(
        "{label}: n={} p50={:?} p95={:?} p99={:?} max={:?} wall={wall:?} throughput={throughput:.1} KiB/s",
        samples.len(),
        pick(0.50),
        pick(0.95),
        pick(0.99),
        samples.last().unwrap(),
    );
}

/// 双向性能基线：延迟分位数、吞吐与停压后的追平时间。
pub fn perf_backend<P: Read + Write + ?Sized>(
    manager: &SerialManager,
    events: &Receiver<Event>,
    peer: &mut P,
    port: &str,
    frame: usize,
    frames: usize,
    label: &str,
) {
    manager.open(config(port));
    wait_connected(events);

    // 对端 -> worker -> Received：记录每帧写入时刻与完整收到时刻。
    let payload = test_payload(frame, 0x33);
    let mut write_times = Vec::with_capacity(frames);
    for _ in 0..frames {
        write_times.push(Instant::now());
        write_all_peer(peer, &payload);
    }
    let write_done = Instant::now();

    let mut received = 0usize;
    let mut completed = 0usize;
    let mut rx_latencies = Vec::with_capacity(frames);
    while completed < frames {
        match events.recv_timeout(IO_TIMEOUT) {
            Ok(Event::Received(bytes)) => {
                received += bytes.len();
                while completed < frames && received >= frame * (completed + 1) {
                    rx_latencies.push(write_times[completed].elapsed());
                    completed += 1;
                }
            }
            Ok(Event::Error(msg)) => panic!("接收阶段收到错误: {msg}"),
            Ok(Event::Disconnected) => panic!("接收阶段连接意外断开"),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => panic!("接收阶段等待数据超时"),
            Err(RecvTimeoutError::Disconnected) => panic!("接收阶段事件通道关闭"),
        }
    }
    let catch_up = write_done.elapsed();
    print_duration_stats(
        &format!("{label} peer->worker->Received"),
        rx_latencies,
        frame * frames,
        catch_up,
    );
    println!("{label} 停止写入后的追平时间: {catch_up:?}");

    // 本端 -> write_all -> 对端读取：测量提交发送到对端读齐整帧的延迟。
    let mut tx_latencies = Vec::with_capacity(frames);
    let tx_started = Instant::now();
    for _ in 0..frames {
        let started = Instant::now();
        manager.send(payload.clone());
        let got = read_exact_peer(peer, frame);
        assert_eq!(got, payload, "性能采样期间发送字节被破坏");
        tx_latencies.push(started.elapsed());
    }
    let tx_elapsed = tx_started.elapsed();
    print_duration_stats(
        &format!("{label} send->write_all->peer"),
        tx_latencies,
        frame * frames,
        tx_elapsed,
    );

    // 批量发送吞吐：先排入全部命令，再连续读齐，测流式吞吐而非单帧往返。
    let bulk_started = Instant::now();
    for _ in 0..frames {
        manager.send(payload.clone());
    }
    for _ in 0..frames {
        let got = read_exact_peer(peer, frame);
        assert_eq!(got, payload, "批量发送期间字节被破坏");
    }
    let bulk_elapsed = bulk_started.elapsed();
    let bulk_kib_s = (frame * frames) as f64 / bulk_elapsed.as_secs_f64() / 1024.0;
    println!("{label} bulk send->peer: wall={bulk_elapsed:?} throughput={bulk_kib_s:.1} KiB/s");

    manager.close();
    wait_disconnected(events);
}
