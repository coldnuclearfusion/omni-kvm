//! macOS key codes → USB HID usage IDs (keyboard page 0x07).
//!
//! A Quartz keyboard event carries a *virtual key code* (the `kVK_*`
//! constants in Carbon's Events.h). Despite the name it identifies the
//! physical key, independent of the input source (ABC, 2-Set Korean, ...),
//! like a Windows scan code.
//!
//! Command is sent as GUI (the Windows key) and Option as Alt: the same
//! physical keys a Windows keyboard reports there, and what Apple
//! keyboards report over USB.

/// Maps a key to its HID usage ID, or `None` for keys we don't forward
/// (fn, and codes no Mac keyboard sends).
pub fn hid_usage(keycode: u16) -> Option<u8> {
    let usage = match keycode {
        0x00 => 0x04, // A
        0x01 => 0x16, // S
        0x02 => 0x07, // D
        0x03 => 0x09, // F
        0x04 => 0x0B, // H
        0x05 => 0x0A, // G
        0x06 => 0x1D, // Z
        0x07 => 0x1B, // X
        0x08 => 0x06, // C
        0x09 => 0x19, // V
        0x0A => 0x64, // ISO § (next to Z on ISO keyboards: "non-US \")
        0x0B => 0x05, // B
        0x0C => 0x14, // Q
        0x0D => 0x1A, // W
        0x0E => 0x08, // E
        0x0F => 0x15, // R
        0x10 => 0x1C, // Y
        0x11 => 0x17, // T
        0x12 => 0x1E, // 1
        0x13 => 0x1F, // 2
        0x14 => 0x20, // 3
        0x15 => 0x21, // 4
        0x16 => 0x23, // 6
        0x17 => 0x22, // 5
        0x18 => 0x2E, // =
        0x19 => 0x26, // 9
        0x1A => 0x24, // 7
        0x1B => 0x2D, // -
        0x1C => 0x25, // 8
        0x1D => 0x27, // 0
        0x1E => 0x30, // ]
        0x1F => 0x12, // O
        0x20 => 0x18, // U
        0x21 => 0x2F, // [
        0x22 => 0x0C, // I
        0x23 => 0x13, // P
        0x24 => 0x28, // Return
        0x25 => 0x0F, // L
        0x26 => 0x0D, // J
        0x27 => 0x34, // '
        0x28 => 0x0E, // K
        0x29 => 0x33, // ;
        0x2A => 0x31, // \
        0x2B => 0x36, // ,
        0x2C => 0x38, // /
        0x2D => 0x11, // N
        0x2E => 0x10, // M
        0x2F => 0x37, // .
        0x30 => 0x2B, // Tab
        0x31 => 0x2C, // Space
        0x32 => 0x35, // `
        0x33 => 0x2A, // Delete (Backspace)
        0x35 => 0x29, // Escape
        0x36 => 0xE7, // right Command -> right GUI
        0x37 => 0xE3, // Command -> left GUI
        0x38 => 0xE1, // Shift
        0x39 => 0x39, // Caps Lock
        0x3A => 0xE2, // Option -> left Alt
        0x3B => 0xE0, // Control
        0x3C => 0xE5, // right Shift
        0x3D => 0xE6, // right Option -> right Alt
        0x3E => 0xE4, // right Control
        0x40 => 0x6C, // F17
        0x41 => 0x63, // keypad .
        0x43 => 0x55, // keypad *
        0x45 => 0x57, // keypad +
        0x47 => 0x53, // keypad Clear -> Num Lock
        0x48 => 0x80, // Volume Up
        0x49 => 0x81, // Volume Down
        0x4A => 0x7F, // Mute
        0x4B => 0x54, // keypad /
        0x4C => 0x58, // keypad Enter
        0x4E => 0x56, // keypad -
        0x4F => 0x6D, // F18
        0x50 => 0x6E, // F19
        0x51 => 0x67, // keypad =
        0x52 => 0x62, // keypad 0
        0x53 => 0x59, // keypad 1
        0x54 => 0x5A, // keypad 2
        0x55 => 0x5B, // keypad 3
        0x56 => 0x5C, // keypad 4
        0x57 => 0x5D, // keypad 5
        0x58 => 0x5E, // keypad 6
        0x59 => 0x5F, // keypad 7
        0x5A => 0x6F, // F20
        0x5B => 0x60, // keypad 8
        0x5C => 0x61, // keypad 9
        0x5D => 0x89, // JIS ¥
        0x5E => 0x87, // JIS _
        0x5F => 0x85, // JIS keypad ,
        0x60 => 0x3E, // F5
        0x61 => 0x3F, // F6
        0x62 => 0x40, // F7
        0x63 => 0x3C, // F3
        0x64 => 0x41, // F8
        0x65 => 0x42, // F9
        0x66 => 0x91, // JIS Eisu -> LANG2
        0x67 => 0x44, // F11
        0x68 => 0x90, // JIS Kana -> LANG1
        0x69 => 0x68, // F13
        0x6A => 0x6B, // F16
        0x6B => 0x69, // F14
        0x6D => 0x43, // F10
        0x6E => 0x65, // context menu -> Application
        0x6F => 0x45, // F12
        0x71 => 0x6A, // F15
        0x72 => 0x49, // Help -> Insert (same position)
        0x73 => 0x4A, // Home
        0x74 => 0x4B, // Page Up
        0x75 => 0x4C, // Forward Delete
        0x76 => 0x3D, // F4
        0x77 => 0x4D, // End
        0x78 => 0x3B, // F2
        0x79 => 0x4E, // Page Down
        0x7A => 0x3A, // F1
        0x7B => 0x50, // Left
        0x7C => 0x4F, // Right
        0x7D => 0x51, // Down
        0x7E => 0x52, // Up
        _ => return None, // incl. 0x3F fn
    };
    Some(usage)
}

/// The Quartz event flags of each modifier key, in HID modifier-byte
/// order: the device-dependent bit, which tells left and right apart
/// (NX_DEVICE*KEYMASK in IOKit's IOLLEvent.h), and the device-independent
/// mask (kCGEventFlagMask*).
const MODIFIER_FLAGS: [(u64, u64); 8] = [
    (0x0000_0001, 0x0004_0000), // left Control
    (0x0000_0002, 0x0002_0000), // left Shift
    (0x0000_0020, 0x0008_0000), // left Option -> left Alt
    (0x0000_0008, 0x0010_0000), // left Command -> left GUI
    (0x0000_2000, 0x0004_0000), // right Control
    (0x0000_0004, 0x0002_0000), // right Shift
    (0x0000_0040, 0x0008_0000), // right Option -> right Alt
    (0x0000_0010, 0x0010_0000), // right Command -> right GUI
];
const DEVICE_BITS: u64 = 0x207F;

/// Is the modifier key with this HID usage (0xE0..=0xE7) down, going by
/// the flags of the "flags changed" event that reports it? For a keyboard
/// whose events carry no device-dependent bits, the device-independent
/// ones decide (left and right then look the same).
pub fn modifier_down(flags: u64, usage: u8) -> bool {
    let (device, independent) = MODIFIER_FLAGS[(usage - 0xE0) as usize];
    if flags & DEVICE_BITS != 0 { flags & device != 0 } else { flags & independent != 0 }
}

/// The Quartz event flags for the modifiers in a HID modifier byte.
pub fn modifier_flags(modifiers: u8) -> u64 {
    MODIFIER_FLAGS
        .iter()
        .enumerate()
        .filter(|(bit, _)| modifiers & 1 << bit != 0)
        .fold(0, |flags, (_, (device, independent))| flags | device | independent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_controls() {
        assert_eq!(hid_usage(0x00), Some(0x04)); // A
        assert_eq!(hid_usage(0x06), Some(0x1D)); // Z
        assert_eq!(hid_usage(0x24), Some(0x28)); // Return
        assert_eq!(hid_usage(0x31), Some(0x2C)); // Space
        assert_eq!(hid_usage(0x7E), Some(0x52)); // Up
        assert_eq!(hid_usage(0x3F), None); // fn
    }

    #[test]
    fn modifier_keys_match_their_flag_bits() {
        // Each modifier key (by its HID usage) is down exactly when its own
        // device-dependent bit is set, and the flags for its HID modifier
        // bit (1 << (usage - 0xE0)) contain that bit.
        for (keycode, flag) in [
            (0x3Bu16, 0x0001u64),
            (0x38, 0x0002),
            (0x3A, 0x0020),
            (0x37, 0x0008),
            (0x3E, 0x2000),
            (0x3C, 0x0004),
            (0x3D, 0x0040),
            (0x36, 0x0010),
        ] {
            let usage = hid_usage(keycode).unwrap();
            assert!(modifier_down(flag | 0x0010_0100, usage), "keycode {keycode:#x}");
            assert_eq!(modifier_flags(1 << (usage - 0xE0)) & DEVICE_BITS, flag, "keycode {keycode:#x}");
            for other in (0xE0..=0xE7).filter(|&u| u != usage) {
                assert!(!modifier_down(flag, other), "keycode {keycode:#x} is not {other:#x}");
            }
        }
    }

    #[test]
    fn modifier_flags_include_the_device_independent_masks() {
        assert_eq!(modifier_flags(0x02), 0x0002_0002); // left Shift
        assert_eq!(modifier_flags(0x88), 0x0010_0018); // both Commands
        assert_eq!(modifier_flags(0), 0);
        // Without device-dependent bits, the device-independent mask decides.
        assert!(modifier_down(0x0002_0000, 0xE1));
        assert!(!modifier_down(0x0004_0000, 0xE1));
    }
}
