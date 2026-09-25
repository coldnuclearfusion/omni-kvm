//! macOS pointer handling for handoffs: puts the pointer where the other
//! computer hands it over, and watches whether it touches a screen edge.
//!
//! Uses a few CoreGraphics functions directly. Coordinates are in points,
//! with the origin at the top-left corner of the main display and y
//! growing downwards; every display shares this space.
//!
//! Neither reading nor moving the pointer needs a macOS permission.

use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::input::Event;
use crate::peer::{self, PeerMsg};

/// How often the pointer position is checked.
const WATCH_INTERVAL: Duration = Duration::from_millis(10);
/// Edge contact is re-sent at least this often, which also tells the
/// other daemon this one is running.
const KEEPALIVE: Duration = Duration::from_secs(1);
/// A handed-over pointer appears this far inside the edge, so it is not
/// pushing against the edge the moment it arrives.
const ENTRY_INSET: f64 = 20.0;

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
    fn CGAssociateMouseAndMouseCursorPosition(connected: i32) -> i32;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// The rectangle enclosing every display: (left, top, right, bottom),
/// with right and bottom being the last point inside.
fn desktop() -> Option<(f64, f64, f64, f64)> {
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

fn pointer() -> Option<CGPoint> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some(p)
    }
}

/// Which edges the pointer touches, and where it is along the left/right
/// edges. (With several displays side by side, only the outer edges of the
/// whole arrangement count.)
fn contact() -> Option<(u8, u16)> {
    let (left, top, right, bottom) = desktop()?;
    let p = pointer()?;
    let mut edges = 0;
    if p.x <= left {
        edges |= peer::CONTACT_LEFT;
    }
    if p.x >= right {
        edges |= peer::CONTACT_RIGHT;
    }
    if p.y <= top {
        edges |= peer::CONTACT_TOP;
    }
    if p.y >= bottom {
        edges |= peer::CONTACT_BOTTOM;
    }
    Some((edges, peer::fraction(p.y, top, bottom)))
}

/// Puts the pointer where a handoff says it enters this screen: it left
/// the other screen by `edge`, so it comes in by the opposite one.
pub fn enter(edge: u8, position: u16) -> bool {
    let Some((left, top, right, bottom)) = desktop() else { return false };
    let x = match edge {
        peer::EDGE_RIGHT => left + ENTRY_INSET,
        peer::EDGE_LEFT => right - ENTRY_INSET,
        _ => return false,
    };
    let y = peer::from_fraction(position, top, bottom);
    unsafe {
        if CGWarpMouseCursorPosition(CGPoint { x, y }) != 0 {
            return false;
        }
        // After a warp, macOS ignores mouse movement for a moment (~0.25 s)
        // unless told otherwise; the pointer should keep moving at once.
        CGAssociateMouseAndMouseCursorPosition(1);
    }
    true
}

/// Reports edge contact to the other daemon whenever it changes (and every
/// second), until the board thread is gone. Runs on the calling thread.
pub fn watch_edges(tx: Sender<Event>) {
    let mut last: Option<(u8, u16)> = None;
    let mut last_sent = Instant::now();
    loop {
        if let Some((edges, position)) = contact() {
            // Along an edge the position matters; away from all edges it doesn't.
            let changed = last.is_none_or(|(e, p)| e != edges || (edges != 0 && p != position));
            if changed || last_sent.elapsed() >= KEEPALIVE {
                if tx.send(Event::Peer(PeerMsg::EdgeContact { edges, position })).is_err() {
                    return;
                }
                last = Some((edges, position));
                last_sent = Instant::now();
            }
        }
        sleep(WATCH_INTERVAL);
    }
}
