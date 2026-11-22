<div align="center">

# CitrusPort

**A simple serial port terminal built with Rust and Slint (WIP)**

A lightweight desktop tool for serial communication, currently in early development.

[![Version](https://img.shields.io/badge/version-1.1.0--dev-45658D?style=flat-square)](Cargo.toml)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows-45658D?style=flat-square)](#quick-start)
[![License](https://img.shields.io/badge/license-MIT-277165?style=flat-square)](#license)

**English** · [简体中文](README.zh-CN.md)

[Status](#status) · [Quick Start](#quick-start) · [Scripts & Tests](#scripts--tests) · [Known Limitations](#known-limitations) · [License](#license)

</div>

## Status

CitrusPort is a personal project exploring a clean, modern serial debugging UI with **Rust** and **Slint**.

It handles basic send and receive tasks, but it is still rough around the edges and actively evolving.

### What works right now

- **Connection**: Port selection from auto-discovery or manual path entry (e.g., `/dev/pts/*` or custom device nodes). Configurable baud rate, data bits, parity, stop bits, and flow control modes.
- **Send & Receive**: Plain text (UTF-8, GBK, ASCII) and raw HEX input. Automatic line ending appending (`\r\n`, `\n`, `\r`, or none) and live RX/TX byte counters.
- **Log display**: Timestamps, text selection (`Ctrl+C` to copy), auto-scrolling toggle, clearing, and saving log contents to a local file.
- **Backend tests**: In-process PTY integration tests on Linux verifying bidirectional transfer, reconnect loops, and error recovery.

### What is still unfinished / unverified

- **Waveform & Protocol tabs**: Currently just UI placeholder screens.
- **Hardware flow control & lines**: RTS/CTS and Break controls exist in the UI and backend logic, but have not been thoroughly validated against physical hardware adapters yet.
- **Windows verification**: Cross-compilation scripts and COM regression tests are written, but native Windows MSVC runs and physical COM tests still await real-machine checks.
- **Heavy throughput**: Large continuous streams (megabytes per second) are throttled to keep the UI usable, but performance tuning is ongoing.

## Quick Start

You will need a recent stable [Rust toolchain](https://www.rust-lang.org/tools/install).

On **Linux**, install development libraries for `udev`:
```bash
# Debian / Ubuntu
sudo apt install pkg-config libudev-dev
```

Run directly with Cargo:
```bash
cargo run --locked
```

1. Pick your serial port or type the path directly in the port field.
2. Adjust baud rate (default: `115200 8N1`), then click to connect.
3. Type text or tick **HEX发送** (HEX send) to send raw hex pairs like `01 02 FF`.

## Scripts & Tests

Helper scripts are provided in `scripts/`:

```bash
# Linux / Unix
./scripts/build.sh --platform linux --profile debug
./scripts/build.sh --platform linux --profile both --test --offline

# Windows PowerShell
.\scripts\build.ps1 --platform windows --profile debug
.\scripts\build.ps1 --platform windows --profile both --test --offline
```

To run tests directly:
```bash
# Run all unit and integration tests (including Linux PTY tests)
cargo test --locked --offline

# Run Unix PTY regression specifically
cargo test --locked --offline --test pty_regression
```

Test reports are saved in English to `test-results.txt` when using `build.sh --test`.

## Known Limitations

- Parity only supports `None`, `Even`, and `Odd` (the underlying `serialport` crate does not support Mark / Space).
- Stop bits only support `1` and `2` (1.5 is rejected before opening).
- If you find bugs or have ideas, feel free to open an issue or check [AGENTS.md](AGENTS.md) for internal architecture notes.

## License

[MIT](LICENSE)
