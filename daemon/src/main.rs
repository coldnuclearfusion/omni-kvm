//! Omni-KVM host daemon.
//!
//! Phase 3, step 1: find the local board, then report its radio link
//! every two seconds. Input capture and forwarding come next.

mod board;
mod protocol;

use std::thread::sleep;
use std::time::Duration;

use board::Board;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

fn main() {
    println!("Omni-KVM daemon {}", env!("CARGO_PKG_VERSION"));
    // Optional argument: the board's COM port, needed only when several
    // boards are plugged into this computer (e.g. during development).
    let wanted_port = std::env::args().nth(1);

    loop {
        let port = match choose_board(wanted_port.as_deref()) {
            Some(p) => p,
            None => {
                sleep(POLL_INTERVAL);
                continue;
            }
        };
        match Board::open(&port) {
            Ok(mut board) => {
                println!("Connected to the board on {port}");
                report_until_error(&mut board);
                println!("Lost the board on {port}; waiting for it to come back...");
            }
            Err(e) => println!("Could not open {port}: {e}"),
        }
        sleep(POLL_INTERVAL);
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
            println!("Several boards found: {}. Pass the port to use, e.g. `omni-kvm COM9`.", list.join(", "));
            None
        }
    }
}

/// Prints the board's link state every POLL_INTERVAL until the board stops answering.
fn report_until_error(board: &mut Board) {
    loop {
        match board.request_stats(Duration::from_secs(1)) {
            Ok(s) => println!(
                "link {} | session {} | input from this PC {} (dropped {}) | from peer {} | \
                 frames {} acked {} failed {} | auth failures {} | replays {}",
                if s.link_up { "UP" } else { "DOWN" },
                s.session,
                s.host_received,
                s.host_dropped,
                s.radio_received,
                s.frames_sent,
                s.frames_acked,
                s.frames_failed,
                s.auth_failures,
                s.replays_dropped,
            ),
            Err(e) => {
                println!("Board error: {e}");
                return;
            }
        }
        sleep(POLL_INTERVAL);
    }
}
