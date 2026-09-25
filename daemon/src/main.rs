//! Omni-KVM host daemon.
//!
//! On Windows (Phase 3): capture this PC's keyboard and mouse and, while
//! the pointer is "on the Mac", forward them to the local board, which
//! relays them over the radio to the board plugged into the Mac.
//!
//! On macOS (Phase 4, step 2): place the pointer where Windows hands it
//! over, and report when it touches a screen edge, so pushing it past the
//! left edge brings it back to Windows. Input capture on the Mac is next.

mod board;
mod focus;
mod input;
#[cfg(windows)]
mod input_windows;
#[cfg(windows)]
mod keymap;
mod peer;
mod protocol;
#[cfg(target_os = "macos")]
mod screen_macos;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use board::Board;
use input::Event;
use peer::{Peer, PeerMsg};
use protocol::{LinkStats, PACKET_SIZE, msg};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const STATUS_INTERVAL: Duration = Duration::from_secs(10);
/// No status reply for this long: the board is gone (unplugged, reset).
const BOARD_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the board thread waits for something to send before checking
/// for packets from the board again. Anything to send wakes it at once;
/// this only bounds how late a message from the other daemon is noticed.
const IDLE_WAIT: Duration = Duration::from_millis(5);

fn main() {
    println!("Omni-KVM daemon {}", env!("CARGO_PKG_VERSION"));
    // Optional argument: the board's serial port, needed only when several
    // boards are plugged into this computer (e.g. during development).
    let wanted_port = std::env::args().nth(1);

    let (tx, rx) = mpsc::channel();
    let peer = Arc::new(Peer::new());
    {
        let peer = peer.clone();
        thread::spawn(move || board_loop(rx, peer, wanted_port));
    }
    run_input_side(tx, peer);
}

#[cfg(windows)]
fn run_input_side(tx: Sender<Event>, peer: Arc<Peer>) {
    // The hooks must live on a thread that pumps messages: this one.
    if let Err(e) = input_windows::run(tx, peer) {
        eprintln!("Input capture failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn run_input_side(tx: Sender<Event>, _peer: Arc<Peer>) {
    screen_macos::watch_edges(tx);
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_input_side(tx: Sender<Event>, _peer: Arc<Peer>) {
    // Nothing to do on this OS: only the board thread works. Holding `tx`
    // keeps it running (it stops once no sender is left).
    let _tx = tx;
    loop {
        thread::park();
    }
}

/// Keeps a connection to the board, forwarding events and tracking the link.
fn board_loop(rx: Receiver<Event>, peer: Arc<Peer>, wanted_port: Option<String>) {
    loop {
        peer.set_link_up(false);
        if let Some(port) = choose_board(wanted_port.as_deref()) {
            match Board::open(&port) {
                Ok(mut board) => {
                    println!("Connected to the board on {port}");
                    if serve(&mut board, &rx, &peer).is_err() {
                        return; // the input side is gone: we are exiting
                    }
                    println!("Lost the board on {port}; waiting for it to come back...");
                }
                Err(e) => println!("Could not open {port}: {e}"),
            }
        }
        peer.set_link_up(false);
        // Drop input that piled up while there was no board.
        while rx.try_recv().is_ok() {}
        sleep(POLL_INTERVAL);
    }
}

/// Forwards events and handles what the board sends, until the board
/// stops answering (Ok) or the input side is gone (Err).
fn serve(board: &mut Board, rx: &Receiver<Event>, peer: &Peer) -> Result<(), ()> {
    let mut last_request = Instant::now() - POLL_INTERVAL;
    let mut last_reply = Instant::now();
    let mut last_status = Instant::now();
    let mut last_link: Option<bool> = None;
    let mut last_handoff: Option<u8> = None;
    let mut peer_daemon = false;
    loop {
        if peer_daemon != peer.daemon_alive() {
            peer_daemon = !peer_daemon;
            println!(
                "[peer] the other computer's daemon {}",
                if peer_daemon { "is running" } else { "is not answering" }
            );
        }
        if last_request.elapsed() >= POLL_INTERVAL {
            last_request = Instant::now();
            if let Err(e) = board.request_stats() {
                return board_lost(e);
            }
        }
        if last_reply.elapsed() >= BOARD_TIMEOUT {
            println!("The board stopped answering.");
            return Ok(());
        }

        let packets = match board.receive() {
            Ok(packets) => packets,
            Err(e) => return board_lost(e),
        };
        for packet in packets {
            if packet[2] == msg::DAEMON_STATUS {
                last_reply = Instant::now();
                let s = match LinkStats::parse(&packet) {
                    Ok(s) => s,
                    Err(e) => {
                        println!("Board error: {e}");
                        continue;
                    }
                };
                peer.set_link_up(s.link_up);
                if last_link != Some(s.link_up) || last_status.elapsed() >= STATUS_INTERVAL {
                    last_link = Some(s.link_up);
                    last_status = Instant::now();
                    print_link(&s);
                }
            } else if let Some(m) = PeerMsg::decode(&packet) {
                peer.heard();
                if let Err(e) = on_peer_message(board, peer, m, &mut last_handoff) {
                    return board_lost(e);
                }
            }
        }

        match rx.recv_timeout(IDLE_WAIT) {
            Ok(event) => {
                if let Err(e) = send_event(board, event) {
                    return board_lost(e);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(()),
        }
    }
}

fn board_lost(e: std::io::Error) -> Result<(), ()> {
    println!("Board error: {e}");
    Ok(())
}

fn print_link(s: &LinkStats) {
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

/// A message from the other computer's daemon.
fn on_peer_message(board: &mut Board, peer: &Peer, m: PeerMsg, last_handoff: &mut Option<u8>) -> std::io::Result<()> {
    match m {
        PeerMsg::EdgeContact { edges, position } => peer.set_contact(edges, position),
        PeerMsg::Handoff { edge, id, position } => {
            // A repeat (its acknowledgement was lost on the radio) is only
            // acknowledged again: the pointer may have moved since.
            let accepted = if *last_handoff == Some(id) {
                true
            } else {
                *last_handoff = Some(id);
                let accepted = enter_screen(edge, position);
                println!(
                    "[peer] pointer handed over at {:.0}% height: {}",
                    position as f64 / 655.35,
                    if accepted { "placed" } else { "could not place it" }
                );
                accepted
            };
            send_event(board, Event::Peer(PeerMsg::HandoffAck { id, accepted }))?;
        }
        PeerMsg::HandoffAck { accepted: false, .. } => {
            println!("[peer] the other computer could not place its pointer")
        }
        PeerMsg::HandoffAck { .. } => {}
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn enter_screen(edge: u8, position: u16) -> bool {
    screen_macos::enter(edge, position)
}

#[cfg(not(target_os = "macos"))]
fn enter_screen(_edge: u8, _position: u16) -> bool {
    false // not needed yet: only Windows hands the pointer over (Phase 4, step 3)
}

/// Encodes one event as a protocol packet (little-endian fields).
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
        Event::Peer(m) => {
            let (msg_type, payload) = m.encode();
            debug_assert!(payload.len() <= PACKET_SIZE - protocol::HEADER_SIZE);
            board.send(msg_type, &payload)
        }
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
