//! Omni-KVM host daemon.
//!
//! On Windows (Phase 3): capture this PC's keyboard and mouse and, while
//! the pointer is "on the Mac", forward them to the local board, which
//! relays them over the radio to the board plugged into the Mac.
//!
//! On macOS (Phase 4, step 1): connect to the local board and report the
//! link. Input capture on the Mac comes next.

mod board;
mod input;
#[cfg(windows)]
mod input_windows;
#[cfg(windows)]
mod keymap;
mod protocol;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use board::Board;
use input::Event;
use protocol::msg;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const STATUS_INTERVAL: Duration = Duration::from_secs(10);

fn main() {
    println!("Omni-KVM daemon {}", env!("CARGO_PKG_VERSION"));
    // Optional argument: the board's serial port, needed only when several
    // boards are plugged into this computer (e.g. during development).
    let wanted_port = std::env::args().nth(1);

    let (tx, rx) = mpsc::channel();
    let link_up = Arc::new(AtomicBool::new(false));
    {
        let link_up = link_up.clone();
        thread::spawn(move || board_loop(rx, link_up, wanted_port));
    }
    capture_input(tx, link_up);
}

#[cfg(windows)]
fn capture_input(tx: Sender<Event>, link_up: Arc<AtomicBool>) {
    // The hooks must live on a thread that pumps messages: this one.
    if let Err(e) = input_windows::run(tx, link_up) {
        eprintln!("Input capture failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn capture_input(tx: Sender<Event>, _link_up: Arc<AtomicBool>) {
    // No input capture on this OS yet: only the board thread works.
    // Holding `tx` keeps it running (it stops once no sender is left).
    let _tx = tx;
    loop {
        thread::park();
    }
}

/// Keeps a connection to the board, forwarding input events and tracking the link.
fn board_loop(rx: Receiver<Event>, link_up: Arc<AtomicBool>, wanted_port: Option<String>) {
    loop {
        link_up.store(false, Ordering::Relaxed);
        if let Some(port) = choose_board(wanted_port.as_deref()) {
            match Board::open(&port) {
                Ok(mut board) => {
                    println!("Connected to the board on {port}");
                    if serve(&mut board, &rx, &link_up).is_err() {
                        return; // the input side is gone: we are exiting
                    }
                    println!("Lost the board on {port}; waiting for it to come back...");
                }
                Err(e) => println!("Could not open {port}: {e}"),
            }
        }
        link_up.store(false, Ordering::Relaxed);
        // Drop input that piled up while there was no board.
        while rx.try_recv().is_ok() {}
        sleep(POLL_INTERVAL);
    }
}

/// Forwards events until the board stops answering (Ok) or the input
/// thread is gone (Err).
fn serve(board: &mut Board, rx: &Receiver<Event>, link_up: &AtomicBool) -> Result<(), ()> {
    let mut last_poll = Instant::now() - POLL_INTERVAL;
    let mut last_status = Instant::now();
    let mut last_link: Option<bool> = None;
    loop {
        if last_poll.elapsed() >= POLL_INTERVAL {
            last_poll = Instant::now();
            match board.request_stats(Duration::from_secs(1)) {
                Ok(s) => {
                    link_up.store(s.link_up, Ordering::Relaxed);
                    if last_link != Some(s.link_up) || last_status.elapsed() >= STATUS_INTERVAL {
                        last_link = Some(s.link_up);
                        last_status = Instant::now();
                        println!(
                            "[link] {} @ {} | session {} | sent {} (resent {}, gave up {}) | frames failed {}/{}",
                            if s.link_up { "UP" } else { "DOWN" },
                            protocol::phy_rate_name(s.phy_rate),
                            s.session,
                            s.radio_sent,
                            s.radio_retransmits,
                            s.radio_gave_up,
                            s.frames_failed,
                            s.frames_acked + s.frames_failed,
                        );
                    }
                }
                Err(e) => {
                    println!("Board error: {e}");
                    return Ok(());
                }
            }
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                if let Err(e) = send_event(board, event) {
                    println!("Board error: {e}");
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(()),
        }
    }
}

/// Encodes one input event as a protocol packet (little-endian fields).
fn send_event(board: &mut Board, event: Event) -> std::io::Result<()> {
    match event {
        Event::Key { usage, down, modifiers } => {
            board.send(if down { msg::KEY_DOWN } else { msg::KEY_UP }, &[usage, modifiers])
        }
        Event::Mouse { dx, dy, buttons } => {
            let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let mut p = Vec::with_capacity(5);
            p.extend_from_slice(&clamp(dx).to_le_bytes());
            p.extend_from_slice(&clamp(dy).to_le_bytes());
            p.push(buttons);
            board.send(msg::MOUSE_MOVE, &p)
        }
        Event::Scroll { vertical, horizontal } => {
            let mut p = Vec::with_capacity(4);
            p.extend_from_slice(&vertical.to_le_bytes());
            p.extend_from_slice(&horizontal.to_le_bytes());
            board.send(msg::MOUSE_SCROLL, &p)
        }
        Event::ModifierSync(modifiers) => board.send(msg::MODIFIER_SYNC, &[modifiers]),
    }
}

/// Picks the board to talk to, or explains why there is none.
fn choose_board(wanted: Option<&str>) -> Option<String> {
    let found = board::find();
    if let Some(port) = wanted {
        return found.iter().any(|b| b.port == port).then(|| port.to_string()).or_else(|| {
            println!("No Omni-KVM board on {port}.");
            None
        });
    }
    match found.as_slice() {
        [] => {
            println!("No Omni-KVM board found. Plug the board's \"USB\" port into this computer.");
            None
        }
        [only] => Some(only.port.clone()),
        several => {
            let list: Vec<String> = several
                .iter()
                .map(|b| format!("{} (serial {})", b.port, b.serial.as_deref().unwrap_or("?")))
                .collect();
            println!("Several boards found: {}. Pass the port to use, e.g. `omni-kvm {}`.", list.join(", "), several[0].port);
            None
        }
    }
}
