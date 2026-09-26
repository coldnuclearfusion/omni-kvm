//! What both platforms' input capture share: how keys and buttons are
//! named for the focus state machine, and the key state message.
//!
//! The state machine sends each release where its press went (D7), so it
//! remembers presses by an id:
//! - a key with a USB HID usage (keyboard page 0x07): that usage;
//! - a mouse button: [`BUTTON`] | its bit in the MSG_MOUSE_MOVE buttons byte;
//! - a key with no HID usage (never forwarded, only kept or passed):
//!   [`OTHER_KEY`] | the operating system's own code for it.

pub const BUTTON: u32 = 0x100;
pub const OTHER_KEY: u32 = 0x1_0000;

pub fn key_id(usage: Option<u8>, native_code: u32) -> u32 {
    usage.map_or(OTHER_KEY | native_code, u32::from)
}

pub fn button_id(bit: u8) -> u32 {
    BUTTON | bit as u32
}

/// For modifier keys (HID 0xE0..=0xE7), the bit in the HID modifier byte.
pub fn modifier_bit(usage: u8) -> Option<u8> {
    (0xE0..=0xE7).contains(&usage).then(|| 1 << (usage - 0xE0))
}

/// Payload of MSG_KEY_STATE for the keys and buttons still held whose
/// press was forwarded (by id). `None` when more keys are held than a
/// keyboard report holds (6): the message is then skipped this time, as
/// it could not describe the state.
pub fn key_state(forwarded_held: impl Iterator<Item = u32>) -> Option<[u8; 8]> {
    let mut state = [0u8; 8]; // modifiers, buttons, keys[6]
    let mut keys = 0;
    for id in forwarded_held {
        if id < BUTTON {
            let usage = id as u8;
            if let Some(bit) = modifier_bit(usage) {
                state[0] |= bit;
            } else {
                if keys == 6 {
                    return None;
                }
                state[2 + keys] = usage;
                keys += 1;
            }
        } else if id < BUTTON + 0x100 {
            state[1] |= (id - BUTTON) as u8;
        }
        // OTHER_KEY: swallowed, never sent, so nothing to repair.
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_bits_match_the_hid_modifier_byte() {
        assert_eq!(modifier_bit(0xE0), Some(0x01)); // left Ctrl
        assert_eq!(modifier_bit(0xE1), Some(0x02)); // left Shift
        assert_eq!(modifier_bit(0xE7), Some(0x80)); // right GUI
        assert_eq!(modifier_bit(0x04), None);
    }

    #[test]
    fn key_state_sorts_ids_into_the_message() {
        let ids = [0xE1, 0x04, button_id(0x01), 0x2C, key_id(None, 0xFF), button_id(0x10)];
        assert_eq!(key_state(ids.into_iter()), Some([0x02, 0x11, 0x04, 0x2C, 0, 0, 0, 0]));
        assert_eq!(key_state(std::iter::empty()), Some([0; 8]));
        let seven_keys = 0x04u32..0x0B;
        assert_eq!(key_state(seven_keys), None, "more than a report can say");
    }
}
