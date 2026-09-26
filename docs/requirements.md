# Omni-KVM requirements

What "working correctly" means, written down before the design. The focus
state machine, the firmware changes and the automated checks all refer to
these rules by their IDs (B1, S3, F6, ...).

Status: agreed with the project owner on 2026-09-26, after Phase 4 testing
showed that fixing problems one at a time kept breaking something else
(see [Why this document exists](#why-this-document-exists)). Nothing was
built or flashed until the design had been checked against these rules
(`docs/focus-design.md`); firmware 0.5.3 and daemon 0.2.0 implement it and
passed acceptance testing on 2026-09-26 (`docs/acceptance-tests.md`).

## Terms

- **Focus**: the computer that keyboard and mouse input goes to.
- **Shared mode**: the focus is on one computer (the *focused* one); the
  other is *unfocused*.
- **Split mode**: no shared focus; each computer's own devices control
  only that computer. This is how the two computers behave with no
  Omni-KVM at all.
- **Board input**: input a board types into its own computer on behalf of
  the other computer (its USB keyboard and mouse).
- **Input gate**: a board's rule for whether it types the other
  computer's input into its own computer (F6).
- **T_link**: time to notice that the radio link is gone: **1 s**.
- **Keepalive**: the regular message each daemon sends the other, at
  least once a second (`MSG_VIEW`).

The layout is fixed for now: the Windows PC on the left, the Mac on the
right. The pointer crosses at the PC's right edge and the Mac's left edge.

## Behaviour the user sees (B)

- **B1. One focus, all devices.** In shared mode, all keyboards, mice and
  trackpads of both computers go to the focused computer, and only there.
  The unfocused computer receives no input at all, not even from its own
  devices. Example: with the focus on the PC, typing a consonant on the
  PC keyboard and a vowel on the MacBook keyboard composes one Hangul
  syllable on the PC.
- **B2. Split mode** when both daemons start, and after the link is lost
  and when it comes back. Shared mode starts with the user's first
  switch, so input never moves to the other computer by itself. A single
  daemon that restarts while the other keeps running (a crash and
  automatic restart, say) rejoins the current state instead (S9).
- **B3. Switching.**
  - (a) Push the pointer past the focused screen's facing edge (PC: right,
    Mac: left) by the edge resistance, with any device of either computer.
  - (b) The switch hotkey, on either keyboard (see [Hotkeys](#hotkeys-h)).
  - In split mode: push past a computer's own facing edge with its own
    devices, or press the hotkey on its keyboard; the focus goes to the
    other computer.
- **B4. Pointer positions.**
  - The unfocused computer's pointer stays exactly where it was when that
    computer lost the focus. On the Mac it is also hidden.
  - After an edge switch, the pointer enters the new screen just inside
    the facing edge, at the same fraction of the screen height.
  - After a hotkey switch, the new screen's pointer resumes where it had
    stopped.
  - When the link is lost, each pointer resumes where it had stopped
    (this replaced an earlier plan to move it to the centre of the main
    display).
- **B5. No stuck keys.** After any switch, link loss, restart or crash, no
  key or mouse button is left held down on either computer.
- **B6. Keystrokes arrive once, in order.** None are lost, doubled or
  reordered, with two stated exceptions:
  - keys pressed on the two keyboards within a few milliseconds of each
    other may arrive in either order (at human alternating speed the
    order holds);
  - keys pressed during the few milliseconds of a focus change may be
    dropped.
  Mouse movement that would arrive late is dropped instead.
- **B7. Latency** stays in single-digit milliseconds (target: 7 ms).
- **B8. One side without its daemon.** With only the board plugged in (at
  the login screen before the daemon starts, on a computer where it
  cannot be installed, or while it restarts), the other computer can
  still take that computer over with the hotkey, as a plain USB keyboard
  and mouse, and come back with the hotkey. Edge switching, pointer
  placement and sharing that computer's own devices need its daemon.
- **B9. Notifications** when the link is lost or restored and when the
  other daemon stops (as planned in `architecture.md`).
- **B10. Keys by position.** The Windows key and Command are the same key,
  as are Alt and Option. There is no Ctrl/Command swap.
- **B11. Trackpad scrolling** on the other computer goes the same way as
  on the Mac. Today this is fixed to the owner's setup (natural scrolling
  off, trackpad flipped back by LinearMouse); it becomes a setting later.

## Hotkeys (H)

- **H1. Switch hotkey: Win+Esc on the PC, Command+Esc on the Mac.** Both
  are the same USB keys (GUI + Escape). It triggers when Escape goes down
  while GUI is held and no other key was pressed since GUI went down, so
  cancelling a Cmd+Tab or Win+Tab switcher with Esc does not count.
- **H2.** The PC keeps **Scroll Lock** too (Fn+Home on the owner's
  keyboard).
- **H3. Each daemon watches its own keyboard, and its computer decides.**
  If the focus is on the other computer, the hotkey brings it here; if
  it is here, or in split mode, the hotkey moves it to the other
  computer. So one keyboard alone can switch both ways, and it works even
  when the other daemon is not running (B8).
- **H4. No side effects.** By default the key that completes the hotkey
  (Escape, Scroll Lock) reaches no computer, so it cannot act as a plain
  Esc in an app. The modifier pressed first (Win, Command) goes out
  before anyone can know Escape will follow, and is released at once;
  that must not open the Windows Start menu (which Windows opens when it
  sees the Windows key pressed and released alone). Built as: a board that
  lets go of a GUI key on its own presses Ctrl first, and the Windows
  daemon taps the unassigned virtual key 0xE8 whenever it keeps a key
  press from Windows while a Windows key is held (`docs/focus-design.md`,
  section 6); acceptance test 4 passed.
- **H5. Later:** hotkeys become configurable, with a left-/right-handed
  choice.
- **H6. Setting "pass the hotkey on"** (default: off). When on, the hotkey
  still switches, and its keys are also delivered where they were going:
  for example the PC's Scroll Lock reaches the Mac (as F14) while the
  focus is on the Mac.

## Focus rules (S)

- **S1. Agreement.** Normally both daemons see the focus in the same place
  (PC, Mac, or split).
- **S2. Recovery.** Whatever made them disagree (a message lost, doubled or
  late, two switches at once, a restart), they agree again within about
  one keepalive interval, as long as the link and both daemons are up.
- **S3. No echo.** Input a board typed into its computer is never sent back
  to the other computer. Input never circles between the two, in any
  state, including while they disagree.
- **S4. One place per input.** A single key press or movement never reaches
  both computers. While they disagree it may reach neither.
- **S5. Swallowing.** A daemon hides its own computer's input from that
  computer only while it sees the focus on the other computer.
- **S6. Who decides.** An edge switch is decided by the computer whose
  screen the pointer is pushed out of: the focused one in shared mode,
  each for itself in split mode. A hotkey switch is decided by the
  computer whose keyboard was used (H3). If two switches are decided at
  the same moment, the PC's wins.
- **S7. Link lost or board unplugged:** both computers are in split mode
  within T_link.
- **S8. The other daemon stops** (no message for 3 s): the running side goes
  to split mode. The stopped side's own devices work normally again (its
  input capture is gone with it), and its board accepts the other
  computer's input again (B8).
- **S9. Restarts.** After either daemon or board restarts, the two agree
  within about one keepalive interval once the link is back. A daemon
  that restarts within the 3 s of S8 rejoins the current state; after a
  longer absence the other side is already in split mode (S8).
- **S10. There is always a way back.**
  - The hotkey on the user's own keyboard (H3).
  - Automatic split mode when the link or the other daemon fails.
  - Last resort: unplugging a board gives split mode within T_link.

## Boards (F)

- **F1.** The main loop never stalls for more than 5 ms per pass. Every call
  that can wait (USB writes, HID reports, log output) is listed with its
  longest possible wait.
- **F2.** Every buffer has a fixed size and a rule for when it is full:
  hold the sender back, or drop whole packets. Drops are counted.
- **F3.** Nothing stays in a buffer forever: every item is sent, given up
  on, or dropped.
- **F4.** After bytes from the computer are lost or corrupted on USB, packet
  boundaries are found again within two packets.
- **F5.** Data for the computer always gets out while the computer reads it.
- **F6. Input gate.** A board types the other computer's input into its own
  computer only while its computer is focused, or while its own daemon is
  not connected. When the gate closes, every key and button it holds down
  is released and keyboard reports still waiting are dropped.
- **F7. Gate order.** Input sent after a focus change is handled by the new
  focus: the gate never opens late (dropping keys that belong to the new
  focus) or closes late (typing input the daemon then sends back, an
  echo).
- **F8.** Keys and buttons held on this computer are released when the link
  is lost, when the other board restarts, and when the other daemon stops
  (the other board tells this one that its computer disconnected).
- **F9.** A change of link state is reported to the daemon at once, not
  only when it asks.
- **F10.** Security rules stay: every radio packet is encrypted and
  authenticated, and replays are rejected.

## Daemons (D)

- **D1.** One daemon per computer.
- **D2.** Never half-dead: if part of it fails, the whole process ends, and
  the operating system removes its input capture.
- **D3.** Input-capture callbacks return quickly and never wait. Moving or
  hiding the pointer, and talking to the board, happen elsewhere.
- **D4.** If the operating system cuts off input capture, the daemon notices
  and goes to a safe state.
- **D5.** The daemon never records what is typed.
- **D6.** It reconnects by itself when its board comes back.
- **D7.** A key's release goes where its press went, even if the focus
  changed in between.

## Platform (P)

- **P1. Know the platform first.** Every design or code decision that
  depends on the board, the chip or the software under the firmware
  (Arduino core, ESP-IDF, TinyUSB) rests on a fact written in
  [platform.md](platform.md) with its source: an official document, a file
  of the toolchain as installed, or a dated measurement. A fact not known
  yet is measured before it is relied on.
- **P2.** Known defects underneath the firmware are listed there with how
  the firmware handles each, and the handling is tested.
- **P3.** The longest wait of every blocking call the firmware makes
  (F1) is taken from there.

Added 2026-09-26, after the board kept stopping under load (A5) because
the USB driver underneath lays out its FIFOs wrongly, which nobody had
checked before building on it.

## Failure model

The design is checked against all of these, alone and combined:

- **Messages:** lost, doubled, delayed. Within one direction they stay in
  order but some may be missing; between the two directions there is no
  order.
- **People:** using both computers' devices at once, typing during a
  switch, switching with a key or button held (including mid-drag),
  switching from both sides at once.
- **Equipment:** link lost and restored, board reset or unplugged, daemon
  killed, restarted or not running.
- **Operating system:** input capture cut off, lock screen or password
  entry (secure input), sleep.

## Assumptions to check by experiment (A)

- **A1.** A background program can keep the Mac pointer hidden reliably on
  macOS 27 ("SetsCursorInBackground", seen working once) while putting
  it back after every movement.
  - Seen working (2026-09-26): acceptance tests 3 and 7b.
- **A2.** Two keyboards typing into one computer: Hangul composes correctly
  on Windows and macOS, and a modifier held on one keyboard applies to
  keys on the other.
  - Result (2026-09-26, `tools/experiments/two_keyboards.py`): Hangul
    composes on both, whichever keyboard types which letter. A Shift held
    by the board applies to the other keyboard's keys **on Windows but
    not on macOS** (ㄱ instead of ㄲ): macOS keeps modifier state per
    keyboard.
  - Decided (2026-09-26): the Mac daemon adds the modifiers held on every
    keyboard to input passed to the Mac (its event tap changes the
    events' flags). Checked: acceptance test 5 passed (2026-09-26).
- **A3.** On Windows, mouse movement typed in by the board is visible to the
  daemon (Raw Input), so edge pushes with the Mac's trackpad count.
  - Seen (2026-09-25): during Phase 4 testing the PC daemon took the
    Mac trackpad's movement, typed in by the board, for a push past the
    PC's right edge.
- **A4.** The Mac event tap was switched off because the tap callback moved
  the pointer (hypothesis); doing that on a worker thread stops it.
  - Not seen since daemon 0.2.0: the daemon logs every time macOS switches
    its tap off (and switches it back on), and its log from 2026-09-26,
    acceptance and remote tests with hundreds of focus switches included,
    has no such line.
- **A5.** The board stopped sending to its computer because data was left in
  the USB transmit buffer without a flush (hypothesis). Reproduce with the
  old firmware under load; check the new firmware does not do it.
  - Reproduced (2026-09-26): with the PC sending ~620 packets/s and asking
    for stats every 50 ms, the board kept answering for 60 s; with the
    other board also relaying 100 messages/s to it, it stopped answering
    after about 3 s and never recovered (only a power cycle helped).
  - The hypothesis was wrong (2026-09-26): firmware 0.5.0, which flushes
    regularly, stopped too, after 0.5 s. It reproduces with one board
    alone (no radio), sending ~930 packets/s and asking for stats 120
    times a second: stopped after 3.5 s and 13 s (once not within 20 s).
    Diagnostic reports from the stuck board showed its serial IN endpoint
    holding a packet it never sent; a dump of the USB core's registers
    showed why: the Arduino core's USB driver places the transmit FIFOs in
    use outside the FIFO memory, over the receive FIFO (details in
    `firmware/include/usb_guard.h` and `shared/protocol.md`,
    MSG_BOARD_REPORT). Firmware 0.5.1 lays the FIFOs out itself.
  - Checked (2026-09-26), same load, one board: no stop in 600 s
    (67,637 replies, longest gap 23 ms). Both boards, as first reproduced
    (the other board relaying 100 messages/s to it, 20 stats requests a
    second): no stop in 300 s (longest gap 0.5 s).
- **A6.** The longest stall HID reports and log output can cause on a board.
- **A7.** Hotkey side effects: does Win+Esc or Command+Esc reach the app in
  front as Esc? Does the Start menu open after the hotkey? Is Command+Esc
  unused on macOS 27?
  - Result (2026-09-26, nothing intercepting): on Windows, a browser in
    full screen ignores Win+Esc, but the Start menu and a game (Wuthering
    Waves) act on it as Esc. So the daemon must keep the Escape from
    apps (H4). On macOS, Command+Esc seems not to act as Esc (partly
    tested).
- **A8.** On Windows, an app that reads the keyboard and mouse directly (Raw
  Input, common in games) may still receive input the daemon's hooks
  swallow. Then keeping the hotkey's Escape from such an app is not
  possible (A7), and while the PC is unfocused such an app would still
  react to the PC's own devices (against S4 for that app). The daemon
  itself receives Raw Input for mouse movement its hook swallowed, which
  suggests this; similar tools are known to have the same limitation.
  Check with a game.

## Decided questions

- **O1. Who decides a hotkey switch** (decided 2026-09-26): the computer
  whose keyboard was used (H3, S6). With the focused computer deciding,
  a hotkey pressed while the focused computer has no daemon (B8) would
  have had no one to decide it, and no way back but unplugging a board.

## Why this document exists

Each problem met while testing Phase 4 broke a rule that had never been
written down:

| Problem seen | Rule now covering it |
|---|---|
| Both daemons believed the other one led; nobody could switch | S2 |
| After a daemon restart, the PC stayed "controlled" | S9 |
| Movement arriving late from the other computer counted as an edge push | S3, S6, F7 |
| One malformed packet blocked a board's send queue for good | F3 |
| A board stopped sending to its computer | F5, P1 |
| The Mac pointer moved along while the Mac controlled the PC | A1, B4 |
| Two Mac daemons ran at once | D1 |
