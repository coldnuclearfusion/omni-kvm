# Omni-KVM daemon

Host-side program (Rust). It talks to the board plugged into this computer
over USB (the board's "USB" port, a CDC serial port) and, on Windows,
captures the keyboard and mouse to control the other computer.

## Build and run (Windows)

```
cargo build --release
target\release\omni-kvm.exe          # finds the board by itself
target\release\omni-kvm.exe COM9     # or name the port, if several boards are plugged in
```

## Using it (Phase 3)

- **To the other computer**: push the pointer past the **right** edge of the
  screen and keep pushing a little (virtual resistance), or press
  **Scroll Lock** (on many keyboards: `Fn` + a key marked "ScrLk", e.g. Home).
  The other computer's pointer jumps to its left edge.
- **Back to Windows**: **Scroll Lock**.
- While the other computer is in control, Windows sees no keyboard or mouse
  input: it all goes over the radio. The Windows key acts as Command on a Mac,
  Alt as Option.
- If the radio link drops, input returns to Windows automatically.
- The other computer needs no software for this: its board is a plain USB
  keyboard and mouse to it.

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
- The daemon never records what is typed.

## Current limitations

- Coming back needs the hotkey: without software on the Mac, the daemon
  cannot know where the Mac's pointer is (Phase 4 adds a Mac daemon).
- The jump to the Mac's left edge is a plain mouse movement and
  occasionally does not arrive.
- Windows does not send input from elevated (administrator) windows to a
  non-elevated program's hooks, and nothing reaches them on the secure
  desktop (UAC prompts, Ctrl+Alt+Del screen).
- Scroll Lock is taken over as the hotkey.
- Console program for now; no tray icon or auto-start yet.
