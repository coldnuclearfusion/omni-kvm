//! Windows key identity → USB HID usage IDs (keyboard page 0x07).
//!
//! The low-level keyboard hook reports each key's *scan code* (the
//! physical key position, independent of the active layout or IME) plus
//! an "extended" flag that tells keys sharing a scan code apart (Enter vs
//! keypad Enter, the arrow keys vs keypad 8/4/6/2, ...). A few keys are
//! easier to recognize by their virtual-key code.

/// Maps a key to its HID usage ID, or `None` for keys we don't forward.
pub fn hid_usage(vk: u32, scan: u32, extended: bool) -> Option<u8> {
    // Keys better identified by virtual-key code.
    match vk {
        0x13 => return Some(0x48), // VK_PAUSE
        0x90 => return Some(0x53), // VK_NUMLOCK
        0x2C => return Some(0x46), // VK_SNAPSHOT (Print Screen)
        0x15 => return Some(0x90), // VK_HANGUL (한/영) -> LANG1
        0x19 => return Some(0x91), // VK_HANJA (한자) -> LANG2
        _ => {}
    }
    let usage = if extended {
        match scan {
            0x1C => 0x58, // keypad Enter
            0x1D => 0xE4, // right Ctrl
            0x35 => 0x54, // keypad /
            0x38 => 0xE6, // right Alt
            0x47 => 0x4A, // Home
            0x48 => 0x52, // Up
            0x49 => 0x4B, // Page Up
            0x4B => 0x50, // Left
            0x4D => 0x4F, // Right
            0x4F => 0x4D, // End
            0x50 => 0x51, // Down
            0x51 => 0x4E, // Page Down
            0x52 => 0x49, // Insert
            0x53 => 0x4C, // Delete
            0x5B => 0xE3, // left Windows -> left GUI (Command on a Mac)
            0x5C => 0xE7, // right Windows -> right GUI
            0x5D => 0x65, // Menu (Application)
            0x36 => 0xE5, // right Shift (some keyboards report it as extended)
            _ => return None,
        }
    } else {
        match scan {
            0x01 => 0x29, // Esc
            0x02..=0x0A => 0x1E + (scan - 0x02) as u8, // 1..9
            0x0B => 0x27, // 0
            0x0C => 0x2D, // -
            0x0D => 0x2E, // =
            0x0E => 0x2A, // Backspace
            0x0F => 0x2B, // Tab
            0x10 => 0x14, // Q
            0x11 => 0x1A, // W
            0x12 => 0x08, // E
            0x13 => 0x15, // R
            0x14 => 0x17, // T
            0x15 => 0x1C, // Y
            0x16 => 0x18, // U
            0x17 => 0x0C, // I
            0x18 => 0x12, // O
            0x19 => 0x13, // P
            0x1A => 0x2F, // [
            0x1B => 0x30, // ]
            0x1C => 0x28, // Enter
            0x1D => 0xE0, // left Ctrl
            0x1E => 0x04, // A
            0x1F => 0x16, // S
            0x20 => 0x07, // D
            0x21 => 0x09, // F
            0x22 => 0x0A, // G
            0x23 => 0x0B, // H
            0x24 => 0x0D, // J
            0x25 => 0x0E, // K
            0x26 => 0x0F, // L
            0x27 => 0x33, // ;
            0x28 => 0x34, // '
            0x29 => 0x35, // `
            0x2A => 0xE1, // left Shift
            0x2B => 0x31, // backslash
            0x2C => 0x1D, // Z
            0x2D => 0x1B, // X
            0x2E => 0x06, // C
            0x2F => 0x19, // V
            0x30 => 0x05, // B
            0x31 => 0x11, // N
            0x32 => 0x10, // M
            0x33 => 0x36, // ,
            0x34 => 0x37, // .
            0x35 => 0x38, // /
            0x36 => 0xE5, // right Shift
            0x37 => 0x55, // keypad *
            0x38 => 0xE2, // left Alt -> left Alt (Option on a Mac)
            0x39 => 0x2C, // Space
            0x3A => 0x39, // Caps Lock
            0x3B..=0x44 => 0x3A + (scan - 0x3B) as u8, // F1..F10
            0x46 => 0x47, // Scroll Lock
            0x47 => 0x5F, // keypad 7
            0x48 => 0x60, // keypad 8
            0x49 => 0x61, // keypad 9
            0x4A => 0x56, // keypad -
            0x4B => 0x5C, // keypad 4
            0x4C => 0x5D, // keypad 5
            0x4D => 0x5E, // keypad 6
            0x4E => 0x57, // keypad +
            0x4F => 0x59, // keypad 1
            0x50 => 0x5A, // keypad 2
            0x51 => 0x5B, // keypad 3
            0x52 => 0x62, // keypad 0
            0x53 => 0x63, // keypad .
            0x56 => 0x64, // the extra key next to left Shift on ISO keyboards
            0x57 => 0x44, // F11
            0x58 => 0x45, // F12
            _ => return None,
        }
    };
    Some(usage)
}

/// For modifier keys (HID 0xE0..=0xE7), the bit in the HID modifier byte.
pub fn modifier_bit(usage: u8) -> Option<u8> {
    (0xE0..=0xE7).contains(&usage).then(|| 1 << (usage - 0xE0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_common_keys() {
        assert_eq!(hid_usage(0x41, 0x1E, false), Some(0x04)); // A
        assert_eq!(hid_usage(0x5A, 0x2C, false), Some(0x1D)); // Z
        assert_eq!(hid_usage(0x31, 0x02, false), Some(0x1E)); // 1
        assert_eq!(hid_usage(0x30, 0x0B, false), Some(0x27)); // 0
        assert_eq!(hid_usage(0xBF, 0x35, false), Some(0x38)); // /
        assert_eq!(hid_usage(0x0D, 0x1C, false), Some(0x28)); // Enter
        assert_eq!(hid_usage(0x7A, 0x57, false), Some(0x44)); // F11
    }

    #[test]
    fn extended_flag_tells_keys_apart() {
        assert_eq!(hid_usage(0x0D, 0x1C, true), Some(0x58)); // keypad Enter
        assert_eq!(hid_usage(0x26, 0x48, true), Some(0x52)); // Up arrow
        assert_eq!(hid_usage(0x68, 0x48, false), Some(0x60)); // keypad 8
        assert_eq!(hid_usage(0xA3, 0x1D, true), Some(0xE4)); // right Ctrl
        assert_eq!(hid_usage(0x5B, 0x5B, true), Some(0xE3)); // left Windows
    }

    #[test]
    fn modifier_bits_match_the_hid_modifier_byte() {
        assert_eq!(modifier_bit(0xE0), Some(0x01)); // left Ctrl
        assert_eq!(modifier_bit(0xE1), Some(0x02)); // left Shift
        assert_eq!(modifier_bit(0xE7), Some(0x80)); // right GUI
        assert_eq!(modifier_bit(0x04), None);
    }
}
