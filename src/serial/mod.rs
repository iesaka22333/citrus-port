// ============================================================================
// CitrusPort 串口后端模块
// ============================================================================

pub mod config;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod event;
pub mod format;
pub mod manager;

pub use config::SerialConfig;
pub use decoder::ReceiveDecoder;
pub use encoder::{Encoding, encode_text};
pub use error::SerialError;
pub use event::{Event, PortInfo};
pub use manager::{SerialManager, list_ports};
