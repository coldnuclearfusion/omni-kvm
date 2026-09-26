//! Where the input side and the board thread meet.
//!
//! Both use the focus state machine (`focus.rs`, the code the model check
//! ran): the input side to route each input event and to report edge
//! pushes and the hotkey; the board thread for what the board and the other
//! daemon report, and for the timers. What the machine asks for, and each
//! input packet to forward, goes into one queue that the board thread works
//! through in order.
//!
//! The queue is only ever filled while the machine is locked, so its order
//! is the order in which the machine made its decisions: the board is told
//! to close its gate before it gets input forwarded after that, a view goes
//! out before the input forwarded under it, and so on, as in the model
//! check. The lock is held for microseconds and never across a wait, so
//! input callbacks can take it (D3).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, MutexGuard};

use crate::focus::{Action, Entry, Event, Machine, Mode, Node, Pointer, Route, View};
use crate::input;

/// Work for the board thread, in the order the machine decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// Input for the other computer: message type and payload.
    Input(u8, [u8; 5]),
    View(View, Option<Entry>),
    CloseGate(u8),
    OpenGate,
    /// MSG_KEY_STATE payload.
    KeyState([u8; 8]),
    StartSettle,
    Pointer(Pointer),
    /// The view or the mode changed (for the log).
    Changed(View, Mode),
    /// A switch towards the other computer was ignored: it cannot be
    /// reached (link or board down).
    Ignored,
}

pub struct Hub {
    inner: Mutex<Inner>,
}

struct Inner {
    machine: Machine,
    out: Sender<Out>,
}

impl Hub {
    /// A hub for a daemon that just started, and the queue for the board
    /// thread. The board is not open yet: it arrives as `BoardBack`.
    pub fn new(me: Node) -> (Hub, Receiver<Out>) {
        let (out, rx) = mpsc::channel();
        let (machine, actions) = Machine::new(me, false, false);
        debug_assert!(actions.is_empty(), "nothing to do without a board");
        (Hub { inner: Mutex::new(Inner { machine, out }) }, rx)
    }

    pub fn lock(&self) -> Guard<'_> {
        // A panic ends the process (panic = "abort"), so the lock is never
        // left poisoned by a half-done change.
        Guard(self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn handle(&self, event: Event) {
        self.lock().handle(event);
    }
}

/// The machine, locked. Everything done through it lands in the queue in
/// this order.
pub struct Guard<'a>(MutexGuard<'a, Inner>);

impl Guard<'_> {
    pub fn handle(&mut self, event: Event) {
        let inner = &mut *self.0;
        let before = (inner.machine.view(), inner.machine.mode());
        for action in inner.machine.handle(event) {
            let out = match action {
                Action::SendView(view, entry) => Out::View(view, entry),
                Action::CloseGate(n) => Out::CloseGate(n),
                Action::OpenGate => Out::OpenGate,
                Action::StartSettle => Out::StartSettle,
                // The mode says whether input is forwarded; the log shows it.
                Action::Forwarding(_) => continue,
                // Taken now, so that it matches the input queued before it.
                Action::SendKeyState => match input::key_state(inner.machine.forwarded_held()) {
                    Some(state) => Out::KeyState(state),
                    None => continue,
                },
                Action::Pointer(p) => Out::Pointer(p),
            };
            let _ = inner.out.send(out);
        }
        let after = (inner.machine.view(), inner.machine.mode());
        if after != before {
            let _ = inner.out.send(Out::Changed(after.0, after.1));
        } else if matches!(event, Event::Hotkey | Event::EdgePushed(_))
            && matches!(after.1, Mode::Split | Mode::Focused)
        {
            let _ = inner.out.send(Out::Ignored);
        }
    }

    pub fn mode(&self) -> Mode {
        self.0.machine.mode()
    }

    pub fn route_press(&mut self, id: u32) -> Route {
        self.0.machine.route_press(id)
    }

    pub fn route_release(&mut self, id: u32) -> Route {
        self.0.machine.route_release(id)
    }

    pub fn route_held(&self, id: u32) -> Route {
        self.0.machine.route_held(id)
    }

    pub fn route_motion(&self) -> Route {
        self.0.machine.route_motion()
    }

    /// Queues input for the other computer; only for input the machine
    /// routed `Forward`.
    pub fn forward(&self, msg_type: u8, payload: [u8; 5]) {
        let _ = self.0.out.send(Out::Input(msg_type, payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::msg;

    fn drain(rx: &Receiver<Out>) -> Vec<Out> {
        rx.try_iter().collect()
    }

    #[test]
    fn the_queue_keeps_the_machines_order() {
        let (hub, rx) = Hub::new(Node::Pc);
        hub.handle(Event::BoardBack);
        hub.handle(Event::LinkUp);
        drain(&rx);

        // Hotkey on the PC: close the gate, then forward only after the
        // board confirmed and the settle time passed.
        let mut g = hub.lock();
        g.handle(Event::Hotkey);
        assert_eq!(g.route_press(0x04), Route::Drop, "closing: own input dropped");
        g.handle(Event::GateClosed(1));
        g.handle(Event::SettleDone);
        assert_eq!(g.route_press(0x05), Route::Forward);
        g.forward(msg::KEY_DOWN, [0x05, 0, 0, 0, 0]);
        g.handle(Event::KeepaliveDue);
        drop(g);

        let out = drain(&rx);
        let close = out.iter().position(|o| matches!(o, Out::CloseGate(1))).expect("gate closed");
        let settle = out.iter().position(|o| *o == Out::StartSettle).expect("settle started");
        let input = out.iter().position(|o| matches!(o, Out::Input(msg::KEY_DOWN, _))).expect("input forwarded");
        assert!(close < settle && settle < input);
        // 0x04 was dropped, not forwarded: the key state lists only 0x05.
        assert_eq!(out.last(), Some(&Out::KeyState([0, 0, 0x05, 0, 0, 0, 0, 0])));
    }

    #[test]
    fn an_unreachable_switch_is_reported() {
        let (hub, rx) = Hub::new(Node::Mac);
        hub.handle(Event::BoardBack); // no link
        drain(&rx);
        hub.handle(Event::Hotkey);
        assert_eq!(drain(&rx), vec![Out::Ignored]);
    }
}
