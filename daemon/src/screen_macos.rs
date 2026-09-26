//! macOS pointer handling: where the pointer enters after an edge switch,
//! and keeping it still and hidden while the Mac is unfocused.
//!
//! Uses a few CoreGraphics functions directly. Coordinates are in points,
//! with the origin at the top-left corner of the main display and y
//! growing downwards; every display shares this space.
//!
//! Neither reading nor moving the pointer needs a macOS permission.

use std::ffi::{c_char, c_void};
use std::ptr::null;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Once, OnceLock};
use std::thread;

use crate::focus::Pointer;
use crate::protocol;

/// An entering pointer appears this far inside the edge, so it is not
/// pushing against the edge the moment it arrives.
const ENTRY_INSET: f64 = 20.0;
/// While hidden, the pointer waits at least this far from every edge of
/// the desktop, where it could reveal a hidden Dock or set off a hot
/// corner. It still moves a little with every movement before it is put
/// back.
const PARK_MARGIN: f64 = 50.0;
/// kCGEventSourceStateCombinedSessionState: the state of this login session.
const COMBINED_SESSION_STATE: i32 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    fn CGWarpMouseCursorPosition(point: CGPoint) -> i32;
    fn CGEventSourceCreate(state: i32) -> *mut c_void;
    fn CGEventSourceSetLocalEventsSuppressionInterval(source: *mut c_void, seconds: f64);
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayHideCursor(display: u32) -> i32;
    fn CGDisplayShowCursor(display: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
    fn CFStringCreateWithCString(allocator: *const c_void, string: *const c_char, encoding: u32) -> *const c_void;
    static kCFBooleanTrue: *const c_void;
}

// libSystem, which every macOS program links.
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// The rectangle enclosing every display: (left, top, right, bottom),
/// with right and bottom being the last point inside.
pub fn desktop() -> Option<(f64, f64, f64, f64)> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } != 0 || count == 0 {
        return None;
    }
    let (mut left, mut top, mut right, mut bottom) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &id in &ids[..count as usize] {
        let r = unsafe { CGDisplayBounds(id) };
        left = left.min(r.origin.x);
        top = top.min(r.origin.y);
        right = right.max(r.origin.x + r.size.width - 1.0);
        bottom = bottom.max(r.origin.y + r.size.height - 1.0);
    }
    Some((left, top, right, bottom))
}

fn pointer_location() -> Option<(f64, f64)> {
    unsafe {
        let event = CGEventCreate(null());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some((p.x, p.y))
    }
}

/// Where a Quartz event happened: for mouse events, the pointer position.
pub fn event_location(event: *const c_void) -> (f64, f64) {
    let p = unsafe { CGEventGetLocation(event) };
    (p.x, p.y)
}

/// Moves the pointer, without the pause macOS normally adds after a warp.
fn warp((x, y): (f64, f64)) {
    unsafe {
        // After a warp, macOS ignores the mouse for a moment: measured 250–
        // 258 ms of a frozen pointer right after entering the Mac, which
        // felt like extra resistance on the way in. Setting the suppression
        // interval to 0 on an event source for this session lifts it
        // (measured: the pointer moves within 6 ms). The older global
        // CGSetLocalEventsSuppressionInterval also works but is deprecated;
        // CGAssociateMouseAndMouseCursorPosition had no effect.
        let source = CGEventSourceCreate(COMBINED_SESSION_STATE);
        if !source.is_null() {
            CGEventSourceSetLocalEventsSuppressionInterval(source, 0.0);
            CFRelease(source);
        }
        CGWarpMouseCursorPosition(CGPoint { x, y });
    }
}

// While the Mac is unfocused, its pointer is frozen: hidden, and put back
// after every movement, since the Mac's own mouse keeps moving it
// (swallowing the events does not stop that, and
// CGAssociateMouseAndMouseCursorPosition, the usual way to freeze it, only
// works for the frontmost app). It waits away from the screen edges and is
// shown again where it stopped (B4).
//
// The work happens on a worker thread: moving or hiding the pointer means
// asking the window server, which may be waiting for the event tap's
// callback to return. Done inside the callback, that can stall until
// macOS gives up on the tap and passes input straight to the Mac (D3).

static FROZEN: AtomicBool = AtomicBool::new(false);

enum Job {
    Freeze,
    PutBack,
    Resume,
    Enter(u16),
}

fn later(job: Job) {
    static JOBS: OnceLock<Sender<Job>> = OnceLock::new();
    let jobs = JOBS.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut stopped_at = (0.0, 0.0);
            let mut wait_at = (0.0, 0.0);
            let mut hidden = false;
            for job in rx {
                match job {
                    Job::Freeze => {
                        stopped_at = pointer_location().unwrap_or(stopped_at);
                        wait_at = away_from_edges(stopped_at);
                        FROZEN.store(true, Ordering::Relaxed);
                        warp(wait_at);
                        show_pointer(false, &mut hidden);
                    }
                    // Skip put-backs queued before the pointer was let go.
                    Job::PutBack if FROZEN.load(Ordering::Relaxed) => warp(wait_at),
                    Job::PutBack => {}
                    Job::Resume => {
                        if FROZEN.swap(false, Ordering::Relaxed) {
                            warp(stopped_at);
                        }
                        show_pointer(true, &mut hidden);
                    }
                    Job::Enter(height) => {
                        FROZEN.store(false, Ordering::Relaxed);
                        if let Some((left, top, _, bottom)) = desktop() {
                            // Just inside the Mac's facing (left) edge, at the
                            // same fraction of the height as where it left the PC.
                            warp((left + ENTRY_INSET, protocol::from_fraction(height, top, bottom)));
                        }
                        show_pointer(true, &mut hidden);
                    }
                }
            }
        });
        tx
    });
    let _ = jobs.send(job);
}

fn away_from_edges((x, y): (f64, f64)) -> (f64, f64) {
    let Some((left, top, right, bottom)) = desktop() else { return (x, y) };
    let inside = |v: f64, low: f64, high: f64| {
        if high - low < 2.0 * PARK_MARGIN { (low + high) / 2.0 } else { v.clamp(low + PARK_MARGIN, high - PARK_MARGIN) }
    };
    (inside(x, left, right), inside(y, top, bottom))
}

/// Carries out a pointer action of the focus state machine.
pub fn pointer(action: Pointer) {
    match action {
        Pointer::Freeze => later(Job::Freeze),
        Pointer::Resume => later(Job::Resume),
        Pointer::Enter(entry) => later(Job::Enter(entry.height)),
    }
}

/// The pointer moved while it should stay still: put it back.
pub fn put_back() {
    if FROZEN.load(Ordering::Relaxed) {
        later(Job::PutBack);
    }
}

/// Hides or shows the pointer, keeping the calls balanced: macOS counts them.
fn show_pointer(visible: bool, hidden: &mut bool) {
    static ALLOW_IN_BACKGROUND: Once = Once::new();
    if visible != *hidden {
        return; // already so
    }
    ALLOW_IN_BACKGROUND.call_once(allow_hiding_in_background);
    unsafe {
        if visible {
            CGDisplayShowCursor(CGMainDisplayID());
        } else {
            CGDisplayHideCursor(CGMainDisplayID());
        }
    }
    *hidden = !visible;
}

/// macOS lets only the frontmost app hide the pointer. The window server
/// setting "SetsCursorInBackground" lifts that for this program; it is
/// undocumented (Synergy and Deskflow rely on it), so it is looked up at
/// run time: if a macOS release drops it, the pointer just stays visible.
fn allow_hiding_in_background() {
    type DefaultConnection = unsafe extern "C" fn() -> i32;
    type SetConnectionProperty = unsafe extern "C" fn(i32, i32, *const c_void, *const c_void) -> i32;
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
    unsafe {
        let default_connection = dlsym(RTLD_DEFAULT, c"_CGSDefaultConnection".as_ptr());
        let set_property = dlsym(RTLD_DEFAULT, c"CGSSetConnectionProperty".as_ptr());
        if default_connection.is_null() || set_property.is_null() {
            say!("[input] this macOS cannot hide the pointer from the background; it stays visible");
            return;
        }
        let default_connection: DefaultConnection = std::mem::transmute(default_connection);
        let set_property: SetConnectionProperty = std::mem::transmute(set_property);
        let key = CFStringCreateWithCString(null(), c"SetsCursorInBackground".as_ptr(), UTF8);
        if key.is_null() {
            return;
        }
        let connection = default_connection();
        set_property(connection, connection, key, kCFBooleanTrue);
        CFRelease(key);
    }
}
