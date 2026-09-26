# Omni-KVM daemon

Host-side program (Rust), one on each computer: Windows (the PC) and macOS
(the Mac). It talks to the board plugged into its computer over USB (the
board's "USB" port, a CDC serial port), captures its computer's keyboard
and mouse or trackpad, and with the other daemon keeps one shared focus:
which computer all the keyboards and mice drive. The design is in
`docs/focus-design.md`, the requirements in `docs/requirements.md`.

## Build and run (Windows)

```
cargo build --release
target\release\omni-kvm.exe          # finds the board by itself
target\release\omni-kvm.exe COM9     # or name the port, if several boards are plugged in
```

## Build and run (macOS)

Needs the Xcode command line tools (their linker) and Rust from
[rustup](https://rustup.rs).

```
cargo build --release
./target/release/omni-kvm                                  # finds the board by itself
./target/release/omni-kvm /dev/cu.usbmodemE8F60A8DB00C2    # or name the port
```

The daemon reads the keyboard and trackpad with an event tap, which needs
the **Accessibility** permission (and **Input Monitoring**, where System
Settings lists it) for the app that starts it, e.g. Terminal. Until it has
them, it says so and waits.

macOS lists each serial device twice, as `/dev/cu.*` and `/dev/tty.*`;
the daemon uses the `cu.` one. The name ends with the board's MAC address.

On both systems it finds the board by its USB vendor ID (0x303A) and looks
again every 2 s while there is none; with two boards plugged in, name the
port. Only one daemon runs at a time on a computer (a lock file in the
temporary directory).

## Using it

Both daemons start in **split mode**: each computer's own keyboard and
mouse drive only that computer.

- **Focus on the other computer**: on the PC, push the pointer past the
  **right** edge and keep pushing a little (virtual resistance), or press
  **Win+Esc** or **Scroll Lock**; on the Mac, push past the **left** edge,
  or press **Command+Esc**. From then on the keyboards and mice of *both*
  computers drive the focused one. The pointer enters at the facing edge,
  at the same height.
- **Back**: the same hotkey on either computer, or pushing past the
  focused Mac's left edge. The hotkey is decided by the computer it is
  pressed on, so it works whatever the other daemon is doing, even if it
  is not running.
- The completing key of a hotkey (Escape, Scroll Lock) goes nowhere; the
  Windows key or Command pressed with it goes where the focus was. No
  Start menu opens (`docs/focus-design.md`, section 6).
- A key or button held while the focus moves is released where it was
  pressed, so nothing stays stuck.
- If the radio link drops, the other board is gone, or the other daemon
  stops answering for 3 s, both computers go back to split mode. A board
  whose daemon is gone acts as a plain USB keyboard and mouse for the other
  computer.
- The Windows key acts as Command on the Mac, Alt as Option.

**Emergency exit.** On the Mac, Command+Esc always brings the focus back
(and if the Mac daemon itself stops answering, macOS switches its event
tap off and input goes straight to the Mac). On Windows, Win+Esc or Scroll
Lock does the same; Ctrl+Alt+Del cannot be captured by any program, and
from there Task Manager can end `omni-kvm.exe`, which removes its hooks at
once.

## How it works

| Module | Role |
|---|---|
| `main.rs` | Board connection, timers (keepalive, link stats, timeouts), log lines |
| `focus.rs` | The focus state machine, model-checked in `focus/check.rs` (`cargo test`) |
| `hub.rs` | Feeds events to the machine and carries out its outputs in order |
| `hotkey.rs` | Recognizes GUI+Escape (Scroll Lock is checked by the Windows capture) |
| `input_windows.rs` | Low-level keyboard and mouse hooks plus Raw Input |
| `input_macos.rs` | Quartz event tap at the HID level |
| `screen_windows.rs`, `screen_macos.rs` | Screen edges, pointer entry, hiding the Mac pointer |
| `keymap.rs`, `keymap_macos.rs` | Keys to USB HID usages |
| `input.rs` | Shared input helpers (held keys, modifier bits) |
| `board.rs` | Finding and opening the board, assembling packets |
| `protocol.rs` | Message layouts (`shared/protocol.md`) |
| `log.rs` | Log lines, written on their own thread |

- Every input event is routed by the focus state machine: passed to this
  computer, forwarded to the board (which sends it over the radio to the
  other board, which types it in), or dropped.
- Keys are identified by physical position (scan code on Windows, key code
  on macOS), independent of the layout or IME, and translated to USB HID
  usages. Forwarded mouse movement on Windows comes from Raw Input, before
  Windows' pointer acceleration, since the Mac applies its own.
- The two daemons keep the focus in step by sending each other their view
  of it (`MSG_VIEW`) through the boards, once a second and on every change.
  Before a daemon forwards its own input it closes its board's input gate,
  so nothing the other computer typed in comes back round.
- The Mac adds the modifiers held on every keyboard to what it passes on
  (macOS keeps them per keyboard), so Shift on one keyboard and a letter on
  the other make a capital (Hangul too).
- The daemon never records what is typed.

## Log

Lines go to standard output, without timestamps: `[focus]` changes of the
shared focus, `[link]` radio link state and, every 10 s, link statistics,
`[peer]` whether the other daemon is answering, `[input]` capture notes,
and plain lines when the board is found or lost.

## Current limitations

- The Mac is assumed to be to the right of the PC. On the Mac, only the
  outer edges of the whole display arrangement count.
- Trackpad scrolling is inverted when forwarded (`INVERT_TRACKPAD_SCROLL`
  in `input_macos.rs`), to match the developer's setup.
- Windows does not send input from elevated (administrator) windows to a
  non-elevated program's hooks, and nothing reaches them on the secure
  desktop (UAC prompts, the Ctrl+Alt+Del screen). macOS hides keystrokes
  from every event tap while a password field or other secure input is
  active.
- Console programs for now; no tray icon, settings or auto-start yet.
