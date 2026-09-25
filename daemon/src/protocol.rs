//! Omni-KVM protocol v0.1: packet layout and constants.
//!
//! The specification is `shared/protocol.md` (the single source of
//! truth); the firmware's copy is `firmware/include/protocol.h`. Keep
//! all three in sync. Multi-byte fields are little-endian.

pub const MAGIC: u8 = 0x4B; // 'K' for KVM
pub const VERSION: u8 = 0x01;
pub const PACKET_SIZE: usize = 64;
pub const HEADER_SIZE: usize = 8;

/// Message types used by the daemon (see protocol.md for the full list).
pub mod msg {
    pub const MOUSE_MOVE: u8 = 0x01;
    pub const MOUSE_SCROLL: u8 = 0x02;
    pub const KEY_DOWN: u8 = 0x03;
    pub const KEY_UP: u8 = 0x04;
    pub const MODIFIER_SYNC: u8 = 0x05;
    // Between the two daemons, relayed by the boards (see peer.rs).
    pub const HANDOFF: u8 = 0x10;
    pub const HANDOFF_ACK: u8 = 0x11;
    pub const EDGE_CONTACT: u8 = 0x12;
    pub const DAEMON_CMD: u8 = 0x40;
    pub const DAEMON_STATUS: u8 = 0x41;
}

pub const CMD_REQUEST_LINK_STATS: u8 = 0x04;
pub const STATUS_LINK_STATS: u8 = 0x03;
pub const LINK_STATS_LAYOUT: u8 = 3;

/// Builds a 64-byte packet: header, then `payload`, zero-padded.
pub fn packet(msg_type: u8, seq: u32, payload: &[u8]) -> [u8; PACKET_SIZE] {
    assert!(payload.len() <= PACKET_SIZE - HEADER_SIZE, "payload too large");
    let mut p = [0u8; PACKET_SIZE];
    p[0] = MAGIC;
    p[1] = VERSION;
    p[2] = msg_type;
    p[3] = 0; // flags
    p[4..8].copy_from_slice(&seq.to_le_bytes());
    p[HEADER_SIZE..HEADER_SIZE + payload.len()].copy_from_slice(payload);
    p
}

/// A board's link counters since it booted (MSG_DAEMON_STATUS 0x03).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub link_up: bool,
    /// Radio TX rate (ESP-IDF `wifi_phy_rate_t`), see [`phy_rate_name`].
    pub phy_rate: u8,
    pub session: u32,
    pub host_received: u32,
    pub host_dropped: u32,
    pub radio_sent: u32,
    pub radio_retransmits: u32,
    pub radio_gave_up: u32,
    pub radio_received: u32,
    pub radio_rx_overflow: u32,
    pub auth_failures: u32,
    pub replays_dropped: u32,
    pub frames_acked: u32,
    pub frames_failed: u32,
    pub hid_stalls: u16,
    pub hid_mouse_dropped: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    NotLinkStats,
    /// The board speaks a different LinkStats layout: firmware and daemon
    /// are out of sync, so the numbers would be misread.
    WrongLayout(u8),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NotLinkStats => write!(f, "not a link statistics packet"),
            ParseError::WrongLayout(l) => write!(
                f,
                "board sends link stats layout {l}, this daemon reads layout {LINK_STATS_LAYOUT}; flash the current firmware"
            ),
        }
    }
}

impl std::error::Error for ParseError {}

impl LinkStats {
    /// Parses a MSG_DAEMON_STATUS packet carrying link statistics.
    pub fn parse(packet: &[u8; PACKET_SIZE]) -> Result<Self, ParseError> {
        let p = &packet[HEADER_SIZE..];
        if packet[2] != msg::DAEMON_STATUS || p[0] != STATUS_LINK_STATS {
            return Err(ParseError::NotLinkStats);
        }
        if p[1] != LINK_STATS_LAYOUT {
            return Err(ParseError::WrongLayout(p[1]));
        }
        // Little-endian fields, in proto::LinkStats order.
        let u32_at = |i: usize| u32::from_le_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
        let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        Ok(LinkStats {
            link_up: p[2] != 0,
            phy_rate: p[3],
            session: u32_at(4),
            host_received: u32_at(8),
            host_dropped: u32_at(12),
            radio_sent: u32_at(16),
            radio_retransmits: u32_at(20),
            radio_gave_up: u32_at(24),
            radio_received: u32_at(28),
            radio_rx_overflow: u32_at(32),
            auth_failures: u32_at(36),
            replays_dropped: u32_at(40),
            frames_acked: u32_at(44),
            frames_failed: u32_at(48),
            hid_stalls: u16_at(52),
            hid_mouse_dropped: u16_at(54),
        })
    }
}

/// Human-readable name of an ESP-NOW PHY rate code.
pub fn phy_rate_name(code: u8) -> &'static str {
    match code {
        0x00 => "1M",
        0x01 => "2M",
        0x02 => "5.5M",
        0x03 => "11M",
        0x0B => "6M",
        0x0F => "9M",
        0x0A => "12M",
        0x0E => "18M",
        0x09 => "24M",
        0x0D => "36M",
        0x08 => "48M",
        0x0C => "54M",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_has_header_and_zero_padding() {
        let p = packet(msg::KEY_DOWN, 0x0102_0304, &[0x04, 0x02]);
        assert_eq!(&p[..8], &[MAGIC, VERSION, msg::KEY_DOWN, 0, 0x04, 0x03, 0x02, 0x01]);
        assert_eq!(&p[8..10], &[0x04, 0x02]);
        assert!(p[10..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parses_link_stats() {
        let mut payload = vec![STATUS_LINK_STATS, LINK_STATS_LAYOUT, 1, 0x09];
        for v in 1u32..=12 {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        payload.extend_from_slice(&13u16.to_le_bytes());
        payload.extend_from_slice(&14u16.to_le_bytes());
        assert_eq!(payload.len(), 56, "LinkStats is 56 bytes");
        let s = LinkStats::parse(&packet(msg::DAEMON_STATUS, 0, &payload)).unwrap();
        assert!(s.link_up);
        assert_eq!(phy_rate_name(s.phy_rate), "24M");
        assert_eq!((s.session, s.host_received, s.radio_retransmits, s.frames_failed), (1, 2, 5, 12));
        assert_eq!((s.hid_stalls, s.hid_mouse_dropped), (13, 14));
    }

    #[test]
    fn rejects_other_layouts() {
        let p = packet(msg::DAEMON_STATUS, 0, &[STATUS_LINK_STATS, 2, 1]);
        assert_eq!(LinkStats::parse(&p), Err(ParseError::WrongLayout(2)));
    }
}
