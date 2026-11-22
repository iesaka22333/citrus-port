<div align="center">

# CitrusPort

**一个用 Rust 和 Slint 写的简单串口调试小工具（开发中）**

一个还在早期阶段的轻量桌面串口助手，探索用现代 UI 框架做硬件调试。

[![Version](https://img.shields.io/badge/version-1.1.0--dev-45658D?style=flat-square)](Cargo.toml)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows-45658D?style=flat-square)](#快速上手)
[![License](https://img.shields.io/badge/license-MIT-277165?style=flat-square)](#许可证)

[English](README.md) · **简体中文**

[当前状态](#当前状态) · [快速上手](#快速上手) · [脚本与测试](#脚本与测试) · [已知不足](#已知不足) · [许可证](#许可证)

</div>

## 当前状态

CitrusPort 是一个探索 **Rust + Slint** 桌面开发的个人项目。

目前基础的收发已经调通，但整体还很粗糙，许多地方还在频繁变动和完善中。

### 目前已经能用的功能

- **连接管理**：支持自动发现串口或手动输入路径（比如 `/dev/pts/*` 或特定设备节点）。支持常见波特率、数据位、校验位、停止位和流控模式设置。
- **数据收发**：支持普通文本（UTF-8、GBK、ASCII）以及 HEX 十六进制输入；可自动附加换行符（`\r\n`、`\n`、`\r` 或不加）；支持实时 RX/TX 字节统计。
- **接收区日志**：支持时间戳显示、文本划选复制（`Ctrl+C`）、自动滚屏开关、清空内容以及保存到本地日志文件。
- **后端测试**：Linux 下内置 PTY 伪终端自动化集成测试，覆盖双向数据收发、重连循环和断开恢复。

### 还没写完 / 还没实测的部分

- **波形分析与协议解析页**：目前纯粹是占位的空页面，还没开始做。
- **硬件流控与信号线**：界面和后端虽然写了 RTS/CTS 和 Break 的逻辑，但还没有找真实硬件模块仔细对照过。
- **Windows 实机验证**：交叉编译脚本与 COM 口测试治具写好了，但因为主力开发在 Linux，Windows MSVC 原生环境和真实 COM 口还没来得及上机细测。
- **超大数据流吞吐**：为了避免界面卡死做了分批节流，但极限流量下的渲染和性能还需要继续打磨。

## 快速上手

需要安装较新的稳定版 [Rust 工具链](https://www.rust-lang.org/tools/install)。

在 **Linux** 上需要安装 `udev` 开发库：
```bash
# Debian / Ubuntu
sudo apt install pkg-config libudev-dev
```

直接通过 Cargo 运行：
```bash
cargo run --locked
```

1. 在端口输入框选择已识别的串口，或者直接手填设备路径。
2. 确认波特率和参数（默认：`115200 8N1`），点击连接。
3. 输入文本，或者勾选 **HEX发送** 输入如 `01 02 FF` 的十六进制字节。

## 脚本与测试

根目录 `scripts/` 下提供了一些辅助构建脚本：

```bash
# Linux / Unix
./scripts/build.sh --platform linux --profile debug
./scripts/build.sh --platform linux --profile both --test --offline

# Windows PowerShell
.\scripts\build.ps1 --platform windows --profile debug
.\scripts\build.ps1 --platform windows --profile both --test --offline
```

直接运行测试：
```bash
# 运行全部单元测试和 Linux PTY 集成测试
cargo test --locked --offline

# 仅运行 Unix PTY 回归测试
cargo test --locked --offline --test pty_regression
```

使用 `build.sh --test` 会自动在根目录生成测试汇总报告 `test-results.txt`。

## 已知不足

- 校验位目前仅支持 `None`、`Even` 和 `Odd`（底层 `serialport` crate 暂不支持 Mark / Space）。
- 停止位目前仅支持 `1` 和 `2`（界面选 1.5 会在连接前被拦截拒绝）。
- 遇到 bug 或有好想法欢迎提 issue，开发架构和实现记录可以看 [AGENTS.md](AGENTS.md)。

## 许可证

[MIT](LICENSE)
