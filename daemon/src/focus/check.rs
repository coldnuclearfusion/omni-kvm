//! Model check of the focus design (docs/focus-design.md, section 10).
//!
//! Builds a small world around two copies of the focus state machine:
//! boards with input gates, one radio direction each way (in order, but
//! messages may be lost or doubled), each computer's input queue between
//! the devices and the daemon's capture (input a board types in reaches
//! the capture later, as for real), a user, and the faults of the failure
//! model. It then tries every order of events up to a bound and checks
//! the rules of docs/requirements.md:
//!
//! - in every state: S3 (no echo), S4 (one place per input), and a daemon
//!   never forwards while its board's gate is open (F7);
//! - from every state, once faults stop and messages flow: S1/S2/S9 (the
//!   views agree), S7 (split mode after link loss), B5 (no key left down
//!   after everything is released), S10 (the hotkey on a computer's own
//!   keyboard brings the focus back there).
//!
//! Modelling choices, to keep the number of states manageable:
//!
//! - A daemon's commands to its own board (over USB, well under a
//!   millisecond) take effect at once. The answers going the other way,
//!   the radio and the operating systems' input queues keep their delays,
//!   so events there can happen in any order. (The late gate confirmation
//!   this hides was found with an earlier, slower version of this model;
//!   numbered close requests fix it.)
//! - Keepalives are sent only once faults have stopped (`complete`): they
//!   spread views and repair key states, which is about where runs end up,
//!   not about the rules that must hold in every state.
//! - Pointer positions, time beyond "in some order", and the operating
//!   systems' own quirks are not modelled (the experiments cover those).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::thread;

use super::*;

/// Keys on each keyboard: an ordinary key and GUI (Windows / Command).
const KEYS: [u8; 2] = [1, 2];

fn node(i: usize) -> Node {
    if i == 0 { Node::Pc } else { Node::Mac }
}

fn other(i: usize) -> usize {
    1 - i
}

fn name(i: usize) -> &'static str {
    if i == 0 { "PC" } else { "Mac" }
}

/// One input event. `token` identifies a user action, to follow where it
/// ends up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Input {
    Press { key: u8, token: u8 },
    Release { key: u8 },
    Motion { token: u8 },
    /// Escape completing the hotkey. From this computer's own keyboard,
    /// or (with H6 on) passed on from the other computer's.
    HotkeyEsc,
}

/// Input on its way through a computer to the daemon's capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct OsInput {
    input: Input,
    /// Whose device produced it.
    origin: usize,
    /// Typed in by this computer's board.
    via_board: bool,
}

/// Daemon → its board (USB); carried out at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ToBoard {
    Open,
    Close(u8),
    View(View, Option<Entry>),
    Input(Input, usize),
    /// Keys still held that were forwarded (bit per key).
    KeyState(u16),
}

/// Board → its daemon (USB).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ToDaemon {
    GateClosed(u8),
    View(View, Option<Entry>),
    Link(bool),
}

/// Board → the other board (radio).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Air {
    View(View, Option<Entry>),
    Input(Input, usize),
    KeyState(u16),
    HostGone,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Computer {
    daemon: Option<Machine>,
    settle_pending: bool,
    /// Board plugged in.
    board: bool,
    gate_open: bool,
    /// Keys the board holds down on this computer.
    board_held: BTreeSet<u8>,
    os: VecDeque<OsInput>,
    to_daemon: VecDeque<ToDaemon>,
    /// Keys the apps see as down.
    apps_down: BTreeSet<u8>,
    /// Keys physically held on this computer's keyboard.
    held: BTreeSet<u8>,
}

impl Computer {
    fn new() -> Computer {
        Computer {
            daemon: None,
            settle_pending: false,
            board: true,
            gate_open: true,
            board_held: BTreeSet::new(),
            os: VecDeque::new(),
            to_daemon: VecDeque::new(),
            apps_down: BTreeSet::new(),
            held: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct World {
    c: [Computer; 2],
    /// `air[i]`: sent by board i, not yet received by the other.
    air: [VecDeque<Air>; 2],
    link_up: bool,
    /// Token → which computers' apps received it (bit per computer), for
    /// tokens still travelling somewhere (see `forget_finished`).
    delivered: BTreeMap<u8, u8>,
    actions_left: u8,
    faults_left: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// User actions (key presses, movements, hotkeys) per run.
    pub actions: u8,
    /// Faults per run: link loss, lost or doubled radio message, daemon
    /// crash, board unplugged, a daemon wrongly thought silent.
    pub faults: u8,
    /// The settle time (R2) outlasts the time input typed in by a board
    /// needs to reach the capture.
    pub settle_waits_for_os: bool,
    /// H6: the hotkey's Escape is also delivered where input was going.
    pub pass_hotkey_on: bool,
    /// Runs whose queues grow beyond this are not followed further.
    pub max_queue: usize,
}

/// One step between two states, for the trace of a failed check.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Start,
    Press(usize, u8),
    Release(usize, u8),
    Move(usize),
    Hotkey(usize),
    Capture(usize),
    CaptureEdge(usize),
    Air(usize),
    Daemon(usize),
    Settle(usize),
    GateTimeout(usize),
    PeerSilent(usize),
    LinkDown,
    LinkUp,
    Lose(usize),
    Double(usize),
    Crash(usize),
    Launch(usize),
    Unplug(usize),
    Replug(usize),
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Step::Start => write!(f, "start"),
            Step::Press(x, k) => write!(f, "{} key {k} down", name(x)),
            Step::Release(x, k) => write!(f, "{} key {k} up", name(x)),
            Step::Move(x) => write!(f, "{} mouse moves", name(x)),
            Step::Hotkey(x) => write!(f, "{} hotkey", name(x)),
            Step::Capture(x) => write!(f, "{} capture takes next input", name(x)),
            Step::CaptureEdge(x) => write!(f, "{} capture takes a movement: edge pushed", name(x)),
            Step::Air(x) => write!(f, "radio delivers from {} board", name(x)),
            Step::Daemon(x) => write!(f, "{} daemon takes next message from its board", name(x)),
            Step::Settle(x) => write!(f, "{} settle time over", name(x)),
            Step::GateTimeout(x) => write!(f, "{} gate confirmation timeout", name(x)),
            Step::PeerSilent(x) => write!(f, "{} finds the other daemon silent", name(x)),
            Step::LinkDown => write!(f, "LINK LOST"),
            Step::LinkUp => write!(f, "link back"),
            Step::Lose(x) => write!(f, "RADIO LOSES next message from {} board", name(x)),
            Step::Double(x) => write!(f, "RADIO DOUBLES next message from {} board", name(x)),
            Step::Crash(x) => write!(f, "{} DAEMON CRASHES", name(x)),
            Step::Launch(x) => write!(f, "{} daemon starts", name(x)),
            Step::Unplug(x) => write!(f, "{} BOARD UNPLUGGED", name(x)),
            Step::Replug(x) => write!(f, "{} board plugged back", name(x)),
        }
    }
}

type Check = Result<(), String>;

impl World {
    /// Both boards plugged in and linked; `daemons` says which daemons run.
    fn new(daemons: [bool; 2], cfg: &Config) -> World {
        let mut w = World {
            c: [Computer::new(), Computer::new()],
            air: [VecDeque::new(), VecDeque::new()],
            link_up: true,
            delivered: BTreeMap::new(),
            actions_left: cfg.actions,
            faults_left: cfg.faults,
        };
        for x in 0..2 {
            if daemons[x] {
                w.launch(x);
            }
        }
        w
    }

    fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }

    /// Tokens with a copy still on its way somewhere.
    fn travelling(&self) -> BTreeSet<u8> {
        let token = |input: &Input| match *input {
            Input::Press { token, .. } | Input::Motion { token } => Some(token),
            _ => None,
        };
        let mut live = BTreeSet::new();
        for c in &self.c {
            live.extend(c.os.iter().filter_map(|e| token(&e.input)));
        }
        for q in &self.air {
            live.extend(q.iter().filter_map(|m| match m {
                Air::Input(input, _) => token(input),
                _ => None,
            }));
        }
        live
    }

    /// Once no copy of an input is left anywhere, it can no longer reach a
    /// second computer (S4), so where it went can be forgotten. Otherwise
    /// states that differ only in finished inputs would count as different.
    fn forget_finished(&mut self) {
        let live = self.travelling();
        self.delivered.retain(|token, _| live.contains(token));
    }

    /// The smallest token not in use.
    fn new_token(&self) -> u8 {
        let live = self.travelling();
        (0..=u8::MAX).find(|t| !live.contains(t) && !self.delivered.contains_key(t)).expect("a free token")
    }

    fn within_bounds(&self, max: usize) -> bool {
        self.air.iter().all(|q| q.len() <= max) && self.c.iter().all(|c| c.os.len() <= max && c.to_daemon.len() <= max)
    }

    // --- the daemon and its board ---

    fn run(&mut self, x: usize, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::SendView(v, e) => self.board_command(x, ToBoard::View(v, e)),
                Action::CloseGate(n) => self.board_command(x, ToBoard::Close(n)),
                Action::OpenGate => self.board_command(x, ToBoard::Open),
                Action::SendKeyState => {
                    let d = self.c[x].daemon.as_ref().expect("its daemon asked");
                    let mask = d.forwarded_held().fold(0u16, |m, key| m | 1 << key);
                    self.board_command(x, ToBoard::KeyState(mask));
                }
                Action::StartSettle => self.c[x].settle_pending = true,
                Action::Forwarding(_) | Action::Pointer(_) => {}
            }
        }
    }

    /// The board carries out a command from its daemon. A daemon without
    /// its board writes into nothing.
    fn board_command(&mut self, x: usize, cmd: ToBoard) {
        if !self.c[x].board {
            return;
        }
        match cmd {
            ToBoard::Open => self.c[x].gate_open = true,
            ToBoard::Close(n) => {
                self.c[x].gate_open = false;
                self.release_all(x);
                self.c[x].to_daemon.push_back(ToDaemon::GateClosed(n));
            }
            ToBoard::View(v, e) => self.send_air(x, Air::View(v, e)),
            ToBoard::Input(input, origin) => self.send_air(x, Air::Input(input, origin)),
            ToBoard::KeyState(mask) => self.send_air(x, Air::KeyState(mask)),
        }
    }

    fn send_air(&mut self, x: usize, msg: Air) {
        if self.link_up {
            self.air[x].push_back(msg);
        }
    }

    fn handle(&mut self, x: usize, event: Event) {
        if let Some(d) = self.c[x].daemon.as_mut() {
            let actions = d.handle(event);
            self.run(x, actions);
        }
    }

    fn launch(&mut self, x: usize) {
        let (m, actions) = Machine::new(node(x), self.c[x].board, self.link_up);
        self.c[x].daemon = Some(m);
        self.run(x, actions);
    }

    // --- where input ends up ---

    fn deliver(&mut self, x: usize, input: Input) -> Check {
        match input {
            Input::Press { key, token } => {
                self.c[x].apps_down.insert(key);
                self.mark(token, x)
            }
            Input::Release { key } => {
                self.c[x].apps_down.remove(&key);
                Ok(())
            }
            Input::Motion { token } => self.mark(token, x),
            Input::HotkeyEsc => Ok(()), // a plain Esc: no daemon to recognize it
        }
    }

    fn mark(&mut self, token: u8, x: usize) -> Check {
        let seen = self.delivered.entry(token).or_insert(0);
        *seen |= 1 << x;
        if *seen == 3 { Err(format!("S4: input #{token} reached both computers")) } else { Ok(()) }
    }

    /// The board lets go of every key it holds down on its computer.
    fn release_all(&mut self, x: usize) {
        for key in std::mem::take(&mut self.c[x].board_held) {
            let input = Input::Release { key };
            self.c[x].os.push_back(OsInput { input, origin: other(x), via_board: true });
        }
    }

    // --- internal steps ---

    /// The capture takes the next input. With `edge`, a movement it passes
    /// is also a push past the screen edge.
    fn capture(&mut self, x: usize, edge: bool, cfg: &Config) -> Check {
        let ev = self.c[x].os.pop_front().expect("input queued");
        let Some(d) = self.c[x].daemon.as_mut() else {
            return self.deliver(x, ev.input); // no daemon: straight to the apps
        };
        let route = match ev.input {
            Input::Press { key, .. } => d.route_press(key as u32),
            Input::Release { key } => d.route_release(key as u32),
            Input::Motion { .. } => d.route_motion(),
            Input::HotkeyEsc if !cfg.pass_hotkey_on => {
                // Kept by the daemon, never delivered or forwarded (H4).
                self.handle(x, Event::Hotkey);
                return Ok(());
            }
            Input::HotkeyEsc => {
                // H6: the Escape goes where input was going before the
                // switch, then the daemon acts on the hotkey. The daemon
                // cannot tell its own keyboard from keys its board typed
                // in, so one passed on from the other computer counts too.
                let route = d.route_motion();
                if route == Route::Forward {
                    if ev.origin != x {
                        return Err(format!("S3: the {} daemon sent back input that came from the other computer", name(x)));
                    }
                    self.board_command(x, ToBoard::Input(ev.input, ev.origin));
                }
                self.handle(x, Event::Hotkey);
                return Ok(());
            }
        };
        match route {
            Route::Pass => {
                self.deliver(x, ev.input)?;
                if edge {
                    self.handle(x, Event::EdgePushed(Entry { height: 0 }));
                }
                Ok(())
            }
            Route::Forward => {
                if ev.origin != x {
                    return Err(format!("S3: the {} daemon sent back input that came from the other computer", name(x)));
                }
                self.board_command(x, ToBoard::Input(ev.input, ev.origin));
                Ok(())
            }
            Route::Drop => Ok(()),
        }
    }

    fn air_step(&mut self, from: usize) {
        let msg = self.air[from].pop_front().expect("message in the air");
        let t = other(from);
        if !self.c[t].board {
            return;
        }
        match msg {
            Air::View(v, e) => {
                if self.c[t].daemon.is_some() {
                    self.c[t].to_daemon.push_back(ToDaemon::View(v, e));
                }
            }
            Air::Input(input, origin) => {
                if self.c[t].gate_open {
                    match input {
                        Input::Press { key, .. } => {
                            self.c[t].board_held.insert(key);
                        }
                        Input::Release { key } => {
                            self.c[t].board_held.remove(&key);
                        }
                        _ => {}
                    }
                    self.c[t].os.push_back(OsInput { input, origin, via_board: true });
                }
            }
            Air::KeyState(mask) => {
                if self.c[t].gate_open {
                    let stale: Vec<u8> = self.c[t].board_held.iter().copied().filter(|k| mask & 1 << k == 0).collect();
                    for key in stale {
                        self.c[t].board_held.remove(&key);
                        let input = Input::Release { key };
                        self.c[t].os.push_back(OsInput { input, origin: from, via_board: true });
                    }
                }
            }
            Air::HostGone => self.release_all(t),
        }
    }

    fn daemon_step(&mut self, x: usize) {
        let event = match self.c[x].to_daemon.pop_front().expect("message queued") {
            ToDaemon::GateClosed(n) => Event::GateClosed(n),
            ToDaemon::View(v, e) => Event::ViewReceived(v, e),
            ToDaemon::Link(true) => Event::LinkUp,
            ToDaemon::Link(false) => Event::LinkDown,
        };
        self.handle(x, event);
    }

    fn settle_ready(&self, x: usize, cfg: &Config) -> bool {
        let c = &self.c[x];
        c.settle_pending && c.daemon.is_some() && (!cfg.settle_waits_for_os || c.os.iter().all(|e| !e.via_board))
    }

    fn settle(&mut self, x: usize) {
        self.c[x].settle_pending = false;
        self.handle(x, Event::SettleDone);
    }

    fn gate_timeout_ready(&self, x: usize) -> bool {
        let c = &self.c[x];
        !c.board && c.daemon.as_ref().is_some_and(|d| d.mode() == Mode::Closing { confirmed: false })
    }

    /// The other daemon really is gone: it will be found silent.
    fn silence_is_real(&self, x: usize) -> bool {
        self.c[other(x)].daemon.is_none() && self.c[x].daemon.as_ref().is_some_and(|d| d.peer_heard())
    }

    /// Nothing left to happen but new user actions (and keepalives): every
    /// queue empty, no timer pending. From any state, once faults stop,
    /// draining the queues leads to such a state, which the exploration
    /// reaches too; so checking where runs end up only here covers every
    /// state.
    fn quiescent(&self) -> bool {
        self.air.iter().all(|q| q.is_empty())
            && (0..2).all(|x| {
                let c = &self.c[x];
                c.os.is_empty()
                    && c.to_daemon.is_empty()
                    && !c.settle_pending
                    && !self.gate_timeout_ready(x)
                    && !self.silence_is_real(x)
            })
    }

    // --- faults and recoveries ---

    fn link_down(&mut self) {
        self.link_up = false;
        self.air = [VecDeque::new(), VecDeque::new()];
        for x in 0..2 {
            if self.c[x].board {
                self.release_all(x); // F8
                if self.c[x].daemon.is_some() {
                    self.c[x].to_daemon.push_back(ToDaemon::Link(false));
                }
            }
        }
    }

    fn link_back(&mut self) {
        self.link_up = true;
        for x in 0..2 {
            if self.c[x].daemon.is_some() {
                self.c[x].to_daemon.push_back(ToDaemon::Link(true));
            }
        }
    }

    fn crash(&mut self, x: usize) {
        let c = &mut self.c[x];
        c.daemon = None;
        c.settle_pending = false;
        c.to_daemon.clear();
        if c.board {
            c.gate_open = true; // no daemon: gate open (B8)
            self.send_air(x, Air::HostGone); // F8
        }
    }

    fn unplug(&mut self, x: usize) {
        self.release_all(x); // the computer lets go of a vanished keyboard's keys
        let c = &mut self.c[x];
        c.board = false;
        c.gate_open = true;
        c.to_daemon.clear();
        if self.link_up {
            self.link_up = false;
            self.air = [VecDeque::new(), VecDeque::new()];
            let o = other(x);
            if self.c[o].board {
                self.release_all(o);
                if self.c[o].daemon.is_some() {
                    self.c[o].to_daemon.push_back(ToDaemon::Link(false));
                }
            }
        }
        self.handle(x, Event::BoardLost);
    }

    fn replug(&mut self, x: usize) {
        self.c[x].board = true;
        self.c[x].gate_open = true;
        self.handle(x, Event::BoardBack);
        if self.c[other(x)].board {
            self.link_back();
        }
    }

    // --- exploring ---

    fn successors(&self, cfg: &Config) -> Vec<(Step, Result<World, String>)> {
        let mut out: Vec<(Step, Result<World, String>)> = Vec::new();
        let mut add = |step: Step, f: &dyn Fn(&mut World) -> Check| {
            let mut n = self.clone();
            let r = f(&mut n).map(|_| {
                n.forget_finished();
                n
            });
            out.push((step, r));
        };

        for x in 0..2 {
            let c = &self.c[x];
            if let Some(ev) = c.os.front() {
                add(Step::Capture(x), &|w| w.capture(x, false, cfg));
                let passes = c.daemon.as_ref().is_some_and(|d| d.route_motion() == Route::Pass);
                if matches!(ev.input, Input::Motion { .. }) && passes {
                    add(Step::CaptureEdge(x), &|w| w.capture(x, true, cfg));
                }
            }
            if self.link_up && !self.air[x].is_empty() {
                add(Step::Air(x), &|w| Ok(w.air_step(x)));
            }
            if c.daemon.is_some() && !c.to_daemon.is_empty() {
                add(Step::Daemon(x), &|w| Ok(w.daemon_step(x)));
            }
            if self.settle_ready(x, cfg) {
                add(Step::Settle(x), &|w| Ok(w.settle(x)));
            }
            if self.gate_timeout_ready(x) {
                add(Step::GateTimeout(x), &|w| Ok(w.handle(x, Event::GateTimeout)));
            }
            if self.silence_is_real(x) {
                add(Step::PeerSilent(x), &|w| Ok(w.handle(x, Event::PeerSilent)));
            }

            // The user.
            if self.actions_left > 0 {
                for key in KEYS {
                    if !c.held.contains(&key) {
                        add(Step::Press(x, key), &|w| {
                            w.actions_left -= 1;
                            let token = w.new_token();
                            w.c[x].held.insert(key);
                            w.c[x].os.push_back(OsInput { input: Input::Press { key, token }, origin: x, via_board: false });
                            Ok(())
                        });
                    }
                }
                add(Step::Move(x), &|w| {
                    w.actions_left -= 1;
                    let token = w.new_token();
                    w.c[x].os.push_back(OsInput { input: Input::Motion { token }, origin: x, via_board: false });
                    Ok(())
                });
                add(Step::Hotkey(x), &|w| {
                    w.actions_left -= 1;
                    w.c[x].os.push_back(OsInput { input: Input::HotkeyEsc, origin: x, via_board: false });
                    Ok(())
                });
            }
            for &key in &c.held {
                add(Step::Release(x, key), &|w| {
                    w.c[x].held.remove(&key);
                    w.c[x].os.push_back(OsInput { input: Input::Release { key }, origin: x, via_board: false });
                    Ok(())
                });
            }

            // Recoveries (free) and faults (from the budget).
            if c.daemon.is_none() {
                add(Step::Launch(x), &|w| Ok(w.launch(x)));
            }
            if !c.board {
                add(Step::Replug(x), &|w| Ok(w.replug(x)));
            }
            if self.faults_left > 0 {
                if c.daemon.is_some() {
                    add(Step::Crash(x), &|w| {
                        w.faults_left -= 1;
                        w.crash(x);
                        Ok(())
                    });
                    if !self.silence_is_real(x) {
                        add(Step::PeerSilent(x), &|w| {
                            w.faults_left -= 1;
                            w.handle(x, Event::PeerSilent);
                            Ok(())
                        });
                    }
                }
                if c.board {
                    add(Step::Unplug(x), &|w| {
                        w.faults_left -= 1;
                        w.unplug(x);
                        Ok(())
                    });
                }
                if self.link_up && !self.air[x].is_empty() {
                    add(Step::Lose(x), &|w| {
                        w.faults_left -= 1;
                        w.air[x].pop_front();
                        Ok(())
                    });
                    add(Step::Double(x), &|w| {
                        w.faults_left -= 1;
                        let head = *w.air[x].front().expect("message in the air");
                        w.air[x].push_front(head);
                        Ok(())
                    });
                }
            }
        }
        if self.link_up && self.faults_left > 0 && self.c.iter().all(|c| c.board) {
            add(Step::LinkDown, &|w| {
                w.faults_left -= 1;
                w.link_down();
                Ok(())
            });
        }
        if !self.link_up && self.c.iter().all(|c| c.board) {
            add(Step::LinkUp, &|w| Ok(w.link_back()));
        }
        out
    }

    // --- the checks ---

    /// Rules that must hold in every state.
    fn check_state(&self) -> Check {
        for x in 0..2 {
            let c = &self.c[x];
            if c.board && c.gate_open && c.daemon.as_ref().is_some_and(|d| d.forwarding()) {
                return Err(format!("F7: the {} daemon forwards while its board's gate is open", name(x)));
            }
        }
        Ok(())
    }

    /// Everything that happens once faults stop and messages flow: every
    /// queue drained, timers run, keepalives exchanged a few times.
    fn complete(&self, cfg: &Config) -> Result<World, String> {
        let mut w = self.clone();
        for _round in 0..4 {
            for _ in 0..200 {
                let mut progressed = false;
                for x in 0..2 {
                    while w.link_up && !w.air[x].is_empty() {
                        w.air_step(x);
                        progressed = true;
                    }
                    while w.c[x].daemon.is_some() && !w.c[x].to_daemon.is_empty() {
                        w.daemon_step(x);
                        progressed = true;
                    }
                    while !w.c[x].os.is_empty() {
                        w.capture(x, false, cfg)?;
                        progressed = true;
                    }
                    if w.settle_ready(x, cfg) {
                        w.settle(x);
                        progressed = true;
                    }
                    if w.gate_timeout_ready(x) {
                        w.handle(x, Event::GateTimeout);
                        progressed = true;
                    }
                    if w.silence_is_real(x) {
                        w.handle(x, Event::PeerSilent);
                        progressed = true;
                    }
                    w.check_state()?;
                }
                if !progressed {
                    break;
                }
            }
            for x in 0..2 {
                if w.c[x].daemon.is_some() && w.c[x].board && w.link_up {
                    w.handle(x, Event::KeepaliveDue);
                }
            }
        }
        Ok(w)
    }

    /// Rules about where every run ends up.
    fn check_eventually(&self, cfg: &Config) -> Check {
        let w = self.complete(cfg)?;
        let running = |c: &Computer| c.daemon.is_some() && c.board;

        if w.link_up && w.c.iter().all(running) {
            let a = w.c[0].daemon.as_ref().expect("running");
            let b = w.c[1].daemon.as_ref().expect("running");
            if a.view() != b.view() {
                return Err(format!("S2: views still differ once faults stopped: PC {:?}, Mac {:?}", a.view(), b.view()));
            }
            match (a.mode(), b.mode()) {
                (Mode::Split, Mode::Split) | (Mode::Focused, Mode::Unfocused) | (Mode::Unfocused, Mode::Focused) => {}
                (pc, mac) => return Err(format!("S1: modes do not match: PC {pc:?}, Mac {mac:?}")),
            }
        }
        if !w.link_up {
            for x in 0..2 {
                if let Some(d) = &w.c[x].daemon {
                    if d.mode() != Mode::Split {
                        return Err(format!("S7: link lost but the {} daemon is {:?}", name(x), d.mode()));
                    }
                }
            }
        }

        // B5: let go of every key; nothing may stay down anywhere.
        let mut r = w.clone();
        for x in 0..2 {
            for key in std::mem::take(&mut r.c[x].held) {
                r.c[x].os.push_back(OsInput { input: Input::Release { key }, origin: x, via_board: false });
            }
        }
        let r = r.complete(cfg)?;
        for x in 0..2 {
            if let Some(key) = r.c[x].apps_down.first() {
                return Err(format!("B5: key {key} still down on the {} after every key was released", name(x)));
            }
        }

        // S10: the hotkey on an unfocused computer's keyboard brings the focus back.
        for x in 0..2 {
            if w.c[x].daemon.as_ref().is_some_and(|d| d.mode() == Mode::Unfocused) {
                let mut h = w.clone();
                h.c[x].os.push_back(OsInput { input: Input::HotkeyEsc, origin: x, via_board: false });
                let h = h.complete(cfg)?;
                let mode = h.c[x].daemon.as_ref().expect("running").mode();
                if mode != Mode::Focused {
                    return Err(format!("S10: the hotkey on the {} left it {mode:?}", name(x)));
                }
            }
        }
        Ok(())
    }

    fn check(&self, cfg: &Config) -> Check {
        self.check_state()?;
        if self.quiescent() { self.check_eventually(cfg) } else { Ok(()) }
    }
}

/// A new state found from `from` by `step`.
struct Found {
    from: u64,
    step: Step,
    fp: u64,
    world: World,
}

/// A rule broken on the way from `from` by `step`.
struct Broken {
    from: u64,
    step: Step,
    error: String,
}

/// Explores every order of events from the start states, up to the bounds
/// in `cfg`, on all processor cores. Returns how many states were checked,
/// or the first broken rule with the steps that led there.
pub fn explore(cfg: &Config) -> Result<usize, String> {
    let starts = [[true, true], [true, false], [false, true]].map(|d| World::new(d, cfg));
    // State fingerprint → (previous state, step), to print the way back.
    let mut seen: HashMap<u64, (u64, Step)> = HashMap::new();
    let mut frontier: Vec<(u64, World)> = Vec::new();
    for w in starts {
        let fp = w.fingerprint();
        if seen.insert(fp, (fp, Step::Start)).is_none() {
            w.check(cfg).map_err(|e| trace(&seen, fp, None, &e))?;
            frontier.push((fp, w));
        }
    }
    let threads = thread::available_parallelism().map_or(4, |n| n.get());
    let mut depth = 0;
    while !frontier.is_empty() {
        depth += 1;
        let chunk = frontier.len().div_ceil(threads).max(1);
        let known = &seen;
        let results: Vec<Result<Vec<Found>, Broken>> = thread::scope(|s| {
            let handles: Vec<_> = frontier
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || {
                        let mut found = Vec::new();
                        for (from, w) in part {
                            for (step, result) in w.successors(cfg) {
                                let n = result.map_err(|error| Broken { from: *from, step, error })?;
                                if !n.within_bounds(cfg.max_queue) {
                                    continue;
                                }
                                let fp = n.fingerprint();
                                if known.contains_key(&fp) {
                                    continue;
                                }
                                n.check(cfg).map_err(|error| Broken { from: *from, step, error })?;
                                found.push(Found { from: *from, step, fp, world: n });
                            }
                        }
                        Ok(found)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("checker thread")).collect()
        });
        let mut next = Vec::new();
        for result in results {
            match result {
                Ok(found) => {
                    for f in found {
                        if !seen.contains_key(&f.fp) {
                            seen.insert(f.fp, (f.from, f.step));
                            next.push((f.fp, f.world));
                        }
                    }
                }
                Err(b) => return Err(trace(&seen, b.from, Some(b.step), &b.error)),
            }
        }
        eprintln!("  depth {depth:3}: {:9} new states, {:10} in all", next.len(), seen.len());
        frontier = next;
    }
    Ok(seen.len())
}

fn trace(seen: &HashMap<u64, (u64, Step)>, mut at: u64, last: Option<Step>, error: &str) -> String {
    let mut steps: Vec<Step> = last.into_iter().collect();
    while let Some(&(prev, step)) = seen.get(&at) {
        if step == Step::Start {
            break;
        }
        steps.push(step);
        at = prev;
    }
    steps.reverse();
    let lines: Vec<String> = steps.iter().enumerate().map(|(i, s)| format!("  {:2}. {s:?}", i + 1)).collect();
    format!("{error}\nafter:\n{}", lines.join("\n"))
}

const CHECKED: Config = Config { actions: 2, faults: 1, settle_waits_for_os: true, pass_hotkey_on: false, max_queue: 4 };

#[test]
fn design_holds() {
    let cfg = CHECKED;
    match explore(&cfg) {
        Ok(states) => println!("{states} states checked, every rule holds"),
        Err(e) => panic!("{e}"),
    }
}

/// R2: without the settle time, input a board typed in just before its
/// gate closed reaches the capture after forwarding started, and goes back.
#[test]
fn design_holds_with_the_hotkey_passed_on() {
    let cfg = Config { pass_hotkey_on: true, ..CHECKED };
    match explore(&cfg) {
        Ok(states) => println!("{states} states checked, every rule holds"),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn settle_time_is_needed() {
    let cfg = Config { actions: 3, faults: 0, settle_waits_for_os: false, ..CHECKED };
    let error = explore(&cfg).expect_err("expected an echo without the settle time");
    assert!(error.starts_with("S3"), "expected an echo (S3), got: {error}");
    println!("as expected without the settle time:\n{error}");
}

#[test]
#[ignore = "slow: cargo test --release -- --ignored"]
fn design_holds_deeper() {
    let cfg = Config { actions: 3, faults: 2, ..CHECKED };
    match explore(&cfg) {
        Ok(states) => println!("{states} states checked, every rule holds"),
        Err(e) => panic!("{e}"),
    }
}
