//! Omni-KVM host daemon.
//!
//! Captures this computer's keyboard and mouse and sends their input where
//! the focus is (docs/focus-design.md): to this computer, or, through the
//! local board and the radio, to the other one. The PC sits to the left of
//! the Mac: the pointer crosses at the PC's right edge and the Mac's left
//! edge.
//!
//! Two threads share the focus state machine through hub.rs: the input
//! side (hooks on Windows, an event tap on macOS) routes input as it
//! arrives, and the board thread (here) talks to the board and runs the
//! timers.

#[macro_use]
mod log;
mod board;
mod focus;
mod hotkey;
mod hub;
mod input;
#[cfg(target_os = "macos")]
mod input_macos;
#[cfg(windows)]
mod input_windows;
#[cfg(windows)]
mod keymap;
#[cfg(target_os = "macos")]
mod keymap_macos;
mod protocol;
#[cfg(target_os = "macos")]
mod screen_macos;
#[cfg(windows)]
mod screen_windows;

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use board::Board;
use focus::{Event, Holder, Mode, Node, Pointer, View};
use hub::{Hub, Out};
use protocol::{LinkStats, Status, msg};

/// This computer. The layout is fixed for now: Windows is the PC on the
/// left, macOS the Mac on the right.
const ME: Node = if cfg!(windows) { Node::Pc } else { Node::Mac };

/// A view goes to the other daemon at least this often (keepalive).
const KEEPALIVE: Duration = Duration::from_secs(1);
/// No view from the other daemon for this long: it has stopped (S8).
const PEER_SILENT: Duration = Duration::from_secs(3);
/// Once the board confirms its gate closed, input it typed in just before
/// may still be on its way through the operating system to our capture.
/// Forwarding starts only after this (R2).
const SETTLE: Duration = Duration::from_millis(20);
/// The board must confirm a gate close within this, or it counts as lost.
const GATE_TIMEOUT: Duration = Duration::from_millis(100);
/// Link statistics are asked for this often: for the log, and to notice a
/// board that stopped answering.
const STATS_INTERVAL: Duration = Duration::from_secs(2);
const STATS_LOG_INTERVAL: Duration = Duration::from_secs(10);
/// No status from the board for this long: it is gone (unplugged, reset,
/// stuck).
const BOARD_TIMEOUT: Duration = Duration::from_secs(5);
/// How often to look for a board while there is none.
const LOOK_INTERVAL: Duration = Duration::from_secs(2);
/// How long the board thread waits for work before it checks the board
/// and the timers again. Work in the queue wakes it at once; this bounds
/// how late a packet from the board is noticed.
const IDLE_WAIT: Duration = Duration::from_millis(2);
/// A switch that cannot happen is logged at most this often.
const IGNORED_LOG_INTERVAL: Duration = Duration::from_secs(5);

fn main() {
    println!("Omni-KVM daemon {} on the {}", env!("CARGO_PKG_VERSION"), node_name(ME));
    // A second daemon would fight the first over the board and add a
    // second set of hooks (or a second event tap on macOS).
    let _instance = match single_instance() {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    // Optional argument: the board's serial port, needed only when several
    // boards are plugged into this computer (e.g. during development).
    let wanted_port = std::env::args().nth(1);

    let (hub, queue) = Hub::new(ME);
    let hub = Arc::new(hub);
    {
        let hub = hub.clone();
        thread::spawn(move || board_loop(&hub, &queue, wanted_port));
    }
    run_input_side(hub);
}

/// Holds a lock that only one daemon per computer can have, until exit.
fn single_instance() -> Result<std::fs::File, String> {
    // A fixed path on macOS: $TMPDIR differs between a Terminal window and
    // an SSH session, and both must see the same lock.
    let path = if cfg!(unix) {
        std::path::PathBuf::from("/tmp/omni-kvm.lock")
    } else {
        std::env::temp_dir().join("omni-kvm.lock")
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("Another Omni-KVM daemon is already running on this computer; stop it first.".into())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(format!("Cannot lock {}: {e}", path.display())),
    }
}

#[cfg(windows)]
fn run_input_side(hub: Arc<Hub>) {
    // The hooks must live on a thread that pumps messages: this one.
    if let Err(e) = input_windows::run(hub) {
        eprintln!("Input capture failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn run_input_side(hub: Arc<Hub>) {
    // The event tap runs on this thread's run loop.
    if let Err(e) = input_macos::run(hub) {
        eprintln!("Input capture failed: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_input_side(_hub: Arc<Hub>) {
    // No input capture on this OS: only the board thread works.
    loop {
        thread::park();
    }
}

#[cfg(windows)]
fn pointer(action: Pointer) {
    screen_windows::pointer(action);
}

#[cfg(target_os = "macos")]
fn pointer(action: Pointer) {
    screen_macos::pointer(action);
}

#[cfg(not(any(windows, target_os = "macos")))]
fn pointer(_action: Pointer) {}

fn node_name(node: Node) -> &'static str {
    match node {
        Node::Pc => "PC",
        Node::Mac => "Mac",
    }
}

/// Log lines the board thread keeps from repeating.
#[derive(Default)]
struct Notes {
    ignored: Option<Instant>,
    looking: Option<String>,
}

impl Notes {
    fn ignored(&mut self) {
        if self.ignored.is_none_or(|t| t.elapsed() >= IGNORED_LOG_INTERVAL) {
            self.ignored = Some(Instant::now());
            say!("[focus] switch ignored: the other computer cannot be reached (radio link or board down)");
        }
    }

    /// Says what the search for a board found, when that changes.
    fn looking(&mut self, text: String) {
        if self.looking.as_ref() != Some(&text) {
            say!("{text}");
            self.looking = Some(text);
        }
    }
}

/// Keeps a connection to the board (D6), and carries out the queue.
fn board_loop(hub: &Hub, queue: &Receiver<Out>, wanted_port: Option<String>) {
    let mut notes = Notes::default();
    loop {
        if let Some(port) = choose_board(wanted_port.as_deref(), &mut notes) {
            match Board::open(&port) {
                Ok(mut board) => {
                    notes.looking = None;
                    say!("Connected to the board on {port}");
                    let why = serve(&mut board, hub, queue, &mut notes);
                    // Closing the port tells the board this daemon is gone:
                    // it opens its gate and tells the other board.
                    drop(board);
                    say!("Lost the board on {port} ({why}); waiting for it to come back...");
                    hub.handle(Event::BoardLost);
                }
                Err(e) => notes.looking(format!("Could not open {port}: {e}")),
            }
        }
        // No board: do what needs none (the pointer, the log).
        let until = Instant::now() + LOOK_INTERVAL;
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            if let Ok(out) = queue.recv_timeout(left) {
                without_board(out, &mut notes);
            }
        }
    }
}

struct Timers {
    keepalive: Instant,
    /// When the other daemon was last heard, while it counts as running.
    heard: Option<Instant>,
    settle: Option<Instant>,
    gate: Option<Instant>,
    stats: Instant,
    last_status: Instant,
    stats_logged: Option<(Instant, bool)>,
}

/// Talks to the board until it is lost; returns why.
fn serve(board: &mut Board, hub: &Hub, queue: &Receiver<Out>, notes: &mut Notes) -> String {
    let start = Instant::now();
    let mut t = Timers {
        keepalive: start,
        // Silence counts from now: a daemon that was running before
        // this board connection must answer again (S8).
        heard: Some(start),
        settle: None,
        gate: None,
        stats: start,
        last_status: start,
        stats_logged: None,
    };
    let mut peer_running = false;
    // Whether the board has told us the link state since we connected.
    let mut link_known = false;
    hub.handle(Event::BoardBack);
    loop {
        let now = Instant::now();
        if now >= t.keepalive {
            t.keepalive = now + KEEPALIVE;
            hub.handle(Event::KeepaliveDue);
        }
        if t.heard.is_some_and(|heard| now >= heard + PEER_SILENT) {
            t.heard = None;
            if peer_running {
                peer_running = false;
                say!("[peer] the other computer's daemon stopped answering");
            }
            hub.handle(Event::PeerSilent);
        }
        if t.settle.is_some_and(|settle| now >= settle) {
            t.settle = None;
            hub.handle(Event::SettleDone);
        }
        if t.gate.is_some_and(|gate| now >= gate) {
            t.gate = None;
            let mut g = hub.lock();
            if g.mode() == (Mode::Closing { confirmed: false }) {
                g.handle(Event::GateTimeout);
                return "it did not confirm closing its input gate".into();
            }
        }
        if now >= t.stats {
            t.stats = now + STATS_INTERVAL;
            if let Err(e) = board.request_stats() {
                return format!("error asking for link stats: {e}");
            }
        }
        if now >= t.last_status + BOARD_TIMEOUT {
            return "it stopped answering".into();
        }

        let packets = match board.receive() {
            Ok(packets) => packets,
            Err(e) => return format!("error reading: {e}"),
        };
        for packet in packets {
            match packet[2] {
                msg::DAEMON_STATUS => {
                    t.last_status = now;
                    match Status::parse(&packet) {
                        Ok(Status::LinkChanged(up)) => {
                            link_known = true;
                            say!("[link] radio link {}", if up { "up" } else { "down" });
                            hub.handle(if up { Event::LinkUp } else { Event::LinkDown });
                        }
                        Ok(Status::GateClosed(n)) => hub.handle(Event::GateClosed(n)),
                        Ok(Status::LinkStats(s)) => {
                            // The board reports the link the moment we
                            // connect, but that report can be lost: opening
                            // the port discards what arrived first (the
                            // serial library flushes on macOS; seen
                            // 2026-09-26). The answer to our first request,
                            // sent after that, says it instead.
                            if !link_known {
                                link_known = true;
                                say!("[link] radio link {}", if s.link_up { "up" } else { "down" });
                                hub.handle(if s.link_up { Event::LinkUp } else { Event::LinkDown });
                            }
                            if t.stats_logged.is_none_or(|(at, up)| up != s.link_up || now >= at + STATS_LOG_INTERVAL) {
                                t.stats_logged = Some((now, s.link_up));
                                print_link(&s);
                            }
                        }
                        Ok(Status::Other(_)) => {}
                        Err(e) => say!("Board error: {e}"),
                    }
                }
                msg::VIEW => {
                    if let Some((view, entry)) = protocol::decode_view(&packet) {
                        if !peer_running {
                            peer_running = true;
                            say!("[peer] the other computer's daemon is running");
                        }
                        t.heard = Some(now);
                        hub.handle(Event::ViewReceived(view, entry));
                    }
                }
                _ => {}
            }
        }

        // The queue: what the machine decided, and input to forward, in
        // the order it was decided.
        let mut work = Vec::new();
        if let Ok(first) = queue.recv_timeout(IDLE_WAIT) {
            work.push(first);
            work.extend(queue.try_iter());
        }
        for out in merge_moves(work) {
            if let Err(e) = carry_out(out, board, &mut t, notes) {
                return format!("error writing: {e}");
            }
        }
    }
}

/// Mouse movement that piled up in the queue (while a write to the board
/// waited: the board holds the computer back when its radio is busy) is
/// merged, so it arrives at once instead of late, packet by packet (B6).
/// Only neighbours with the same buttons merge: everything keeps its order.
fn merge_moves(work: Vec<Out>) -> Vec<Out> {
    let mut merged: Vec<Out> = Vec::with_capacity(work.len());
    for out in work {
        if let (Some(Out::Input(msg::MOUSE_MOVE, last)), Out::Input(msg::MOUSE_MOVE, next)) = (merged.last_mut(), &out) {
            let field = |p: &[u8; 5], i: usize| i16::from_le_bytes([p[i], p[i + 1]]) as i32;
            let (dx, dy) = (field(last, 0) + field(next, 0), field(last, 2) + field(next, 2));
            let fits = |v: i32| (i16::MIN as i32..=i16::MAX as i32).contains(&v);
            if last[4] == next[4] && fits(dx) && fits(dy) {
                last[0..2].copy_from_slice(&(dx as i16).to_le_bytes());
                last[2..4].copy_from_slice(&(dy as i16).to_le_bytes());
                continue;
            }
        }
        merged.push(out);
    }
    merged
}

fn carry_out(out: Out, board: &mut Board, t: &mut Timers, notes: &mut Notes) -> std::io::Result<()> {
    match out {
        Out::Input(msg_type, payload) => board.send(msg_type, &payload),
        Out::View(view, entry) => board.send(msg::VIEW, &protocol::encode_view(view, entry)),
        Out::CloseGate(n) => {
            t.gate = Some(Instant::now() + GATE_TIMEOUT);
            board.send(msg::DAEMON_CMD, &[protocol::CMD_GATE_CLOSE, n])
        }
        Out::OpenGate => board.send(msg::DAEMON_CMD, &[protocol::CMD_GATE_OPEN]),
        Out::KeyState(state) => board.send(msg::KEY_STATE, &state),
        Out::StartSettle => {
            t.settle = Some(Instant::now() + SETTLE);
            Ok(())
        }
        other => {
            without_board(other, notes);
            Ok(())
        }
    }
}

/// What needs no board: the pointer and the log. The rest was for a board
/// that is not there.
fn without_board(out: Out, notes: &mut Notes) {
    match out {
        Out::Pointer(action) => pointer(action),
        Out::Changed(view, mode) => say!("{}", describe(view, mode)),
        Out::Ignored => notes.ignored(),
        _ => {}
    }
}

fn describe(view: View, mode: Mode) -> String {
    let focus = match view.holder {
        Holder::Split => "split mode".to_string(),
        Holder::At(node) => format!("focus on the {}", node_name(node)),
    };
    let here = match mode {
        Mode::Split => "own keyboard and mouse here",
        Mode::Focused => "all input here",
        Mode::Closing { .. } => "closing the board's input gate",
        Mode::Unfocused => "forwarding own input",
    };
    format!(
        "[focus] {focus} (view {} by the {}); this {}: {here}",
        view.epoch,
        node_name(view.decider),
        node_name(ME)
    )
}

fn print_link(s: &LinkStats) {
    say!(
        "[link] {} @ {} | session {} | from host {} (dropped {}) | sent {} (resent {}, gave up {}) | frames failed {}/{}",
        if s.link_up { "UP" } else { "DOWN" },
        protocol::phy_rate_name(s.phy_rate),
        s.session,
        s.host_received,
        s.host_dropped,
        s.radio_sent,
        s.radio_retransmits,
        s.radio_gave_up,
        s.frames_failed,
        s.frames_acked + s.frames_failed,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(dx: i16, dy: i16, buttons: u8) -> Out {
        let [x0, x1] = dx.to_le_bytes();
        let [y0, y1] = dy.to_le_bytes();
        Out::Input(msg::MOUSE_MOVE, [x0, x1, y0, y1, buttons])
    }

    #[test]
    fn piled_up_movement_merges_in_order() {
        let key = Out::Input(msg::KEY_DOWN, [0x04, 0, 0, 0, 0]);
        let work = vec![
            moved(3, -1, 0),
            moved(4, -2, 0),
            moved(0, 0, 1), // left button down: not merged into the move before it
            moved(5, 5, 1),
            key,
            moved(1, 1, 1), // after the key: stays after it
            moved(30000, 0, 1),
            moved(30000, 0, 1), // would overflow: a packet of its own
        ];
        assert_eq!(
            merge_moves(work),
            vec![moved(7, -3, 0), moved(5, 5, 1), key, moved(30001, 1, 1), moved(30000, 0, 1)]
        );
    }
}

/// Picks the board to talk to, or says why there is none.
fn choose_board(wanted: Option<&str>, notes: &mut Notes) -> Option<String> {
    let found = board::find();
    if let Some(port) = wanted {
        if found.iter().any(|b| b.port == port) {
            return Some(port.to_string());
        }
        notes.looking(format!("No Omni-KVM board on {port}."));
        return None;
    }
    match found.as_slice() {
        [] => {
            notes.looking("No Omni-KVM board found. Plug the board's \"USB\" port into this computer.".into());
            None
        }
        [only] => Some(only.port.clone()),
        several => {
            let list: Vec<String> = several
                .iter()
                .map(|b| format!("{} (serial {})", b.port, b.serial.as_deref().unwrap_or("?")))
                .collect();
            notes.looking(format!(
                "Several boards found: {}. Pass the port to use, e.g. `omni-kvm {}`.",
                list.join(", "),
                several[0].port
            ));
            None
        }
    }
}
