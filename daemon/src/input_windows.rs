//! Windows input capture: low-level hooks plus Raw Input.
//!
//! Every keyboard and mouse event is routed by the focus state machine
//! (through hub.rs): passed to Windows, swallowed and forwarded to the
//! board, or swallowed and dropped. That includes what the board types in
//! for the Mac: Windows reports it like the PC's own devices, and the
//! design does not need the two told apart. The hooks decide what Windows
//! gets to see and forward keys, buttons and the wheel; forwarded movement
//! comes from Raw Input, which reports what the mouse counted before
//! Windows' pointer acceleration (the Mac applies its own).
//!
//! While input passes to Windows, pushing the pointer past the screen's
//! right edge by EDGE_RESISTANCE moves the focus to the Mac; so does the
//! hotkey (Win+Esc or Scroll Lock), which also brings it back.
//!
//! This module never records what is typed: it only tracks which keys are
//! held, to route their repeats and releases.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ptr::null_mut;
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_SCROLL,
};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RID_INPUT, RIDEV_INPUTSINK, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::focus::{Entry, Event, Route};
use crate::hotkey;
use crate::hub::{Guard, Hub};
use crate::input;
use crate::keymap;
use crate::protocol::{self, msg};
use crate::screen_windows::{cursor_pos, virtual_screen};

/// Raw mouse counts pushed against the right edge before switching.
const EDGE_RESISTANCE: i32 = 50;
/// Setting H6, "pass the hotkey on" (default off): the key completing the
/// hotkey is also delivered where input was going. To become a setting.
const PASS_HOTKEY_ON: bool = false;
/// A key-down this long after the key's previous event is a new press
/// whose release the hook never saw (it went to the lock screen, say), not
/// an auto-repeat: Windows repeats a held key within 1 s at the slowest.
const REPEAT_GAP_MS: u32 = 1500;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
/// An unassigned virtual-key code: tapping it counts as "another key" for
/// Windows without doing anything (see `tap_mask_key`).
const VK_MASK: u16 = 0xE8;

struct State {
    hub: Arc<Hub>,
    hotkey: hotkey::Watch,
    /// Keys down as the hook saw them, with the time of their last event.
    held: HashMap<u32, u32>,
    /// Keys that completed the hotkey: kept from everyone, their repeats
    /// and release too (H4).
    kept: HashSet<u32>,
    /// Modifiers and buttons forwarded and still held: the byte each
    /// forwarded key or mouse event carries.
    fwd_modifiers: u8,
    fwd_buttons: u8,
    /// Windows keys that Windows saw go down and not yet up (bit 0 left,
    /// bit 1 right), and whether the mask key was tapped since.
    windows_keys: u8,
    masked: bool,
    /// The hooks' latest decision about pointer movement (see on_raw_motion).
    motion_route: Route,
    edge_push: i32,
    wheel: i32,
    hwheel: i32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn windows_key_bit(vk: u32) -> u8 {
    match vk {
        VK_LWIN => 1,
        VK_RWIN => 2,
        _ => 0,
    }
}

/// Payload of MSG_MOUSE_MOVE.
fn mouse_payload(dx: i32, dy: i32, buttons: u8) -> [u8; 5] {
    let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let [x0, x1] = clamp(dx).to_le_bytes();
    let [y0, y1] = clamp(dy).to_le_bytes();
    [x0, x1, y0, y1, buttons]
}

impl State {
    /// Keyboard hook. Returns (Windows gets the event, tap the mask key).
    fn on_key(&mut self, vk: u32, scan: u32, extended: bool, down: bool, time: u32) -> (bool, bool) {
        let usage = keymap::hid_usage(vk, scan, extended);
        let id = input::key_id(usage, vk);
        let hub = self.hub.clone();
        let mut g = hub.lock();
        let mut mask = false;
        if !down {
            self.held.remove(&id);
            return (self.release(&mut g, id, usage, vk), false);
        }
        match self.held.insert(id, time) {
            Some(last) if time.wrapping_sub(last) < REPEAT_GAP_MS => {
                // Auto-repeat: where the press went. The other computer
                // repeats held keys on its own.
                let pass = !self.kept.contains(&id) && g.route_held(id) == Route::Pass;
                return (pass, false);
            }
            // Its release never reached the hook: settle that first.
            Some(_) => {
                self.release(&mut g, id, usage, vk);
            }
            None => {}
        }
        let pass = self.press(&mut g, id, usage, vk, &mut mask);
        (pass, mask)
    }

    /// A new key press. Returns whether Windows gets it.
    fn press(&mut self, g: &mut Guard, id: u32, usage: Option<u8>, vk: u32, mask: &mut bool) -> bool {
        let gui_escape = self.hotkey.press(usage);
        let completes = gui_escape || vk == VK_SCROLL as u32;
        if completes && !PASS_HOTKEY_ON {
            self.kept.insert(id);
            *mask |= self.withheld();
            g.handle(Event::Hotkey);
            return false;
        }
        let route = g.route_press(id);
        match route {
            Route::Pass => {
                let bit = windows_key_bit(vk);
                if bit != 0 {
                    if self.windows_keys == 0 {
                        self.masked = false;
                    }
                    self.windows_keys |= bit;
                }
            }
            Route::Forward => {
                self.forward_key(g, usage, true);
                *mask |= self.withheld();
            }
            Route::Drop => *mask |= self.withheld(),
        }
        if completes {
            g.handle(Event::Hotkey);
        }
        route == Route::Pass
    }

    /// A key release. Returns whether Windows gets it.
    fn release(&mut self, g: &mut Guard, id: u32, usage: Option<u8>, vk: u32) -> bool {
        self.hotkey.release(usage);
        if self.kept.remove(&id) {
            return false;
        }
        let route = g.route_release(id);
        match route {
            Route::Pass => self.windows_keys &= !windows_key_bit(vk),
            Route::Forward => self.forward_key(g, usage, false),
            Route::Drop => {}
        }
        route == Route::Pass
    }

    /// A key press Windows does not get. Windows opens the Start menu when
    /// a Windows key goes up with no other key since it went down, so if
    /// it holds one, it must see some other key first: returns true to tap
    /// the mask key (once per Windows-key press).
    fn withheld(&mut self) -> bool {
        let tap = self.windows_keys != 0 && !self.masked;
        self.masked |= tap;
        tap
    }

    fn forward_key(&mut self, g: &Guard, usage: Option<u8>, down: bool) {
        let Some(usage) = usage else { return }; // no HID usage: swallowed only
        if let Some(bit) = input::modifier_bit(usage) {
            if down {
                self.fwd_modifiers |= bit;
            } else {
                self.fwd_modifiers &= !bit;
            }
        }
        g.forward(if down { msg::KEY_DOWN } else { msg::KEY_UP }, [usage, self.fwd_modifiers, 0, 0, 0]);
    }

    /// Low-level mouse hook. Returns whether Windows gets the event.
    fn on_mouse_hook(&mut self, message: u32, mouse_data: u32) -> bool {
        let hub = self.hub.clone();
        let mut g = hub.lock();
        let button = match message {
            WM_LBUTTONDOWN | WM_LBUTTONUP => 0x01,
            WM_RBUTTONDOWN | WM_RBUTTONUP => 0x02,
            WM_MBUTTONDOWN | WM_MBUTTONUP => 0x04,
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                if (mouse_data >> 16) == 1 { 0x08 } else { 0x10 }
            }
            _ => 0,
        };
        if button != 0 {
            let down = matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN);
            let id = input::button_id(button);
            let route = if down { g.route_press(id) } else { g.route_release(id) };
            if route == Route::Forward {
                if down {
                    self.fwd_buttons |= button;
                } else {
                    self.fwd_buttons &= !button;
                }
                g.forward(msg::MOUSE_MOVE, mouse_payload(0, 0, self.fwd_buttons));
            }
            return route == Route::Pass;
        }
        let route = g.route_motion();
        match message {
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                if route == Route::Forward {
                    self.forward_wheel(&g, message == WM_MOUSEHWHEEL, (mouse_data >> 16) as u16 as i16);
                }
            }
            _ => self.motion_route = route,
        }
        route == Route::Pass
    }

    /// WHEEL_DELTA (120) per notch; high-resolution wheels send less.
    fn forward_wheel(&mut self, g: &Guard, horizontal: bool, delta: i16) {
        let rest = if horizontal { &mut self.hwheel } else { &mut self.wheel };
        *rest += delta as i32;
        let notches = (*rest / WHEEL_DELTA as i32) as i16;
        *rest -= notches as i32 * WHEEL_DELTA as i32;
        if notches != 0 {
            // Both positive = up / right, as in MSG_MOUSE_SCROLL.
            let (v, h) = if horizontal { (0, notches) } else { (notches, 0) };
            let [v0, v1] = v.to_le_bytes();
            let [h0, h1] = h.to_le_bytes();
            g.forward(msg::MOUSE_SCROLL, [v0, v1, h0, h1, 0]);
        }
    }

    /// Raw Input: what the mouse counted, before acceleration.
    fn on_raw_motion(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let hub = self.hub.clone();
        let mut g = hub.lock();
        match g.route_motion() {
            // Only while the hooks swallow movement too: Raw Input can lag
            // behind the hooks, and movement Windows was given must not be
            // sent to the Mac as well.
            Route::Forward if self.motion_route == Route::Forward => {
                g.forward(msg::MOUSE_MOVE, mouse_payload(dx, dy, self.fwd_buttons));
            }
            Route::Pass => return self.watch_edge(&mut g, dx),
            _ => {}
        }
        self.edge_push = 0;
    }

    /// Pushing past the right edge, by any device (the Mac's too, typed in
    /// by the board), moves the focus to the Mac.
    fn watch_edge(&mut self, g: &mut Guard, dx: i32) {
        let (_, top, right, bottom) = virtual_screen();
        let p = cursor_pos();
        if p.x >= right && dx > 0 {
            self.edge_push += dx;
            if self.edge_push >= EDGE_RESISTANCE {
                self.edge_push = 0;
                let height = protocol::fraction(p.y as f64, top as f64, bottom as f64);
                g.handle(Event::EdgePushed(Entry { height }));
            }
        } else if dx < 0 || p.x < right {
            self.edge_push = 0;
        }
    }
}

/// Taps the unassigned key for Windows alone (our hooks let injected input
/// through), so that the release of a Windows key it holds does not look
/// like a lone tap and open the Start menu. AutoHotkey uses the same key
/// for the same purpose.
fn tap_mask_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // Leave injected input alone: other software's, and our mask key.
        if kb.flags & LLKHF_INJECTED == 0 {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let extended = kb.flags & LLKHF_EXTENDED != 0;
            let result = STATE.with(|s| {
                s.borrow_mut().as_mut().map(|st| st.on_key(kb.vkCode, kb.scanCode, extended, down, kb.time))
            });
            if let Some((pass, mask)) = result {
                if mask {
                    tap_mask_key();
                }
                if !pass {
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
            let pass = STATE.with(|s| {
                s.borrow_mut().as_mut().is_none_or(|st| st.on_mouse_hook(wparam as u32, ms.mouseData))
            });
            if !pass {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_INPUT {
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
        // Injected input has no device; absolute devices (pen tablets,
        // remote desktop) are not forwarded.
        if read != u32::MAX && raw.header.dwType == RIM_TYPEMOUSE && !raw.header.hDevice.is_null() {
            let m = unsafe { raw.data.mouse };
            if m.usFlags & MOUSE_MOVE_ABSOLUTE == 0 {
                STATE.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut() {
                        st.on_raw_motion(m.lLastX, m.lLastY);
                    }
                });
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Installs the hooks and runs the message loop on the calling thread,
/// which must be the thread that keeps pumping messages. Never returns
/// normally; the hooks disappear with the process.
pub fn run(hub: Arc<Hub>) -> Result<(), String> {
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            hub,
            hotkey: hotkey::Watch::default(),
            held: HashMap::new(),
            kept: HashSet::new(),
            fwd_modifiers: 0,
            fwd_buttons: 0,
            windows_keys: 0,
            masked: false,
            motion_route: Route::Pass,
            edge_push: 0,
            wheel: 0,
            hwheel: 0,
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
        say!("[input] watching keyboard and mouse. Push the pointer past the right edge, or press Win+Esc or Scroll Lock, to switch.");

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        UnhookWindowsHookEx(kb);
        UnhookWindowsHookEx(ms);
    }
    Ok(())
}
