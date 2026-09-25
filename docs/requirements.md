# Omni-KVM requirements

What "working correctly" means, written down before the design. The focus
state machine, the firmware changes and the automated checks all refer to
these rules by their IDs (B1, S3, F6, ...).

Status: agreed with the project owner on 2026-09-26, after Phase 4 testing
showed that fixing problems one at a time kept breaking something else
(see [Why this document exists](#why-this-document-exists)). Nothing is
built or flashed until the design has been checked against these rules.

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
  least once a second (today `MSG_EDGE_CONTACT`).

The layout is fixed for now: the Windows PC on the left, the Mac on the
right. The pointer crosses at the PC's right edge and the Mac's left edge.

## Behaviour the user sees (B)

- **B1. One focus, all devices.** In shared mode, all keyboards, mice and
  trackpads of both computers go to the focused computer, and only there.
  The unfocused computer receives no input at all, not even from its own
  devices. Example: with the focus on the PC, typing a consonant on the
  PC keyboard and a vowel on the MacBook keyboard composes one Hangul
  syllable on the PC.
- **B2. Split mode** at startup, and after the link is lost and when it
  comes back. Shared mode starts with the user's first switch, so input
  never moves to the other computer by itself.
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
    (this replaces "move it to the centre of the main display" in
    `architecture.md`).
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
- **B8. One side without its daemon.** With only the board plugged in, the
  other computer can still take that computer over with the hotkey, as a
  plain USB keyboard and mouse. Edge switching, pointer placement and
  sharing that computer's own devices need its daemon.
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
- **H3. Meaning: move the focus to the other computer.** In split mode, the
  focus goes to the computer opposite the keyboard used.
- **H4. No side effects.** The hotkey must not also act as a plain Esc in
  the app in front, nor open the Windows Start menu (which Windows opens
  when it sees the Windows key pressed and released alone). How this is
  done depends on experiment A7.
- **H5. Later:** hotkeys become configurable, with a left-/right-handed
  choice.

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
- **S6. Who decides.** In shared mode the focused computer decides every
  switch: edge pushes on its screen, and the hotkey pressed on either
  keyboard. The unfocused computer only forwards. In split mode each
  computer decides for itself; if both switch at the same moment, the PC
  wins. (See open question O1.)
- **S7. Link lost or board unplugged:** both computers are in split mode
  within T_link.
- **S8. The other daemon stops** (no message for 3 s): the running side goes
  to split mode. The stopped side's own devices work normally again (its
  input capture is gone with it), and its board accepts the other
  computer's input again (B8).
- **S9. Restarts.** After either daemon or board restarts, the two agree
  within about one keepalive interval once the link is back.
- **S10. There is always a way back.**
  - The hotkey on the user's own keyboard (see O1).
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
- **A2.** Two keyboards typing into one computer: Hangul composes correctly
  on Windows and macOS, and a modifier held on one keyboard applies to
  keys on the other.
- **A3.** On Windows, mouse movement typed in by the board is visible to the
  daemon (Raw Input), so edge pushes with the Mac's trackpad count.
- **A4.** The Mac event tap was switched off because the tap callback moved
  the pointer (hypothesis); doing that on a worker thread stops it.
- **A5.** The board stopped sending to its computer because data was left in
  the USB transmit buffer without a flush (hypothesis). Reproduce with the
  old firmware under load; check the new firmware does not do it.
- **A6.** The longest stall HID reports and log output can cause on a board.
- **A7.** Hotkey side effects: does Win+Esc or Command+Esc reach the app in
  front as Esc? Does the Start menu open after the hotkey? Is Command+Esc
  unused on macOS 27?
  - Preliminary (2026-09-26): a browser in F11 full screen, which leaves
    full screen when Esc is held, did not leave it when Win was held
    together with Esc.

## Open questions

- **O1. Who decides a hotkey switch.** S6 lets the focused computer decide,
  so a hotkey pressed on the unfocused keyboard is forwarded and decided
  there; if the focused daemon is not answering, the way back is the S8
  timeout. Letting the keyboard's own computer decide would work even
  then. To be settled together with the state machine.

## Why this document exists

Each problem met while testing Phase 4 broke a rule that had never been
written down:

| Problem seen | Rule now covering it |
|---|---|
| Both daemons believed the other one led; nobody could switch | S2 |
| After a daemon restart, the PC stayed "controlled" | S9 |
| Movement arriving late from the other computer counted as an edge push | S3, S6, F7 |
| One malformed packet blocked a board's send queue for good | F3 |
| A board stopped sending to its computer | F5 |
| The Mac pointer moved along while the Mac controlled the PC | A1, B4 |
| Two Mac daemons ran at once | D1 |
