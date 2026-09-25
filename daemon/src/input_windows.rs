//! Windows input capture: low-level hooks plus Raw Input.
//!
//! **Local** mode: input goes to Windows as usual; we only watch it.
//! Pushing the pointer past the right edge of the screen (with some
//! "virtual resistance") switches to **Remote** mode: keyboard and mouse
//! input is swallowed, so Windows never sees it, and forwarded to the
//! board as protocol events. If the Mac's daemon is running, a handoff
//! puts the Mac pointer at the same height, and pushing it past the Mac's
//! left edge comes back. Scroll Lock toggles between the two modes, and
//! a lost radio link falls back to Local.
//!
//! Mouse movement comes from Raw Input: the counts the mouse reported,
//! before Windows pointer acceleration, because the Mac applies its own
//! (see shared/protocol.md, "Keep batching windows short").
//!
//! This module never records what is typed: in Local mode it only tracks
//! which keys are held, so they can be released cleanly on a switch.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_SCROLL;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::input::Event;
use crate::keymap;
use crate::peer::{self, Peer, PeerMsg};

/// Raw mouse counts pushed against an edge before switching.
const EDGE_RESISTANCE: i32 = 60;
/// Sent on entering Remote when the Mac has no daemon to place its
/// pointer, so it at least starts somewhere on the left edge.
const SLAM_LEFT: i32 = -5000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Local,
    Remote,
}

struct State {
    mode: Mode,
    tx: Sender<Event>,
    peer: Arc<Peer>,
    handoff_id: u8,
    // Local mode
    local_keys: HashSet<u32>, // virtual-key codes held on Windows
    local_buttons: u8,
    edge_push: i32,
    // Remote mode: pushing past the Mac's left edge
    return_push: i32,
    // Remote mode
    keys_held_at_switch: HashSet<u32>, // their key-up must still reach Windows
    buttons_held_at_switch: u8,
    remote_keys: HashSet<u8>, // HID usages held on the remote side
    modifiers: u8,
    buttons: u8,
    wheel: i32,
    hwheel: i32,
    entry_y: i32,
    scroll_lock_held: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Something to do after the state borrow is released (Win32 calls that
/// could, in principle, re-enter our hooks).
enum After {
    Nothing,
    MoveCursor(i32, i32),
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        (x, y, x + w - 1, y + h - 1) // left, top, right, bottom
    }
}

fn cursor_pos() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    p
}

impl State {
    fn send(&self, event: Event) {
        let _ = self.tx.send(event); // the board thread is gone only when we are exiting
    }

    fn enter_remote(&mut self) {
        if !self.peer.link_up() {
            println!("[input] radio link is down; staying on Windows");
            return;
        }
        self.mode = Mode::Remote;
        self.keys_held_at_switch = self.local_keys.clone();
        self.buttons_held_at_switch = self.local_buttons;
        self.remote_keys.clear();
        self.modifiers = 0;
        self.buttons = 0;
        self.wheel = 0;
        self.hwheel = 0;
        self.return_push = 0;
        self.entry_y = cursor_pos().y;
        if self.peer.daemon_alive() {
            // The Mac pointer moves away from wherever it was: forget any
            // edge it touched until the Mac reports again.
            self.peer.set_contact(0, 0);
            self.handoff_id = self.handoff_id.wrapping_add(1);
            let (_, top, _, bottom) = virtual_screen();
            let position = peer::fraction(self.entry_y as f64, top as f64, bottom as f64);
            self.send(Event::Peer(PeerMsg::Handoff { edge: peer::EDGE_RIGHT, id: self.handoff_id, position }));
            println!("[input] -> Mac  (push past its left edge, or Scroll Lock, to come back)");
        } else {
            self.send(Event::Mouse { dx: SLAM_LEFT, dy: 0, buttons: 0 });
            println!("[input] -> Mac  (Scroll Lock to come back; start the Mac daemon to use its left edge)");
        }
    }

    /// Releases everything held on the remote side and gives input back to
    /// Windows, with the pointer at height `y` (default: where it left).
    fn leave_remote(&mut self, why: &str, y: Option<i32>) -> After {
        for &usage in &self.remote_keys {
            self.send(Event::Key { usage, down: false, modifiers: 0 });
        }
        self.send(Event::ModifierSync(0));
        if self.buttons != 0 {
            self.send(Event::Mouse { dx: 0, dy: 0, buttons: 0 });
        }
        self.remote_keys.clear();
        self.modifiers = 0;
        self.buttons = 0;
        self.mode = Mode::Local;
        self.edge_push = 0;
        println!("[input] <- Windows ({why})");
        let (_, _, right, _) = virtual_screen();
        After::MoveCursor(right - 40, y.unwrap_or(self.entry_y)) // a little away from the edge
    }

    fn check_link(&mut self) -> After {
        if self.mode == Mode::Remote && !self.peer.link_up() {
            return self.leave_remote("radio link lost", None);
        }
        After::Nothing
    }

    /// Returns (swallow, after).
    fn on_key(&mut self, vk: u32, scan: u32, extended: bool, down: bool) -> (bool, After) {
        if vk == VK_SCROLL as u32 {
            let mut after = After::Nothing;
            if down && !self.scroll_lock_held {
                after = match self.mode {
                    Mode::Local => {
                        self.enter_remote();
                        After::Nothing
                    }
                    Mode::Remote => self.leave_remote("Scroll Lock", None),
                };
            }
            self.scroll_lock_held = down;
            return (true, after); // never let Scroll Lock itself through
        }

        if down {
            self.local_keys.insert(vk);
        } else {
            self.local_keys.remove(&vk);
        }

        let after = self.check_link();
        if self.mode == Mode::Local {
            return (false, after);
        }
        // A key pressed on Windows before the switch: Windows must see it go up.
        if !down && self.keys_held_at_switch.remove(&vk) {
            return (false, after);
        }
        let Some(usage) = keymap::hid_usage(vk, scan, extended) else {
            return (true, after); // unknown key: swallow, don't forward
        };
        if let Some(bit) = keymap::modifier_bit(usage) {
            if down {
                self.modifiers |= bit;
            } else {
                self.modifiers &= !bit;
            }
        } else if down {
            if !self.remote_keys.insert(usage) {
                return (true, after); // Windows auto-repeat; the Mac repeats on its own
            }
        } else {
            self.remote_keys.remove(&usage);
        }
        self.send(Event::Key { usage, down, modifiers: self.modifiers });
        (true, after)
    }

    /// Low-level mouse hook: decides what Windows gets to see.
    fn on_mouse_hook(&mut self, msg: u32, mouse_data: u32) -> bool {
        let button = match msg {
            WM_LBUTTONDOWN | WM_LBUTTONUP => 0x01,
            WM_RBUTTONDOWN | WM_RBUTTONUP => 0x02,
            WM_MBUTTONDOWN | WM_MBUTTONUP => 0x04,
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                if (mouse_data >> 16) == 1 { 0x08 } else { 0x10 }
            }
            _ => 0,
        };
        let down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN);
        if button != 0 {
            if down {
                self.local_buttons |= button;
            } else {
                self.local_buttons &= !button;
            }
        }
        if self.mode == Mode::Local {
            return false;
        }
        // A button pressed on Windows before the switch: Windows must see it go up.
        if button != 0 && !down && self.buttons_held_at_switch & button != 0 {
            self.buttons_held_at_switch &= !button;
            return false;
        }
        true // Remote: Windows sees no pointer movement, clicks, or wheel
    }

    /// Raw Input: what the mouse actually reported, before acceleration.
    fn on_raw_mouse(&mut self, dx: i32, dy: i32, flags: u32, data: i16) -> After {
        let after = self.check_link();
        if self.mode == Mode::Local {
            let (_, _, right, _) = virtual_screen();
            if cursor_pos().x >= right && dx > 0 {
                self.edge_push += dx;
                if self.edge_push >= EDGE_RESISTANCE {
                    self.enter_remote();
                }
            } else if dx < 0 || cursor_pos().x < right {
                self.edge_push = 0;
            }
            return after;
        }

        let before = self.buttons;
        for (down_flag, up_flag, bit) in [
            (RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, 0x01u8),
            (RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, 0x02),
            (RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP, 0x04),
            (RI_MOUSE_BUTTON_4_DOWN, RI_MOUSE_BUTTON_4_UP, 0x08),
            (RI_MOUSE_BUTTON_5_DOWN, RI_MOUSE_BUTTON_5_UP, 0x10),
        ] {
            if flags & down_flag != 0 {
                self.buttons |= bit;
            }
            if flags & up_flag != 0 {
                self.buttons &= !bit;
            }
        }
        if dx != 0 || dy != 0 || self.buttons != before {
            self.send(Event::Mouse { dx, dy, buttons: self.buttons });
        }

        // Pushing past the Mac's left edge: the Mac daemon reports when its
        // pointer touches the edge, and further leftward counts build up
        // to the same resistance as on the way over.
        if let Some(position) = self.peer.touching(peer::CONTACT_LEFT) {
            if dx < 0 {
                self.return_push -= dx;
                if self.return_push >= EDGE_RESISTANCE {
                    let (_, top, _, bottom) = virtual_screen();
                    let y = peer::from_fraction(position, top as f64, bottom as f64).round() as i32;
                    return self.leave_remote("Mac's left edge", Some(y));
                }
            } else if dx > 0 {
                self.return_push = 0;
            }
        } else {
            self.return_push = 0;
        }

        // Wheel: WHEEL_DELTA (120) per notch; high-resolution wheels send less.
        let (mut vertical, mut horizontal) = (0i16, 0i16);
        if flags & RI_MOUSE_WHEEL != 0 {
            self.wheel += data as i32;
            vertical = (self.wheel / WHEEL_DELTA as i32) as i16;
            self.wheel -= vertical as i32 * WHEEL_DELTA as i32;
        }
        if flags & RI_MOUSE_HWHEEL != 0 {
            self.hwheel += data as i32;
            horizontal = (self.hwheel / WHEEL_DELTA as i32) as i16;
            self.hwheel -= horizontal as i32 * WHEEL_DELTA as i32;
        }
        if vertical != 0 || horizontal != 0 {
            self.send(Event::Scroll { vertical, horizontal });
        }
        after
    }
}

fn apply(after: After) {
    if let After::MoveCursor(x, y) = after {
        unsafe { SetCursorPos(x, y) };
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // Leave input injected by other software alone.
        if kb.flags & LLKHF_INJECTED == 0 {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let extended = kb.flags & LLKHF_EXTENDED != 0;
            let result = STATE.with(|s| {
                s.borrow_mut().as_mut().map(|st| st.on_key(kb.vkCode, kb.scanCode, extended, down))
            });
            if let Some((swallow, after)) = result {
                apply(after);
                if swallow {
                    return 1;
                }
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let ms = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        if ms.flags & LLMHF_INJECTED == 0 {
            let swallow = STATE.with(|s| {
                s.borrow_mut().as_mut().is_some_and(|st| st.on_mouse_hook(wparam as u32, ms.mouseData))
            });
            if swallow {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        let mut raw: RAWINPUT = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let read = unsafe {
            GetRawInputData(
                lparam as HRAWINPUT,
                RID_INPUT,
                &mut raw as *mut RAWINPUT as *mut _,
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if read != u32::MAX && raw.header.dwType == RIM_TYPEMOUSE {
            let m = unsafe { raw.data.mouse };
            // Absolute devices (pen tablets, remote desktop) are not forwarded.
            if m.usFlags & MOUSE_MOVE_ABSOLUTE == 0 {
                let (flags, data) = unsafe { (m.Anonymous.Anonymous.usButtonFlags, m.Anonymous.Anonymous.usButtonData) };
                let after = STATE.with(|s| {
                    s.borrow_mut()
                        .as_mut()
                        .map(|st| st.on_raw_mouse(m.lLastX, m.lLastY, flags as u32, data as i16))
                });
                if let Some(after) = after {
                    apply(after);
                }
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Installs the hooks and runs the message loop on the calling thread,
/// which must be the thread that keeps pumping messages. Never returns
/// normally; the hooks disappear with the process.
pub fn run(tx: Sender<Event>, peer: Arc<Peer>) -> Result<(), String> {
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            mode: Mode::Local,
            tx,
            peer,
            handoff_id: 0,
            local_keys: HashSet::new(),
            local_buttons: 0,
            edge_push: 0,
            return_push: 0,
            keys_held_at_switch: HashSet::new(),
            buttons_held_at_switch: 0,
            remote_keys: HashSet::new(),
            modifiers: 0,
            buttons: 0,
            wheel: 0,
            hwheel: 0,
            entry_y: 0,
            scroll_lock_held: false,
        })
    });

    unsafe {
        let instance = GetModuleHandleW(null_mut());
        let class_name = wide("OmniKvmInput");
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null_mut(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            return Err("RegisterClassW failed".into());
        }
        // A message-only window: invisible, exists only to receive WM_INPUT.
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            null_mut(),
        );
        if hwnd.is_null() {
            return Err("CreateWindowExW failed".into());
        }
        // Raw mouse input, even while another window has focus.
        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01, // generic desktop
            usUsage: 0x02,     // mouse
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        if RegisterRawInputDevices(&device, 1, std::mem::size_of::<RAWINPUTDEVICE>() as u32) == 0 {
            return Err("RegisterRawInputDevices failed".into());
        }
        let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), instance, 0);
        let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), instance, 0);
        if kb.is_null() || ms.is_null() {
            return Err("SetWindowsHookExW failed".into());
        }
        println!("[input] watching keyboard and mouse. Push the pointer past the right edge (or press Scroll Lock) to control the Mac.");

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWindowsHookEx(kb);
        UnhookWindowsHookEx(ms);
    }
    Ok(())
}
