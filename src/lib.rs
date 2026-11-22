//! CitrusPort 共享库。
//!
//! 桌面程序与回归治具复用同一份实现：
//! - [`serial`]：跨平台串口后端、配置、事件与编解码。
//! - [`app`]：Slint UI 与后端之间的桥接层、发送请求与日志状态。
//!
//! Slint 组件也在库中生成，二进制入口只负责创建 `MainWindow` 并调用
//! [`app::App::run`]。

pub mod app;
pub mod serial;

slint::include_modules!();
