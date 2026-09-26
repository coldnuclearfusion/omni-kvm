//! The focus state machine of `docs/focus-design.md`: where keyboard and
//! mouse input goes, and what this daemon has to do about it.
//!
//! Pure logic: no operating system, no board, no clock. The daemon feeds
//! it events (a push past the screen edge, the hotkey, a view from the
//! other daemon, the link going down, ...) and carries out the actions it
//! returns (send a view, close the board's input gate, start forwarding,
//! ...). Being pure, it can be run through every order of events by the
//! model check in `focus/check.rs`, and the daemon then uses the very
//! code that was checked (through `hub.rs`).

use std::collections::BTreeMap;

/// The two computers. The PC ranks higher: when two views are otherwise
/// equal, the PC's decision wins (S6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Mac,
    Pc,
}

impl Node {
    pub fn other(self) -> Node {
        match self {
            Node::Mac => Node::Pc,
            Node::Pc => Node::Mac,
        }
    }
}

/// Where the focus is: on one computer, or nowhere (split mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    Split,
    At(Node),
}

impl Holder {
    /// Last tie-breaker between views (see [`View`]).
    fn rank(self) -> u8 {
        match self {
            Holder::Split => 0,
            Holder::At(Node::Mac) => 1,
            Holder::At(Node::Pc) => 2,
        }
    }
}

/// A node's view of the focus. The newer of two views wins: higher
/// epoch, then the PC's decision, then the holder (equal epoch and
/// decider with different holders happens only after a restart resets
/// the counter). Any two views are therefore ordered, and both nodes
/// always pick the same one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct View {
    pub epoch: u32,
    pub decider: Node,
    pub holder: Holder,
}

impl View {
    fn order(&self) -> (u32, Node, u8) {
        (self.epoch, self.decider, self.holder.rank())
    }

    pub fn is_newer_than(&self, other: &View) -> bool {
        self.order() > other.order()
    }
}

/// Where the pointer enters after an edge switch: just inside the facing
/// edge, at this height (0 = top, 65535 = bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    pub height: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Each computer's devices control only that computer.
    Split,
    /// All input goes to this computer.
    Focused,
    /// On the way to Unfocused: the board's gate is closing (`confirmed`
    /// once the board says so, then the settle time runs). Own input is
    /// dropped meanwhile.
    Closing { confirmed: bool },
    /// Own input is swallowed and forwarded to the other computer.
    Unfocused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// The pointer was pushed past this screen's facing edge by the
    /// resistance, by any device.
    EdgePushed(Entry),
    /// The switch hotkey on this computer's own keyboard (H3).
    Hotkey,
    /// A view from the other daemon, with the entry position if it came
    /// with an edge switch.
    ViewReceived(View, Option<Entry>),
    LinkUp,
    LinkDown,
    /// Nothing heard from the other daemon for 3 s.
    PeerSilent,
    BoardLost,
    BoardBack,
    /// Time to send the view again (every second).
    KeepaliveDue,
    /// The board confirms it closed its input gate, answering the
    /// close request with this number.
    GateClosed(u8),
    /// No confirmation within 100 ms.
    GateTimeout,
    /// The settle time after the gate closed is over (R2).
    SettleDone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// Send this view to the other daemon (through the board).
    SendView(View, Option<Entry>),
    /// Ask the board to close its input gate. It answers with
    /// GateClosed carrying the same number: a late answer to an earlier
    /// request must not be taken for this one.
    CloseGate(u8),
    OpenGate,
    /// Start the settle timer; SettleDone when it runs out.
    StartSettle,
    /// Start or stop forwarding own input.
    Forwarding(bool),
    /// Send the other board the keys and buttons still held that were
    /// forwarded ([`Machine::forwarded_held`]). It lets go of any others
    /// it holds, which repairs a release the radio lost (B5).
    SendKeyState,
    Pointer(Pointer),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pointer {
    /// Remember where the pointer is and keep it there (Mac: hide it).
    Freeze,
    /// Show it again where it was.
    Resume,
    /// Put it just inside the facing edge at this height, and show it.
    Enter(Entry),
}

/// What to do with an input event that reached this daemon's capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// Let this computer have it.
    Pass,
    /// Swallow it and send it to the other computer.
    Forward,
    /// Swallow it.
    Drop,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Machine {
    me: Node,
    view: View,
    mode: Mode,
    /// Highest epoch seen, own or received: a new view gets this + 1.
    highest_epoch: u32,
    link_up: bool,
    board: bool,
    /// Heard from the other daemon since it last went silent. Only a
    /// daemon that was there can "go silent" (S8); with none there at all
    /// (B8) the user may switch to that computer on purpose.
    peer_heard: bool,
    /// Where the press of each key or button still held went, so its
    /// release goes to the same place (D7).
    pressed: BTreeMap<u32, Route>,
    /// Number of the latest gate close request.
    close_request: u8,
}

impl Machine {
    /// A daemon that just started: split mode (B2), gate open.
    pub fn new(me: Node, board: bool, link_up: bool) -> (Machine, Vec<Action>) {
        let view = View { epoch: 0, decider: me, holder: Holder::Split };
        let m = Machine {
            me,
            view,
            mode: Mode::Split,
            highest_epoch: 0,
            link_up,
            board,
            peer_heard: false,
            pressed: BTreeMap::new(),
            close_request: 0,
        };
        let mut out = Vec::new();
        if board {
            out.push(Action::OpenGate);
            out.push(Action::SendView(view, None));
        }
        (m, out)
    }

    pub fn view(&self) -> View {
        self.view
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn forwarding(&self) -> bool {
        self.mode == Mode::Unfocused
    }

    #[cfg(test)]
    pub fn peer_heard(&self) -> bool {
        self.peer_heard
    }

    pub fn handle(&mut self, event: Event) -> Vec<Action> {
        let mut out = Vec::new();
        match event {
            // Input can only go to the other computer over the link: with
            // it down, a switch there is ignored (and the user told, B9).
            Event::EdgePushed(entry) => {
                if matches!(self.mode, Mode::Focused | Mode::Split) && self.can_reach_peer() {
                    self.decide(Holder::At(self.me.other()), Some(entry), &mut out);
                }
            }
            Event::Hotkey => match self.mode {
                Mode::Focused | Mode::Split if self.can_reach_peer() => {
                    self.decide(Holder::At(self.me.other()), None, &mut out)
                }
                Mode::Focused | Mode::Split => {}
                Mode::Unfocused => self.decide(Holder::At(self.me), None, &mut out),
                Mode::Closing { .. } => {} // own input is dropped while closing
            },
            Event::ViewReceived(view, entry) => {
                self.peer_heard = true;
                self.highest_epoch = self.highest_epoch.max(view.epoch);
                if view.is_newer_than(&self.view) {
                    self.view = view;
                    self.apply(entry, &mut out);
                }
            }
            Event::LinkUp => {
                self.link_up = true;
                out.push(Action::SendView(self.view, None));
            }
            Event::LinkDown => {
                self.link_up = false;
                self.fall_back(&mut out);
            }
            Event::PeerSilent => {
                if self.peer_heard {
                    self.peer_heard = false;
                    self.fall_back(&mut out);
                }
            }
            Event::BoardLost => {
                self.board = false;
                self.link_up = false;
                self.fall_back(&mut out);
            }
            Event::BoardBack => {
                self.board = true;
                // A board that lost its daemon's connection opens its gate
                // (B8); we are in split mode after BoardLost, so keep it so.
                out.push(Action::OpenGate);
                out.push(Action::SendView(self.view, None));
            }
            Event::KeepaliveDue => {
                out.push(Action::SendView(self.view, None));
                if self.forwarding() {
                    out.push(Action::SendKeyState);
                }
            }
            Event::GateClosed(request) => {
                if self.mode == (Mode::Closing { confirmed: false }) && request == self.close_request {
                    self.mode = Mode::Closing { confirmed: true };
                    out.push(Action::StartSettle);
                }
            }
            Event::SettleDone => {
                if self.mode == (Mode::Closing { confirmed: true }) {
                    self.mode = Mode::Unfocused;
                    out.push(Action::Forwarding(true));
                }
            }
            Event::GateTimeout => {
                if self.mode == (Mode::Closing { confirmed: false }) {
                    self.board = false;
                    self.link_up = false;
                    self.fall_back(&mut out);
                }
            }
        }
        out
    }

    fn can_reach_peer(&self) -> bool {
        self.link_up && self.board
    }

    /// Where an input event goes now, by mode.
    fn route_now(&self) -> Route {
        match self.mode {
            Mode::Focused | Mode::Split => Route::Pass,
            Mode::Closing { .. } => Route::Drop,
            Mode::Unfocused => Route::Forward,
        }
    }

    /// A key or button went down (`id`: which one). Remembered, so its
    /// release follows it (D7).
    pub fn route_press(&mut self, id: u32) -> Route {
        let route = self.route_now();
        self.pressed.insert(id, route);
        route
    }

    /// A key or button went up: same place as its press. One pressed
    /// before this daemon started goes to this computer.
    pub fn route_release(&mut self, id: u32) -> Route {
        self.pressed.remove(&id).unwrap_or(Route::Pass)
    }

    /// A held key repeats (auto-repeat): where its press went, which
    /// stays as it was. One pressed before this daemon started goes to
    /// this computer.
    pub fn route_held(&self, id: u32) -> Route {
        self.pressed.get(&id).copied().unwrap_or(Route::Pass)
    }

    /// Movement and scrolling: no memory needed.
    pub fn route_motion(&self) -> Route {
        self.route_now()
    }

    /// Keys and buttons still held whose press was forwarded.
    pub fn forwarded_held(&self) -> impl Iterator<Item = u32> + '_ {
        self.pressed.iter().filter(|(_, route)| **route == Route::Forward).map(|(id, _)| *id)
    }

    /// A new view made here: epoch = highest seen + 1.
    fn decide(&mut self, holder: Holder, entry: Option<Entry>, out: &mut Vec<Action>) {
        self.highest_epoch += 1;
        self.view = View { epoch: self.highest_epoch, decider: self.me, holder };
        // The entry position is for the computer that becomes focused.
        out.push(Action::SendView(self.view, entry));
        self.apply(None, out);
    }

    /// Link, other daemon or board gone: split mode, unless already there.
    fn fall_back(&mut self, out: &mut Vec<Action>) {
        if self.view.holder != Holder::Split {
            self.decide(Holder::Split, None, out);
        }
    }

    /// Brings the mode in line with the current view.
    fn apply(&mut self, entry: Option<Entry>, out: &mut Vec<Action>) {
        let place = |entry: Option<Entry>| Action::Pointer(entry.map_or(Pointer::Resume, Pointer::Enter));
        match self.view.holder {
            Holder::At(node) if node == self.me => match self.mode {
                Mode::Focused => {}
                Mode::Split => {
                    self.mode = Mode::Focused;
                    if let Some(entry) = entry {
                        out.push(Action::Pointer(Pointer::Enter(entry)));
                    }
                }
                Mode::Closing { .. } => {
                    self.mode = Mode::Focused;
                    out.push(Action::OpenGate);
                    out.push(place(entry));
                }
                Mode::Unfocused => {
                    // Stop forwarding before opening the gate (S3).
                    out.push(Action::Forwarding(false));
                    out.push(Action::OpenGate);
                    self.mode = Mode::Focused;
                    out.push(place(entry));
                }
            },
            Holder::At(_) => match self.mode {
                Mode::Focused | Mode::Split => {
                    self.mode = Mode::Closing { confirmed: false };
                    self.close_request = self.close_request.wrapping_add(1);
                    out.push(Action::Pointer(Pointer::Freeze));
                    out.push(Action::CloseGate(self.close_request));
                }
                Mode::Closing { .. } | Mode::Unfocused => {}
            },
            Holder::Split => match self.mode {
                Mode::Split => {}
                Mode::Focused => self.mode = Mode::Split,
                Mode::Closing { .. } => {
                    self.mode = Mode::Split;
                    out.push(Action::OpenGate);
                    out.push(Action::Pointer(Pointer::Resume));
                }
                Mode::Unfocused => {
                    out.push(Action::Forwarding(false));
                    out.push(Action::OpenGate);
                    self.mode = Mode::Split;
                    out.push(Action::Pointer(Pointer::Resume));
                }
            },
        }
    }
}

#[cfg(test)]
mod check;

#[cfg(test)]
mod tests {
    use super::*;

    fn view(epoch: u32, decider: Node, holder: Holder) -> View {
        View { epoch, decider, holder }
    }

    #[test]
    fn views_are_totally_ordered() {
        let a = view(3, Node::Mac, Holder::At(Node::Pc));
        let b = view(3, Node::Pc, Holder::At(Node::Mac));
        assert!(b.is_newer_than(&a), "same epoch: the PC's decision wins");
        assert!(view(4, Node::Mac, Holder::Split).is_newer_than(&b), "higher epoch wins");
        let c = view(1, Node::Pc, Holder::Split);
        let d = view(1, Node::Pc, Holder::At(Node::Mac));
        assert!(d.is_newer_than(&c) != c.is_newer_than(&d), "after a restart: holder breaks the tie");
    }

    #[test]
    fn edge_switch_closes_the_gate_before_forwarding() {
        let (mut pc, _) = Machine::new(Node::Pc, true, true);
        let out = pc.handle(Event::EdgePushed(Entry { height: 100 }));
        assert!(out.contains(&Action::CloseGate(1)));
        assert_eq!(pc.route_motion(), Route::Drop, "closing: own input dropped");
        pc.handle(Event::GateClosed(1));
        assert!(!pc.forwarding(), "not before the settle time");
        pc.handle(Event::SettleDone);
        assert!(pc.forwarding());
    }

    #[test]
    fn a_release_follows_its_press() {
        let (mut pc, _) = Machine::new(Node::Pc, true, true);
        assert_eq!(pc.route_press(7), Route::Pass);
        pc.handle(Event::Hotkey);
        pc.handle(Event::GateClosed(1));
        pc.handle(Event::SettleDone);
        assert_eq!(pc.route_held(7), Route::Pass, "repeats follow the press too");
        assert_eq!(pc.route_release(7), Route::Pass, "pressed here, released here");
        assert_eq!(pc.route_press(8), Route::Forward);
        assert_eq!(pc.route_held(8), Route::Forward);
    }
}
