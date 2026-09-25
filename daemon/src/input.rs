//! What the board thread is asked to send, by the input side (one module
//! per OS). Platform-independent: `main.rs` turns these into protocol
//! packets.

use crate::peer::PeerMsg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Input capture exists only on Windows so far (macOS: Phase 4, step 3).
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Event {
    Key { usage: u8, down: bool, modifiers: u8 },
    Mouse { dx: i32, dy: i32, buttons: u8 },
    Scroll { vertical: i16, horizontal: i16 },
    ModifierSync(u8),
    /// For the other computer's daemon.
    Peer(PeerMsg),
}
