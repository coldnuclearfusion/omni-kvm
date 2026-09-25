//! The local Omni-KVM board, reached over its USB CDC serial port.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use serialport::{SerialPort, SerialPortType};

use crate::protocol::{self, LinkStats, PACKET_SIZE};

/// Espressif's USB vendor ID; the board's "USB" port uses it.
pub const ESPRESSIF_VID: u16 = 0x303A;

/// A board's CDC port as found on this computer.
#[derive(Debug, Clone)]
pub struct Found {
    pub port: String,
    /// USB serial number: the board's MAC address, e.g. "907069353188".
    pub serial: Option<String>,
}

/// Lists Omni-KVM boards whose "USB" port is connected to this computer.
pub fn find() -> Vec<Found> {
    let ports = serialport::available_ports().unwrap_or_default();
    ports
        .into_iter()
        .filter_map(|p| match p.port_type {
            SerialPortType::UsbPort(usb) if usb.vid == ESPRESSIF_VID => Some(Found {
                port: p.port_name,
                serial: usb.serial_number,
            }),
            _ => None,
        })
        .collect()
}

pub struct Board {
    port: Box<dyn SerialPort>,
    seq: u32,
}

impl Board {
    pub fn open(port_name: &str) -> serialport::Result<Board> {
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(50))
            .open()?;
        // The board only sends to us once the host signals it is listening
        // (DTR). Its firmware ignores DTR/RTS otherwise, so this is safe.
        port.write_data_terminal_ready(true)?;
        Ok(Board { port, seq: 0 })
    }

    /// Sends one protocol message to the board.
    pub fn send(&mut self, msg_type: u8, payload: &[u8]) -> std::io::Result<()> {
        self.seq = self.seq.wrapping_add(1);
        self.port.write_all(&protocol::packet(msg_type, self.seq, payload))
    }

    /// Asks the board for its link counters and waits up to `timeout` for the reply.
    pub fn request_stats(&mut self, timeout: Duration) -> Result<LinkStats, Box<dyn std::error::Error>> {
        self.port.clear(serialport::ClearBuffer::Input)?;
        self.send(protocol::msg::DAEMON_CMD, &[protocol::CMD_REQUEST_LINK_STATS])?;

        let wanted = [protocol::MAGIC, protocol::VERSION, protocol::msg::DAEMON_STATUS];
        let mut buf: Vec<u8> = Vec::with_capacity(4 * PACKET_SIZE);
        let deadline = Instant::now() + timeout;
        let mut chunk = [0u8; PACKET_SIZE];
        while Instant::now() < deadline {
            match self.port.read(&mut chunk) {
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }
            // The serial link is a byte stream: find where the reply starts.
            if let Some(start) = buf.windows(wanted.len()).position(|w| w == wanted) {
                if buf.len() - start >= PACKET_SIZE {
                    let packet: [u8; PACKET_SIZE] = buf[start..start + PACKET_SIZE].try_into().unwrap();
                    return Ok(LinkStats::parse(&packet)?);
                }
            }
        }
        Err("no reply from the board".into())
    }
}
