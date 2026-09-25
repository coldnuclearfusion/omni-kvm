//! Input events, as the capture side (one module per OS) hands them to the
//! board thread. Platform-independent: `main.rs` turns them into protocol
//! packets.

/// What the capture side asks the board to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Key { usage: u8, down: bool, modifiers: u8 },
    Mouse { dx: i32, dy: i32, buttons: u8 },
    Scroll { vertical: i16, horizontal: i16 },
    ModifierSync(u8),
}
