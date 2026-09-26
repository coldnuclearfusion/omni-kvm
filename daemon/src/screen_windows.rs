//! Windows pointer handling: where the pointer enters after an edge switch.
//!
//! The PC's pointer needs nothing to stay put while the PC is unfocused:
//! its hooks swallow its own mouse, and its board's gate is closed.
//!
//! Coordinates are those of the virtual screen (all monitors together).
//! This program does not declare DPI awareness, so Windows scales them
//! consistently for every call here.

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SetCursorPos,
};

use crate::focus::Pointer;
use crate::protocol;

/// An entering pointer appears this far inside the edge, so it is not
/// pushing against the edge the moment it arrives.
const ENTRY_INSET: i32 = 40;

/// (left, top, right, bottom) of the virtual screen, right and bottom
/// being the last pixel inside.
pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        (x, y, x + w - 1, y + h - 1)
    }
}

pub fn cursor_pos() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    p
}

/// Carries out a pointer action of the focus state machine.
pub fn pointer(action: Pointer) {
    if let Pointer::Enter(entry) = action {
        // Just inside the PC's facing (right) edge, at the same fraction
        // of the height as where the pointer left the Mac.
        let (_, top, right, bottom) = virtual_screen();
        let y = protocol::from_fraction(entry.height, top as f64, bottom as f64).round() as i32;
        unsafe { SetCursorPos(right - ENTRY_INSET, y) };
    }
}
