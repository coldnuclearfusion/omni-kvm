# Omni-KVM daemon

Host-side program (Rust). It talks to the board plugged into this computer
over USB (the board's "USB" port, a CDC serial port). On Windows it
captures the keyboard and mouse to control the other computer. On macOS
it places the pointer where Windows hands it over and tells Windows when
the pointer touches a screen edge (Phase 4, step 2).

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

macOS lists each serial device twice, as `/dev/cu.*` and `/dev/tty.*`;
the daemon uses the `cu.` one. The name ends with the board's MAC address.
Reading and moving the pointer needs no macOS permission.

## Using it (Windows keyboard and mouse)

- **To the Mac**: push the pointer past the **right** edge of the screen and
  keep pushing a little (virtual resistance), or press **Scroll Lock** (on
  many keyboards: `Fn` + a key marked "ScrLk", e.g. Home). The Mac pointer
  appears near its left edge, at the same height.
- **Back to Windows**: push the Mac pointer past the Mac's **left** edge (same
  resistance), or press **Scroll Lock**. The Windows pointer appears near its
  right edge, at the height it left the Mac.
- While the Mac is in control, Windows sees no keyboard or mouse input: it
  all goes over the radio. The Windows key acts as Command on a Mac, Alt as
  Option.
- If the radio link drops, input returns to Windows automatically.
- Without the Mac daemon, the Mac still works as a plain USB keyboard and
  mouse, but it can neither place its pointer nor report its edges: the
  pointer is pushed to the Mac's left edge at whatever height it was, and
  only Scroll Lock comes back. The Windows daemon says which case applies
  (`[peer] the other computer's daemon is running`).

**Emergency exit**: Ctrl+Alt+Del cannot be captured by any program; from
there, Task Manager can end `omni-kvm.exe`, which removes its hooks at once.

## How it works

- Low-level keyboard and mouse hooks (`WH_KEYBOARD_LL`, `WH_MOUSE_LL`) let
  input through in Local mode and swallow it in Remote mode.
- Keys are identified by scan code (physical position, independent of the
  layout or IME) and translated to USB HID usages (`src/keymap.rs`).
- Mouse movement comes from Raw Input: the mouse's own counts, before
  Windows pointer acceleration, since the receiving OS applies its own.
- Keys and buttons held during a switch are released on the side that no
  longer has control, so nothing stays stuck.
- The two daemons talk through the boards (`src/peer.rs`): `MSG_HANDOFF`
  says where the pointer enters, as a fraction of the screen height;
  `MSG_EDGE_CONTACT` says which edges the Mac pointer touches. Pushing past
  an edge is decided on Windows, the only side that sees the mouse's
  movement once the Mac pointer stops at the edge. See
  `shared/protocol.md`.
- The daemon never records what is typed.

## Current limitations

- The Mac is assumed to be to the right of Windows. On the Mac, only the
  outer edges of the whole display arrangement count.
- The Mac daemon checks the pointer every 10 ms and the board every 5 ms
  (polling), which costs a little battery; to be made event-driven.
- Windows does not send input from elevated (administrator) windows to a
  non-elevated program's hooks, and nothing reaches them on the secure
  desktop (UAC prompts, Ctrl+Alt+Del screen).
- Scroll Lock is taken over as the hotkey.
- Console programs for now; no tray icon or auto-start yet.
