//! The other computer's daemon: the messages the two daemons exchange
//! (relayed by the boards, see shared/protocol.md) and what this daemon
//! currently knows about the other side.
//!
//! The daemon of the computer whose keyboard and mouse are in use decides
//! when the pointer changes screens. The other daemon helps: it places its
//! pointer where a handoff says, and reports when its pointer touches a
//! screen edge, so the first one can bring the pointer back.

// Until both sides can lead (the Mac captures input from Phase 4, step 3),
// each OS uses only its half of this module.
#![allow(dead_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crate::protocol::{PACKET_SIZE, msg};

/// Screen edges, as `MSG_HANDOFF` names the one the pointer left by.
pub const EDGE_RIGHT: u8 = 0x01;
pub const EDGE_LEFT: u8 = 0x02;

/// Bits of `MSG_EDGE_CONTACT`: which edges the pointer is touching.
pub const CONTACT_RIGHT: u8 = 1 << 0;
pub const CONTACT_LEFT: u8 = 1 << 1;
pub const CONTACT_TOP: u8 = 1 << 2;
pub const CONTACT_BOTTOM: u8 = 1 << 3;

/// The other daemon counts as running if heard from this recently. It
/// sends `MSG_EDGE_CONTACT` at least once a second.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerMsg {
    /// "The pointer is now yours": it left the sender's screen by `edge`,
    /// at `position` along it (see [`fraction`]). `id` tells a repeat
    /// apart from a new handoff.
    Handoff { edge: u8, id: u8, position: u16 },
    HandoffAck { id: u8, accepted: bool },
    /// "My pointer is touching these edges (`CONTACT_*` bits), at
    /// `position` along the left/right edge."
    EdgeContact { edges: u8, position: u16 },
}

impl PeerMsg {
    /// Message type and payload.
    pub fn encode(&self) -> (u8, Vec<u8>) {
        match *self {
            PeerMsg::Handoff { edge, id, position } => {
                let [lo, hi] = position.to_le_bytes();
                (msg::HANDOFF, vec![edge, id, lo, hi])
            }
            PeerMsg::HandoffAck { id, accepted } => (msg::HANDOFF_ACK, vec![accepted as u8, id]),
            PeerMsg::EdgeContact { edges, position } => {
                let [lo, hi] = position.to_le_bytes();
                (msg::EDGE_CONTACT, vec![edges, 0, lo, hi])
            }
        }
    }

    /// Reads a relayed packet, or `None` for other packets.
    pub fn decode(packet: &[u8; PACKET_SIZE]) -> Option<PeerMsg> {
        let p = &packet[crate::protocol::HEADER_SIZE..];
        let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
        match packet[2] {
            msg::HANDOFF => Some(PeerMsg::Handoff { edge: p[0], id: p[1], position: u16_at(2) }),
            msg::HANDOFF_ACK => Some(PeerMsg::HandoffAck { accepted: p[0] != 0, id: p[1] }),
            msg::EDGE_CONTACT => Some(PeerMsg::EdgeContact { edges: p[0], position: u16_at(2) }),
            _ => None,
        }
    }
}

/// Where `value` lies between `min` and `max`, as 0..=65535. Screens of
/// different sizes agree on "40% of the way down" without knowing each
/// other's resolution.
pub fn fraction(value: f64, min: f64, max: f64) -> u16 {
    if max <= min {
        return 0;
    }
    (((value - min) / (max - min)).clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// The inverse of [`fraction`].
pub fn from_fraction(position: u16, min: f64, max: f64) -> f64 {
    min + (max - min) * position as f64 / 65535.0
}

/// What this daemon knows about the other side, shared between the board
/// thread (which hears from it) and the input side.
pub struct Peer {
    link_up: AtomicBool,
    heard: Mutex<Option<Instant>>,
    contact: AtomicU32, // edges << 16 | position
}

impl Peer {
    pub fn new() -> Peer {
        Peer { link_up: AtomicBool::new(false), heard: Mutex::new(None), contact: AtomicU32::new(0) }
    }

    /// The radio link between the two boards.
    pub fn link_up(&self) -> bool {
        self.link_up.load(Ordering::Relaxed)
    }

    pub fn set_link_up(&self, up: bool) {
        self.link_up.store(up, Ordering::Relaxed);
    }

    /// Records that a message from the other daemon arrived.
    pub fn heard(&self) {
        *self.heard.lock().unwrap() = Some(Instant::now());
    }

    /// Is the other daemon running (not just its board)?
    pub fn daemon_alive(&self) -> bool {
        self.link_up() && self.heard.lock().unwrap().is_some_and(|t| t.elapsed() < DAEMON_TIMEOUT)
    }

    pub fn set_contact(&self, edges: u8, position: u16) {
        self.contact.store((edges as u32) << 16 | position as u32, Ordering::Relaxed);
    }

    /// Where along the edge the other pointer touches `edge` (a
    /// `CONTACT_*` bit), if it does.
    pub fn touching(&self, edge: u8) -> Option<u16> {
        let c = self.contact.load(Ordering::Relaxed);
        ((c >> 16) as u8 & edge != 0).then_some(c as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::packet;

    #[test]
    fn messages_survive_a_round_trip() {
        for m in [
            PeerMsg::Handoff { edge: EDGE_RIGHT, id: 7, position: 40_000 },
            PeerMsg::HandoffAck { id: 7, accepted: true },
            PeerMsg::EdgeContact { edges: CONTACT_LEFT | CONTACT_TOP, position: 65_535 },
        ] {
            let (msg_type, payload) = m.encode();
            assert!(payload.len() <= 28, "relayed data must fit the radio");
            assert_eq!(PeerMsg::decode(&packet(msg_type, 0, &payload)), Some(m));
        }
        assert_eq!(PeerMsg::decode(&packet(msg::KEY_DOWN, 0, &[4, 0])), None);
    }

    #[test]
    fn fractions_map_between_screens() {
        assert_eq!(fraction(0.0, 0.0, 1079.0), 0);
        assert_eq!(fraction(1079.0, 0.0, 1079.0), 65535);
        assert_eq!(fraction(-5.0, 0.0, 1079.0), 0);
        // 40% down a 1080-pixel-high screen is 40% down a 956-point one.
        let f = fraction(0.4 * 1079.0, 0.0, 1079.0);
        assert!((from_fraction(f, 0.0, 955.0) - 0.4 * 955.0).abs() < 0.01);
    }

    #[test]
    fn contact_is_per_edge() {
        let peer = Peer::new();
        assert_eq!(peer.touching(CONTACT_LEFT), None);
        peer.set_contact(CONTACT_LEFT, 1234);
        assert_eq!(peer.touching(CONTACT_LEFT), Some(1234));
        assert_eq!(peer.touching(CONTACT_RIGHT), None);
    }
}
