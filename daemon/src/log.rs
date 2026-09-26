//! Log lines, printed by a thread of their own. Input callbacks must never
//! wait (D3), and printing can: a slow terminal, or a full pipe to `tee`.
//!
//! `say!` works like `println!`. Lines keep their order; ones still waiting
//! when the process ends are lost.

use std::io::Write;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Sender};
use std::thread;

pub fn line(text: String) {
    static LINES: OnceLock<Sender<String>> = OnceLock::new();
    let lines = LINES.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            let stdout = std::io::stdout();
            for text in rx {
                let mut out = stdout.lock();
                let _ = writeln!(out, "{text}");
                let _ = out.flush();
            }
        });
        tx
    });
    let _ = lines.send(text);
}

macro_rules! say {
    ($($arg:tt)*) => {
        $crate::log::line(format!($($arg)*))
    };
}
