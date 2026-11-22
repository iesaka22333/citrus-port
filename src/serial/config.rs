use crate::serial::error::SerialError;

/// 串口连接参数，与 UI 中的字符串选项一一对应。
#[derive(Debug, Clone)]
pub struct SerialConfig {
    /// 实际端口名，如 "COM3"
    pub port: String,
    pub baud_rate: u32,
    pub data_bits: serialport::DataBits,
    pub parity: serialport::Parity,
    pub stop_bits: serialport::StopBits,
    pub flow_control: serialport::FlowControl,
}

impl SerialConfig {
    /// 根据 UI 提供的字符串选项构造配置，转换失败时返回明确错误而不是 panic。
    #[allow(clippy::too_many_arguments)]
    pub fn from_strings(
        port: String,
        baud_rate: &str,
        data_bits: &str,
        parity: &str,
        stop_bits: &str,
        flow_control: &str,
    ) -> Result<Self, SerialError> {
        let port = port.trim().to_string();
        if port.is_empty() {
            return Err(SerialError::EmptyPort);
        }

        let baud_rate = baud_rate
            .trim()
            .parse::<u32>()
            .map_err(|_| SerialError::InvalidBaudRate(baud_rate.to_string()))?;
        // 波特率 0 无意义，在配置层直接拒绝。
        if baud_rate == 0 {
            return Err(SerialError::InvalidBaudRate(baud_rate.to_string()));
        }

        let data_bits = match data_bits {
            "5" => serialport::DataBits::Five,
            "6" => serialport::DataBits::Six,
            "7" => serialport::DataBits::Seven,
            "8" => serialport::DataBits::Eight,
            other => return Err(SerialError::InvalidDataBits(other.to_string())),
        };

        let parity = match parity {
            "None" => serialport::Parity::None,
            "Even" => serialport::Parity::Even,
            "Odd" => serialport::Parity::Odd,
            // serialport 4.x 的 Parity 枚举仅支持 None/Even/Odd，
            // Mark/Space 在此库中不可用，返回明确错误而非静默映射。
            "Mark" => {
                return Err(SerialError::InvalidParity(
                    "Mark（当前 serialport 库不支持）".to_string(),
                ));
            }
            "Space" => {
                return Err(SerialError::InvalidParity(
                    "Space（当前 serialport 库不支持）".to_string(),
                ));
            }
            other => return Err(SerialError::InvalidParity(other.to_string())),
        };

        let stop_bits = match stop_bits {
            "1" => serialport::StopBits::One,
            "2" => serialport::StopBits::Two,
            // serialport 跨平台库仅支持 1 和 2 停止位，1.5 无法表达。
            "1.5" => {
                return Err(SerialError::InvalidStopBits(
                    "1.5（当前 serialport 库不支持 1.5 停止位）".to_string(),
                ));
            }
            other => return Err(SerialError::InvalidStopBits(other.to_string())),
        };

        let flow_control = match flow_control {
            "None" => serialport::FlowControl::None,
            "Software (XON/XOFF)" => serialport::FlowControl::Software,
            "Hardware (RTS/CTS)" => serialport::FlowControl::Hardware,
            other => return Err(SerialError::InvalidFlowControl(other.to_string())),
        };

        Ok(SerialConfig {
            port,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            flow_control,
        })
    }
}
