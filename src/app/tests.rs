use super::*;
use std::time::Instant;

fn request(text: &str, hex: bool, encoding: Encoding, newline: &str) -> SendRequest {
    SendRequest::new(text, hex, encoding, newline)
}

#[test]
fn text_send_appends_newline_bytes() {
    let req = request("AT", false, Encoding::Utf8, "\\r\\n");
    assert_eq!(req.encode().unwrap(), b"AT\r\n");
    // 文本模式的发送记录保留用户原始输入，不显示编码后的字节。
    assert_eq!(req.display(b"AT\r\n"), "AT");
}

#[test]
fn hex_send_parses_then_appends_newline() {
    let req = request("48 69", true, Encoding::Utf8, "\\n");
    let data = req.encode().unwrap();
    assert_eq!(data, vec![0x48, 0x69, 0x0A]);
    assert_eq!(req.display(&data), "48 69 0A");
}

#[test]
fn gbk_text_send_uses_real_gbk_bytes() {
    let req = request("你好", false, Encoding::Gbk, "NONE");
    assert_eq!(req.encode().unwrap(), vec![0xC4, 0xE3, 0xBA, 0xC3]);
    // 发送记录仍显示原文，而不是把 GBK 字节转回乱码。
    assert_eq!(req.display(&[0xC4, 0xE3, 0xBA, 0xC3]), "你好");
}

#[test]
fn ascii_and_invalid_hex_report_errors() {
    assert!(
        request("中文", false, Encoding::Ascii, "NONE")
            .encode()
            .is_err()
    );
    assert!(
        request("ABC", true, Encoding::Utf8, "NONE")
            .encode()
            .is_err()
    );
}

#[test]
fn receive_counters_and_reconnect_queue_are_consistent() {
    let mut st = AppState::new();
    st.append_received(b"\x01\x02", true, false, "UTF-8");
    st.append_received(b"\x03", true, false, "UTF-8");
    assert_eq!(st.rx_bytes, 3);
    assert_eq!(st.rx.text, "← 01 02 03");

    // 模拟一次未确认发送后断开：旧记录必须被清掉。
    st.pending_tx.push_back("旧记录".into());
    st.on_disconnected();
    assert!(st.pending_tx.is_empty(), "断开后必须清除未确认发送项");

    // 重连后的下一次成功发送只能显示本次内容。
    st.pending_tx.push_back("新记录".into());
    assert_eq!(st.on_sent(b"abcd"), "新记录");
    assert_eq!(st.tx_bytes, 4);
    assert!(st.pending_tx.is_empty());
}

#[test]
fn timestamp_does_not_consume_line_start_arrow() {
    let mut st = AppState::new();
    st.append_received(b"ok", false, true, "UTF-8");
    assert!(
        st.rx.text.contains("← ok"),
        "行首箭头应保留: {}",
        st.rx.text
    );
}

#[test]
fn rx_buffer_trims_in_amortized_chunks() {
    let mut buf = RxBuffer::new();
    buf.text = "a".repeat(MAX_RX_BYTES);
    buf.append("b");

    let after_trim = buf.text.len();
    assert!(after_trim <= MAX_RX_BYTES);
    assert!(
        after_trim >= MAX_RX_BYTES - RX_TRIM_SLACK_BYTES,
        "触顶后应保留在松弛区间内，实际 {after_trim}"
    );

    // 下一次小批量追加不应再次触发整段搬移。
    buf.append("c");
    assert_eq!(buf.text.len(), after_trim + 1);
}

#[test]
fn settings_default_to_chinese_and_citrus_dark() {
    let settings = AppSettings::default();
    assert_eq!(settings.lang, "zh");
    assert_eq!(settings.theme_id, "citrus-dark");
}

#[test]
fn settings_serialization_round_trip() {
    let settings = AppSettings {
        lang: "en".to_string(),
        theme_id: "catppuccin-mocha".to_string(),
    };
    let text = serialize_settings(&settings);
    assert!(text.contains("lang = \"en\""));
    assert!(text.contains("theme-id = \"catppuccin-mocha\""));
    assert_eq!(parse_settings(&text), settings);
}

#[test]
fn invalid_settings_fall_back_to_defaults() {
    let settings = parse_settings("lang = \"fr\"\ntheme-id = \"unknown\"\n");
    assert_eq!(settings.lang, "zh");
    assert_eq!(settings.theme_id, "citrus-dark");
}

#[test]
fn backend_text_matches_selected_language() {
    let zh = backend_text("zh");
    assert!(zh.connected.contains("串口"));
    assert!(zh.not_connected_send.contains("未连接"));
    assert_eq!(zh.error_prefix, "[错误] ");

    let en = backend_text("en");
    assert!(en.connected.contains("Serial port"));
    assert!(en.not_connected_send.contains("not connected"));
    assert_eq!(en.error_prefix, "[Error] ");
}

#[test]
fn serial_errors_localize_to_selected_language() {
    let error = SerialError::InvalidHex("zz".to_string());
    assert!(error.localized("zh").contains("HEX 格式错误"));
    assert!(error.localized("en").starts_with("Invalid HEX"));

    assert_eq!(
        SerialError::EmptyPort.localized("en"),
        "No serial port selected"
    );
}

fn print_duration_stats(label: &str, mut samples: Vec<Duration>, bytes: usize) {
    assert!(!samples.is_empty());
    samples.sort();
    let pick = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    let total: Duration = samples.iter().sum();
    let throughput = if total.is_zero() {
        f64::INFINITY
    } else {
        bytes as f64 / total.as_secs_f64() / 1024.0
    };
    println!(
        "{label}: n={} p50={:?} p95={:?} p99={:?} max={:?} throughput={throughput:.1} KiB/s",
        samples.len(),
        pick(0.50),
        pick(0.95),
        pick(0.99),
        samples.last().unwrap(),
    );
}

/// 性能基线：短日志与接近 5 MiB 上限时的日志追加/整份复制成本。
/// 运行：`cargo test --offline -- --ignored --nocapture perf_app_state`
#[test]
#[ignore = "performance sample; run manually"]
fn perf_app_state_short_vs_long_log() {
    const BATCH: usize = 1024;
    const ROUNDS: usize = 1000;
    let batch = vec![b'x'; BATCH];

    let mut st = AppState::new();
    for _ in 0..64 {
        st.append_received(&batch, false, false, "UTF-8");
    }

    let mut short_samples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let started = Instant::now();
        st.append_received(&batch, false, false, "UTF-8");
        short_samples.push(started.elapsed());
    }
    print_duration_stats("append_received(short)", short_samples, BATCH * ROUNDS);

    // 直接填充到接近上限，避免测试本身耗在数十万次追加上。
    st.rx.text = "x".repeat(MAX_RX_BYTES - BATCH);
    st.rx_dirty = false;

    let mut long_samples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let started = Instant::now();
        st.append_received(&batch, false, false, "UTF-8");
        long_samples.push(started.elapsed());
    }
    assert!(st.rx.text.len() <= MAX_RX_BYTES);
    print_duration_stats("append_received(long)", long_samples, BATCH * ROUNDS);

    // 近似 UI 刷新时的整份文本复制成本。
    let mut copy_samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let started = Instant::now();
        let copy = st.rx.text.clone();
        std::hint::black_box(&copy);
        copy_samples.push(started.elapsed());
    }
    print_duration_stats("rx_text.clone(long)", copy_samples, st.rx.text.len() * 20);
}
