# Acceptance tests

Tests run with both computers, by hand or, where a run says so, driven
by software, after the automated checks (daemon unit tests and the focus
model check) pass. Each run records the
firmware and daemon versions, what was done, what was seen, and what was
not tested.

## 2026-09-26 — firmware 0.5.2-dev, daemon 0.2.0

Setup: board 1 on the Windows PC (native USB port), board 2 on the MacBook
Air (native USB port); both daemons running; Mac input source Korean
2-Set.

| # | Test | Requirement | Result |
|---|---|---|---|
| 1 | Both daemons start in split mode; each computer's own devices control only it | B2 | Pass |
| 2 | Push past the PC's right edge with the PC mouse: focus to the Mac, pointer enters at the Mac's left edge at the same height; all devices of both computers drive the Mac | B1, B3, B4 | Pass |
| 3 | Push past the Mac's left edge (any device): focus back to the PC, pointer at the PC's right edge; Mac pointer hidden | B3, B4 | Pass |
| 4 | Hotkeys: Win+Esc and Scroll Lock on the PC, Command+Esc on the Mac, both ways; Escape not delivered; no Start menu; pointer resumes where it stopped | H1–H4, B4 | Pass |
| 5 | Hangul across keyboards (consonant on one, vowel on the other; Shift on one keyboard, letter on the other), focus on either computer | B1, A2 | Pass (the Mac now adds every keyboard's modifiers) |
| 6 | Key held down while switching (edge and hotkey): nothing left held | B5, D7 | Pass |
| 7a | Mac daemon killed and restarted with the focus on the Mac: rejoins, PC input keeps driving the Mac, switching back works | S9, R1 | Pass after a fix: the first run rejoined but believed the link was down, so the Mac could not switch (the board's link report was discarded when the port opened; the daemon now takes the link state from its first stats reply) |
| 7b | Same with the focus on the PC: Mac pointer shows briefly, hides again, Mac devices drive the PC again | S9, B4 | Pass |
| 8 | Mac board unplugged and plugged back with the focus on the Mac: split mode at once, still split after replugging | S7, B2 | Pass |
| 8b | Switching after the replug | S10 | **Fail**: the Mac board's serial link to its computer was stuck (USB driver defect, docs/platform.md K7), so the Mac daemon could not talk to its board; fixed in 0.5.3 (below) |

Not tested:

- Keys or buttons left held on the other computer when a daemon quits
  or crashes while one is held (F8, `MSG_HOST_GONE`), by hand. The
  deeper model check found that a lost `MSG_HOST_GONE` left such a key
  held; fixed in firmware 0.5.4 (`docs/focus-design.md`, found by the
  automated check, 4).
- Link loss by distance or interference (only by unplugging).
- The Windows PC or the Mac going to sleep and waking up.
- A game that reads the keyboard and mouse directly (A8).

## 2026-09-26 evening — firmware 0.5.3-dev, daemon 0.2.0, run remotely

Setup as above, plus each board's "UART" port on the same computer, and
nobody at the computers: every action came from software.

- USB events came from test commands on the boards' UART ports (firmware
  `main.cpp`, "commands on the UART port"; `tools/usb_tests/uart_console.py`): `usb reconnect` disconnects
  from USB and connects again, as the USB watchdog does, instead of
  unplugging; `usb k6` imitates the USB driver's own reset
  (docs/platform.md K6) while a serial transfer is under way.
- The Mac's hotkey and pointer came from a small program in the Mac's
  Terminal that posts keyboard and pointer events at the HID level
  (`tools/usb_tests/input_driver.swift`). The Mac daemon cannot tell them from
  its own keyboard and trackpad. The Windows daemon ignores injected
  input by design, so nothing was pressed on the PC.
- Results were read from both daemons' logs and both boards' UART logs.

| # | Test | Requirement | Result |
|---|---|---|---|
| E1 | Command+Esc on the Mac, twice: focus to the PC, then back to the Mac; both daemons agree on every view | H3, B1 | Pass |
| E1b | Push past the Mac's left edge: focus to the PC; Command+Esc back | B3 | Pass |
| E2 | With the focus on the Mac, board 2 reconnects its USB (as unplugging and replugging, test 8): split mode at once, still split after the board is back (~2 s); then Command+Esc switches to the PC and back | S7, S10 | Pass |
| E3 | With the focus on the Mac, board 2's driver resets itself (imitated K6) while the Mac daemon has the port open: the Mac did not reset the device; the USB watchdog reconnected it after 1.5 s of a stuck serial endpoint; split mode meanwhile; then switching both ways | S7, S10 | Pass |
| E4a | Same as E2 with board 1 (Windows) | S7, S10 | Pass |
| E4b | Same as E3 with board 1: Windows reset the device within a second (keeping the COM port), the firmware stopped the transfer left over from before the reset, the PC daemon reconnected; then switching both ways | S7, S10 | Pass |

Soak runs the same evening (`tools/usb_tests/soak.py`): the Mac hotkey
twice per cycle, each switch checked on both daemons, and USB events on
the boards taking turns (reconnect, imitated driver reset):

| Firmware, daemon | Length | Switches | USB events | Result |
|---|---|---|---|---|
| 0.5.3-dev, 0.2.0 | 8 min slow + 60 min fast | 16 + 662 | 27 | All passed. Seen meanwhile on the boards: 3 real driver resets (K6), 8 watchdog reconnections, 10 forgotten transfers stopped, no old data sent. |
| 0.5.4-dev, 0.2.0 (cleaned up) | 5 min | 52 | 4 | 51 passed. **Once, one hotkey switched twice**: to the Mac (view 47) and at once back to the PC (view 48), both decided by the Mac. Right after view 47 the PC's board sent about 9 more acknowledged packets than usual, so the PC most likely forwarded key events to the Mac. Most likely real input: the project owner came home and moved the mouse at about that time, and the Mac pointer was found moved (a push past the Mac's left edge, by any device, moves the focus to the PC). Not certain, because the daemons do not log why a switch was decided; such a log would settle cases like this. |

MSG_HOST_GONE repeated (firmware 0.5.4-dev), same evening: with the focus
on the PC and a Shift held on the Mac forwarded to the PC, the Mac daemon
was stopped. The PC's board let go of the Shift 1 s later, once; the Mac's
board then repeated `MSG_HOST_GONE` once a second (9 in 9 s, 34 in 34 s)
without further effect, and stopped when the Mac daemon was back.
