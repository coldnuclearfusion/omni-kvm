//! The local Omni-KVM board, reached over its USB CDC serial port.

use std::io::{Read, Write};
use std::time::Duration;

use serialport::{SerialPort, SerialPortType};

use crate::protocol::{self, PACKET_SIZE};

/// Espressif's USB vendor ID; the board's "USB" port uses it.
pub const ESPRESSIF_VID: u16 = 0x303A;

/// A board's CDC port as found on this computer.
#[derive(Debug, Clone)]
pub struct Found {
    /// e.g. "COM9" on Windows, "/dev/cu.usbmodemE8F60A8DB00C2" on macOS.
    pub port: String,
    /// USB serial number: the board's MAC address, e.g. "907069353188".
    pub serial: Option<String>,
}

/// Lists Omni-KVM boards whose "USB" port is connected to this computer.
pub fn find() -> Vec<Found> {
    let ports = serialport::available_ports().unwrap_or_default();
    ports
        .into_iter()
        // macOS lists each serial device twice: /dev/cu.* ("call-out") and
        // /dev/tty.* ("dial-in", whose open can wait for a modem carrier).
        // Programs that start the conversation, like this one, use cu.*.
        .filter(|p| !(cfg!(target_os = "macos") && p.port_name.starts_with("/dev/tty.")))
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
    received: Vec<u8>, // bytes from the board not yet assembled into packets
}

impl Board {
    pub fn open(port_name: &str) -> serialport::Result<Board> {
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(50))
            .open()?;
        // The board only sends to us once the host signals it is listening
        // (DTR). Its firmware ignores DTR/RTS otherwise, so this is safe.
        port.write_data_terminal_ready(true)?;
        Ok(Board { port, seq: 0, received: Vec::new() })
    }

    /// Sends one protocol message to the board.
    pub fn send(&mut self, msg_type: u8, payload: &[u8]) -> std::io::Result<()> {
        self.seq = self.seq.wrapping_add(1);
        self.port.write_all(&protocol::packet(msg_type, self.seq, payload))
    }

    /// Asks the board for its link counters; the reply (MSG_DAEMON_STATUS)
    /// comes back through [`Board::receive`].
    pub fn request_stats(&mut self) -> std::io::Result<()> {
        self.send(protocol::msg::DAEMON_CMD, &[protocol::CMD_REQUEST_LINK_STATS])
    }

    /// Returns the packets the board has sent since the last call, without
    /// waiting: status replies and messages relayed from the other daemon.
    pub fn receive(&mut self) -> std::io::Result<Vec<[u8; PACKET_SIZE]>> {
        let waiting = self.port.bytes_to_read()? as usize;
        if waiting > 0 {
            let start = self.received.len();
            self.received.resize(start + waiting, 0);
            let n = self.port.read(&mut self.received[start..])?;
            self.received.truncate(start + n);
        }
        Ok(take_packets(&mut self.received))
    }
}

/// Removes the complete packets from the front of `buf`. The serial link
/// is a byte stream with no packet boundaries, so a packet is found by its
/// first two bytes (magic, version); anything before that is skipped.
fn take_packets(buf: &mut Vec<u8>) -> Vec<[u8; PACKET_SIZE]> {
    let mut packets = Vec::new();
    loop {
        let Some(start) = buf.windows(2).position(|w| w == [protocol::MAGIC, protocol::VERSION]) else {
            // Keep a trailing magic byte: its version byte may be on its way.
            let keep = buf.last() == Some(&protocol::MAGIC);
            buf.drain(..buf.len() - keep as usize);
            return packets;
        };
        buf.drain(..start);
        if buf.len() < PACKET_SIZE {
            return packets;
        }
        packets.push(buf[..PACKET_SIZE].try_into().unwrap());
        buf.drain(..PACKET_SIZE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{msg, packet};

    #[test]
    fn assembles_packets_from_a_byte_stream() {
        let a = packet(msg::DAEMON_STATUS, 1, &[3]);
        let b = packet(msg::HANDOFF, 2, &[1, 7, 0, 0x80]);
        let mut buf = vec![0x00, 0x13]; // noise before the first packet
        buf.extend_from_slice(&a);
        buf.extend_from_slice(&b[..10]); // b arrives in two pieces
        assert_eq!(take_packets(&mut buf), vec![a]);
        buf.extend_from_slice(&b[10..]);
        assert_eq!(take_packets(&mut buf), vec![b]);
        assert!(buf.is_empty());
    }

    #[test]
    fn keeps_a_split_header() {
        let a = packet(msg::HANDOFF_ACK, 3, &[1, 7]);
        let mut buf = vec![a[0]];
        assert!(take_packets(&mut buf).is_empty());
        assert_eq!(buf, vec![a[0]]);
        buf.extend_from_slice(&a[1..]);
        assert_eq!(take_packets(&mut buf), vec![a]);
    }
}
