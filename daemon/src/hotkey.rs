//! Recognizing the switch hotkey (requirement H1): Escape going down while
//! a GUI key (Windows / Command) is held, with no other key pressed since
//! the GUI key went down. Cancelling a Win+Tab or Cmd+Tab switcher with
//! Escape therefore does not count. (Scroll Lock, H2, is a single key: the
//! Windows capture checks for it itself.)
//!
//! Fed with every key press and release the capture sees, by HID usage.

const LEFT_GUI: u8 = 0xE3;
const RIGHT_GUI: u8 = 0xE7;
const ESCAPE: u8 = 0x29;

#[derive(Debug, Default)]
pub struct Watch {
    /// GUI keys held: bit 0 left, bit 1 right.
    gui: u8,
    /// No other key pressed since a GUI key went down.
    clean: bool,
}

fn gui_bit(usage: u8) -> u8 {
    if usage == LEFT_GUI { 1 } else { 2 }
}

impl Watch {
    /// A key went down: a new press, not an auto-repeat. True if it
    /// completes the hotkey.
    pub fn press(&mut self, usage: Option<u8>) -> bool {
        match usage {
            Some(u @ (LEFT_GUI | RIGHT_GUI)) => {
                self.gui |= gui_bit(u);
                self.clean = true;
                false
            }
            Some(ESCAPE) => {
                let completes = self.gui != 0 && self.clean;
                self.clean = false;
                completes
            }
            _ => {
                self.clean = false;
                false
            }
        }
    }

    pub fn release(&mut self, usage: Option<u8>) {
        if let Some(u @ (LEFT_GUI | RIGHT_GUI)) = usage {
            self.gui &= !gui_bit(u);
            if self.gui == 0 {
                self.clean = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: u8 = 0x2B;
    const LEFT_SHIFT: u8 = 0xE1;

    #[test]
    fn gui_then_escape() {
        let mut w = Watch::default();
        assert!(!w.press(Some(LEFT_GUI)));
        assert!(w.press(Some(ESCAPE)));
        assert!(!w.press(Some(ESCAPE)), "once per GUI press");
        w.release(Some(LEFT_GUI));
        assert!(!w.press(Some(RIGHT_GUI)));
        assert!(w.press(Some(ESCAPE)), "right GUI too");
    }

    #[test]
    fn another_key_in_between_does_not_count() {
        let mut w = Watch::default();
        w.press(Some(LEFT_GUI));
        w.press(Some(TAB)); // Win+Tab / Cmd+Tab
        assert!(!w.press(Some(ESCAPE)), "Escape closes the switcher instead");
    }

    #[test]
    fn needs_gui_held() {
        let mut w = Watch::default();
        assert!(!w.press(Some(ESCAPE)));
        w.press(Some(LEFT_GUI));
        w.release(Some(LEFT_GUI));
        assert!(!w.press(Some(ESCAPE)));
        assert!(!w.press(None), "keys without a usage count as other keys");
    }

    #[test]
    fn keys_held_before_gui_do_not_matter() {
        let mut w = Watch::default();
        w.press(Some(LEFT_SHIFT));
        w.press(Some(LEFT_GUI));
        assert!(w.press(Some(ESCAPE)));
    }
}
