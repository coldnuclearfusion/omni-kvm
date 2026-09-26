//! macOS input capture with a Quartz event tap.
//!
//! Every keyboard, mouse and trackpad event is routed by the focus state
//! machine (through hub.rs): passed to the Mac, swallowed and forwarded to
//! the board, or swallowed and dropped. That includes what the board types
//! in for the PC: macOS reports it like the Mac's own devices, and the
//! design does not need the two told apart.
//!
//! While input passes to the Mac, pushing the pointer past the screen's
//! left edge by EDGE_RESISTANCE moves the focus to the PC; so does the
//! hotkey (Command+Esc), which also brings it back. While the Mac is
//! unfocused its pointer stays hidden where it was (screen_macos.rs).
//!
//! macOS keeps modifier keys per keyboard: a Shift held on one keyboard
//! (the board's, typing for the PC) does not apply to keys typed on
//! another (experiment A2). Input passed to the Mac therefore gets the
//! modifiers passed to it from every keyboard added to its flags.
//!
//! The event tap needs the Accessibility permission for the app that
//! starts the daemon (e.g. Terminal). macOS hides keystrokes from every
//! tap while a password field or other secure input is active.
//!
//! This module never records what is typed: it only tracks which keys are
//! held, to route their repeats and releases.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::focus::{Entry, Event, Route};
use crate::hotkey;
use crate::hub::{Guard, Hub};
use crate::input;
use crate::keymap_macos;
use crate::protocol::{self, msg};
use crate::screen_macos;

/// Pointer movement (points) pushed against the left edge before switching.
const EDGE_RESISTANCE: f64 = 50.0;
/// Setting H6, "pass the hotkey on" (default off): the key completing the
/// hotkey is also delivered where input was going. To become a setting.
const PASS_HOTKEY_ON: bool = false;
/// Trackpad scrolling arrives in points; the other computer gets wheel
/// notches.
const SCROLL_POINTS_PER_NOTCH: f64 = 24.0;
/// Send trackpad scrolling reversed. The tap sees it the way the Mac's
/// "natural scrolling" setting makes it, before tools like LinearMouse
/// flip the trackpad alone; reversing matches a Mac with natural scrolling
/// off and the trackpad flipped back to natural (the developer's setup).
/// With macOS defaults (natural scrolling on) this should be false. To
/// become a setting (B11).
const INVERT_TRACKPAD_SCROLL: bool = true;
/// A tap callback slower than this is logged: macOS switches off taps
/// that keep it waiting.
const SLOW_CALLBACK: Duration = Duration::from_millis(50);
/// How long the desktop's size is trusted before it is read again.
const DESKTOP_REFRESH: Duration = Duration::from_secs(1);

// Quartz event types (CGEventTypes.h).
const LEFT_MOUSE_DOWN: u32 = 1;
const LEFT_MOUSE_UP: u32 = 2;
const RIGHT_MOUSE_DOWN: u32 = 3;
const RIGHT_MOUSE_UP: u32 = 4;
const MOUSE_MOVED: u32 = 5;
const LEFT_MOUSE_DRAGGED: u32 = 6;
const RIGHT_MOUSE_DRAGGED: u32 = 7;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const SCROLL_WHEEL: u32 = 22;
const OTHER_MOUSE_DOWN: u32 = 25;
const OTHER_MOUSE_UP: u32 = 26;
const OTHER_MOUSE_DRAGGED: u32 = 27;
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

// Quartz event fields (CGEventTypes.h).
const MOUSE_BUTTON_NUMBER: u32 = 3;
const MOUSE_DELTA_X: u32 = 4;
const MOUSE_DELTA_Y: u32 = 5;
const KEYBOARD_AUTOREPEAT: u32 = 8;
const KEYBOARD_KEYCODE: u32 = 9;
const SCROLL_DELTA_AXIS_1: u32 = 11; // vertical, in lines (mouse wheels)
const SCROLL_DELTA_AXIS_2: u32 = 12; // horizontal
const SCROLL_IS_CONTINUOUS: u32 = 88; // trackpads, Magic Mouse
const SCROLL_POINT_DELTA_AXIS_1: u32 = 96;
const SCROLL_POINT_DELTA_AXIS_2: u32 = 97;

const HID_EVENT_TAP: u32 = 0; // kCGHIDEventTap: before any app sees the event
const HEAD_INSERT_EVENT_TAP: u32 = 0;
const TAP_OPTION_DEFAULT: u32 = 0; // may swallow events (not listen-only)

const HID_CAPS_LOCK: u8 = 0x39;

type TapCallback = extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: TapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventGetDoubleValueField(event: *mut c_void, field: u32) -> f64;
    fn CGEventGetFlags(event: *mut c_void) -> u64;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGRequestPostEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(allocator: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRun();
    static kCFRunLoopCommonModes: *const c_void;
}

fn int_field(event: *mut c_void, field: u32) -> i64 {
    unsafe { CGEventGetIntegerValueField(event, field) }
}

fn double_field(event: *mut c_void, field: u32) -> f64 {
    unsafe { CGEventGetDoubleValueField(event, field) }
}

/// Payload of MSG_MOUSE_MOVE.
fn mouse_payload(dx: i32, dy: i32, buttons: u8) -> [u8; 5] {
    let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    let [x0, x1] = clamp(dx).to_le_bytes();
    let [y0, y1] = clamp(dy).to_le_bytes();
    [x0, x1, y0, y1, buttons]
}

struct State {
    hub: Arc<Hub>,
    tap: *mut c_void,
    hotkey: hotkey::Watch,
    /// Keys that completed the hotkey: kept from everyone, their repeats
    /// and release too (H4).
    kept: HashSet<u32>,
    /// Modifiers and buttons forwarded and still held: the byte each
    /// forwarded key or mouse event carries.
    fwd_modifiers: u8,
    fwd_buttons: u8,
    /// Modifiers passed to the Mac and not released since, from any
    /// keyboard (HID modifier byte): added to what passes to the Mac.
    passed_modifiers: u8,
    motion_rest: (f64, f64), // sub-point movement not sent yet
    scroll_rest: (f64, f64),
    edge_push: f64,
    desktop: Option<(f64, f64, f64, f64)>,
    desktop_read: Option<Instant>,
    slow_reported: Option<Instant>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

impl State {
    /// Returns true to swallow the event.
    fn on_event(&mut self, event_type: u32, event: *mut c_void) -> bool {
        match event_type {
            KEY_DOWN | KEY_UP => self.on_key(event, event_type == KEY_DOWN),
            FLAGS_CHANGED => self.on_flags(event),
            MOUSE_MOVED | LEFT_MOUSE_DRAGGED | RIGHT_MOUSE_DRAGGED | OTHER_MOUSE_DRAGGED => {
                self.on_move(event, event_type != MOUSE_MOVED)
            }
            LEFT_MOUSE_DOWN | LEFT_MOUSE_UP | RIGHT_MOUSE_DOWN | RIGHT_MOUSE_UP | OTHER_MOUSE_DOWN
            | OTHER_MOUSE_UP => self.on_button(event_type, event),
            SCROLL_WHEEL => self.on_scroll(event),
            _ => false,
        }
    }

    /// Passes `event` to the Mac with the modifiers held on every keyboard
    /// (returns false), or swallows it (true).
    fn deliver(&self, route: Route, event: *mut c_void) -> bool {
        if route != Route::Pass {
            return true;
        }
        if self.passed_modifiers != 0 {
            unsafe {
                let flags = CGEventGetFlags(event);
                let merged = flags | keymap_macos::modifier_flags(self.passed_modifiers);
                if merged != flags {
                    CGEventSetFlags(event, merged);
                }
            }
        }
        false
    }

    fn on_key(&mut self, event: *mut c_void, down: bool) -> bool {
        let keycode = int_field(event, KEYBOARD_KEYCODE) as u16;
        let usage = keymap_macos::hid_usage(keycode);
        let id = input::key_id(usage, keycode as u32);
        let hub = self.hub.clone();
        let mut g = hub.lock();
        if self.kept.contains(&id) {
            if !down {
                self.kept.remove(&id);
            }
            return true;
        }
        if down && int_field(event, KEYBOARD_AUTOREPEAT) != 0 {
            // Where the press went. The other computer repeats held keys
            // on its own.
            let route = g.route_held(id);
            return if route == Route::Pass { self.deliver(route, event) } else { true };
        }
        let completes = down && self.hotkey.press(usage);
        if !down {
            self.hotkey.release(usage);
        }
        if completes && !PASS_HOTKEY_ON {
            self.kept.insert(id);
            g.handle(Event::Hotkey);
            return true;
        }
        let route = if down { g.route_press(id) } else { g.route_release(id) };
        if route == Route::Forward {
            self.forward_key(&g, usage, down);
        }
        if completes {
            g.handle(Event::Hotkey);
        }
        self.deliver(route, event)
    }

    /// Modifier keys, Caps Lock and fn arrive as "flags changed" events.
    fn on_flags(&mut self, event: *mut c_void) -> bool {
        let keycode = int_field(event, KEYBOARD_KEYCODE) as u16;
        let usage = keymap_macos::hid_usage(keycode);
        let hub = self.hub.clone();
        let mut g = hub.lock();
        let Some((usage, bit)) = usage.and_then(|u| Some((u, input::modifier_bit(u)?))) else {
            // Caps Lock (each press toggles it; the other computer toggles
            // its own) and fn: nothing held to remember.
            let route = g.route_motion();
            if route == Route::Forward && usage == Some(HID_CAPS_LOCK) {
                g.forward(msg::KEY_DOWN, [HID_CAPS_LOCK, self.fwd_modifiers, 0, 0, 0]);
                g.forward(msg::KEY_UP, [HID_CAPS_LOCK, self.fwd_modifiers, 0, 0, 0]);
            }
            return self.deliver(route, event);
        };
        let down = keymap_macos::modifier_down(unsafe { CGEventGetFlags(event) }, usage);
        let route = if down {
            self.hotkey.press(Some(usage));
            g.route_press(usage as u32)
        } else {
            self.hotkey.release(Some(usage));
            g.route_release(usage as u32)
        };
        match route {
            Route::Pass if down => self.passed_modifiers |= bit,
            Route::Pass => self.passed_modifiers &= !bit,
            Route::Forward => self.forward_key(&g, Some(usage), down),
            Route::Drop => {}
        }
        self.deliver(route, event)
    }

    fn forward_key(&mut self, g: &Guard, usage: Option<u8>, down: bool) {
        let Some(usage) = usage else { return }; // no HID usage (fn): swallowed only
        if let Some(bit) = input::modifier_bit(usage) {
            if down {
                self.fwd_modifiers |= bit;
            } else {
                self.fwd_modifiers &= !bit;
            }
        }
        g.forward(if down { msg::KEY_DOWN } else { msg::KEY_UP }, [usage, self.fwd_modifiers, 0, 0, 0]);
    }

    fn on_move(&mut self, event: *mut c_void, dragged: bool) -> bool {
        let dx = double_field(event, MOUSE_DELTA_X);
        let dy = double_field(event, MOUSE_DELTA_Y);
        let hub = self.hub.clone();
        let mut g = hub.lock();
        let route = g.route_motion();
        match route {
            Route::Pass => {
                self.motion_rest = (0.0, 0.0);
                self.watch_edge(&mut g, event, dx);
                return if dragged { self.deliver(route, event) } else { false };
            }
            Route::Forward => {
                // Quartz reports fractions of a point; keep the remainder
                // so slow movement still adds up.
                let (rest_x, rest_y) = (self.motion_rest.0 + dx, self.motion_rest.1 + dy);
                let (step_x, step_y) = (rest_x.trunc(), rest_y.trunc());
                self.motion_rest = (rest_x - step_x, rest_y - step_y);
                if step_x != 0.0 || step_y != 0.0 {
                    g.forward(msg::MOUSE_MOVE, mouse_payload(step_x as i32, step_y as i32, self.fwd_buttons));
                }
            }
            Route::Drop => {}
        }
        // macOS moved the pointer before the tap saw the movement.
        screen_macos::put_back();
        self.edge_push = 0.0;
        true
    }

    /// Pushing past the left edge, by any device (the PC's too, typed in
    /// by the board), moves the focus to the PC.
    fn watch_edge(&mut self, g: &mut Guard, event: *mut c_void, dx: f64) {
        let (x, y) = screen_macos::event_location(event);
        let Some((left, top, _, bottom)) = self.desktop() else { return };
        if dx < 0.0 && x <= left {
            self.edge_push -= dx;
            if self.edge_push >= EDGE_RESISTANCE {
                self.edge_push = 0.0;
                g.handle(Event::EdgePushed(Entry { height: protocol::fraction(y, top, bottom) }));
            }
        } else if dx != 0.0 {
            self.edge_push = 0.0; // moving right, or left away from the edge
        }
    }

    fn desktop(&mut self) -> Option<(f64, f64, f64, f64)> {
        if self.desktop_read.is_none_or(|t| t.elapsed() >= DESKTOP_REFRESH) {
            self.desktop = screen_macos::desktop();
            self.desktop_read = Some(Instant::now());
        }
        self.desktop
    }

    fn on_button(&mut self, event_type: u32, event: *mut c_void) -> bool {
        let button = match event_type {
            LEFT_MOUSE_DOWN | LEFT_MOUSE_UP => 0x01,
            RIGHT_MOUSE_DOWN | RIGHT_MOUSE_UP => 0x02,
            _ => match int_field(event, MOUSE_BUTTON_NUMBER) {
                2 => 0x04, // middle
                3 => 0x08, // back
                4 => 0x10, // forward
                _ => 0,
            },
        };
        let down = matches!(event_type, LEFT_MOUSE_DOWN | RIGHT_MOUSE_DOWN | OTHER_MOUSE_DOWN);
        let hub = self.hub.clone();
        let mut g = hub.lock();
        if button == 0 {
            let route = g.route_motion(); // a button the protocol has no bit for
            return self.deliver(route, event);
        }
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
        self.deliver(route, event)
    }

    fn on_scroll(&mut self, event: *mut c_void) -> bool {
        let hub = self.hub.clone();
        let g = hub.lock();
        let route = g.route_motion();
        if route != Route::Forward {
            self.scroll_rest = (0.0, 0.0);
            return self.deliver(route, event);
        }
        let (vertical, horizontal) = if int_field(event, SCROLL_IS_CONTINUOUS) != 0 {
            let scale = if INVERT_TRACKPAD_SCROLL { -1.0 } else { 1.0 } / SCROLL_POINTS_PER_NOTCH;
            let v = self.scroll_rest.0 + double_field(event, SCROLL_POINT_DELTA_AXIS_1) * scale;
            let h = self.scroll_rest.1 + double_field(event, SCROLL_POINT_DELTA_AXIS_2) * scale;
            self.scroll_rest = (v - v.trunc(), h - h.trunc());
            (v.trunc(), h.trunc())
        } else {
            (int_field(event, SCROLL_DELTA_AXIS_1) as f64, int_field(event, SCROLL_DELTA_AXIS_2) as f64)
        };
        // Quartz: positive = up / left. MSG_MOUSE_SCROLL: positive = up / right.
        if vertical != 0.0 || horizontal != 0.0 {
            let [v0, v1] = (vertical as i16).to_le_bytes();
            let [h0, h1] = (-horizontal as i16).to_le_bytes();
            g.forward(msg::MOUSE_SCROLL, [v0, v1, h0, h1, 0]);
        }
        true
    }
}

extern "C" fn tap_callback(_proxy: *mut c_void, event_type: u32, event: *mut c_void, _user: *mut c_void) -> *mut c_void {
    let swallow = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(st) = s.as_mut() else { return false };
        if event_type == TAP_DISABLED_BY_TIMEOUT || event_type == TAP_DISABLED_BY_USER_INPUT {
            // macOS switched the tap off (D4), and input went straight to
            // the Mac meanwhile, releases included: forget the modifiers
            // passed before rather than add one that is no longer held.
            unsafe { CGEventTapEnable(st.tap, true) };
            st.passed_modifiers = 0;
            let why = if event_type == TAP_DISABLED_BY_TIMEOUT { "it answered too slowly" } else { "user input" };
            say!("[input] macOS switched the event tap off ({why}); switched it back on");
            return false;
        }
        let start = Instant::now();
        let swallow = st.on_event(event_type, event);
        let took = start.elapsed();
        if took >= SLOW_CALLBACK && st.slow_reported.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            say!("[input] slow event tap callback: {} ms (event type {event_type})", took.as_millis());
            st.slow_reported = Some(Instant::now());
        }
        swallow
    });
    if swallow { null_mut() } else { event }
}

/// Installs the event tap and runs the run loop on the calling thread.
/// Never returns normally.
pub fn run(hub: Arc<Hub>) -> Result<(), String> {
    let mask = [
        LEFT_MOUSE_DOWN,
        LEFT_MOUSE_UP,
        RIGHT_MOUSE_DOWN,
        RIGHT_MOUSE_UP,
        MOUSE_MOVED,
        LEFT_MOUSE_DRAGGED,
        RIGHT_MOUSE_DRAGGED,
        KEY_DOWN,
        KEY_UP,
        FLAGS_CHANGED,
        SCROLL_WHEEL,
        OTHER_MOUSE_DOWN,
        OTHER_MOUSE_UP,
        OTHER_MOUSE_DRAGGED,
    ]
    .iter()
    .fold(0u64, |mask, t| mask | 1 << t);

    let mut asked = false;
    let tap = loop {
        let tap = unsafe {
            CGEventTapCreate(HID_EVENT_TAP, HEAD_INSERT_EVENT_TAP, TAP_OPTION_DEFAULT, mask, tap_callback, null_mut())
        };
        if !tap.is_null() {
            break tap;
        }
        if !asked {
            say!(
                "[input] macOS has not allowed this program to read the keyboard and trackpad yet.\n\
                 [input] Allow the app running it (e.g. Terminal) in System Settings > Privacy & Security >\n\
                 [input] Accessibility (and Input Monitoring, if listed). Waiting..."
            );
            unsafe {
                CGRequestPostEventAccess();
                CGRequestListenEventAccess();
            }
            asked = true;
        }
        sleep(Duration::from_secs(2));
    };

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            hub,
            tap,
            hotkey: hotkey::Watch::default(),
            kept: HashSet::new(),
            fwd_modifiers: 0,
            fwd_buttons: 0,
            passed_modifiers: 0,
            motion_rest: (0.0, 0.0),
            scroll_rest: (0.0, 0.0),
            edge_push: 0.0,
            desktop: None,
            desktop_read: None,
            slow_reported: None,
        })
    });

    unsafe {
        let source = CFMachPortCreateRunLoopSource(null(), tap, 0);
        if source.is_null() {
            return Err("CFMachPortCreateRunLoopSource failed".into());
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
    }
    say!("[input] watching keyboard and trackpad. Push the pointer past the left edge, or press Command+Esc, to switch.");
    unsafe { CFRunLoopRun() };
    Ok(())
}
