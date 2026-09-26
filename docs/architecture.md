# Architecture

This document describes the overall design of Omni-KVM. It complements the project README with an expanded view of components, data flows, and the rationale behind major design decisions. The detailed rules live elsewhere: `docs/requirements.md` (rule IDs such as B1 or S7 below refer to it), `docs/focus-design.md` (the shared focus), `shared/protocol.md` (every message) and `docs/platform.md` (what the board and the software under the firmware can do).

## System overview

Omni-KVM is composed of four cooperating components: two firmware instances running on the ESP32-S3 devices, and two host daemons running on the connected computers.

```
       Computer A (Windows)               Computer B (macOS)
  ┌────────────────────────────┐    ┌────────────────────────────┐
  │  Host daemon A             │    │  Host daemon B             │
  │  • input capture           │    │  • input capture           │
  │  • focus state machine     │    │  • focus state machine     │
  │  • screen edges, pointer   │    │  • screen edges, pointer   │
  │  • switch hotkey           │    │  • switch hotkey           │
  └────────────┬───────────────┘    └────────────┬───────────────┘
               │ USB CDC                         │ USB CDC
               │ (serial)                        │ (serial)
  ┌────────────▼───────────────┐    ┌────────────▼───────────────┐
  │  Firmware A (ESP32-S3)     │    │  Firmware B (ESP32-S3)     │
  │  • USB composite device    │    │  • USB composite device    │
  │  • input gate              │    │  • input gate              │
  │  • ESP-NOW transport       │◄══►│  • ESP-NOW transport       │
  │  • heartbeat, sessions     │    │  • heartbeat, sessions     │
  │  • encryption              │    │  • encryption              │
  └────────────────────────────┘    └────────────────────────────┘
                ◄══ 2.4 GHz, ChaCha20-Poly1305 ══►
```

Both boards run the same firmware, which has no role of its own. The two daemons are built from the same source. Each daemon's role is fixed by the operating system it is built for: on Windows it is the PC, on the left; on macOS it is the Mac, on the right (`daemon/src/main.rs`). When two switches are decided at the same moment, the PC's wins (S6). Where the focus is, is decided at run time (see [Focus model](#focus-model)).

## Layered model

```
┌─────────────────────────────────────────────────────────────┐
│ L5 — UX            edge resistance, hotkey, pointer hiding  │ daemon
├─────────────────────────────────────────────────────────────┤
│ L4 — Application   input capture, focus state machine       │ daemon
├─────────────────────────────────────────────────────────────┤
│ L3 — Protocol      message format, sequencing, encryption   │ shared
├─────────────────────────────────────────────────────────────┤
│ L2 — Transport     ESP-NOW radio, retransmits, heartbeat    │ firmware
├─────────────────────────────────────────────────────────────┤
│ L1 — Physical I/O  USB HID, USB CDC, radio PHY              │ firmware
└─────────────────────────────────────────────────────────────┘
```

Planned for L5, not built yet: notifications (B9) and an input lock for full-screen apps (`MSG_LOCK`, reserved in `shared/protocol.md`).

Each layer depends only on the one below it. This makes individual layers testable in isolation: L1+L2 can be validated with a wired (UART) link before introducing the radio, L3 can be exercised with simulated packets, and L4+L5 can be developed against a stub firmware.

## Component responsibilities

### Firmware (ESP32-S3)

The firmware is intentionally minimal. It does not understand cursor positions or screen layouts, and it does not know where the focus is. Its only part in the focus is its input gate, which its daemon opens and closes. Its job is to be a reliable transport between the radio and the USB ports on each side.

**Owns:**
- USB composite device descriptor and enumeration
- Receiving input from the radio → typing it into its computer as HID reports, only while its input gate is open (F6)
- The input gate (`shared/protocol.md`, "Input gate"): its daemon closes it (command 0x09) and opens it (0x08). On closing, the board drops keyboard reports not sent yet, releases every key and button it holds for the other computer, and confirms (status 0x06) once those releases have reached its computer. While no daemon is connected, the gate is open (B8).
- Pacing HID reports: keyboard reports go out at least 10 ms apart, and modifier changes (Shift, Ctrl, …) are sent before the key they apply to. In Phase 1 testing, key reports sent ~1 ms apart came out garbled on Windows (dropped letters, Shift on the wrong keys); faster than 10 ms garbles Korean IME input on Windows (`docs/platform.md`, section 5). Mouse reports go out at once, or are dropped if USB is not ready.
- Receiving packets from its daemon over CDC → sealing and sending them over the radio (input and daemon-to-daemon messages), or acting on them (commands for the board itself)
- Relaying daemon-to-daemon messages between the two hosts without reading them: today `MSG_VIEW`; the lock messages are reserved, not implemented
- Reporting the radio link state to its daemon at once (status 0x05, F9); a restart of the other board is reported as the link going down, then up
- `MSG_HOST_GONE`: sent to the other board when its own daemon disconnects
- `MSG_KEY_STATE`: letting go of every key and button it holds for the other computer that the message does not list
- ESP-NOW peer setup, retransmits, and heartbeat
- Authenticated encryption (ChaCha20-Poly1305), per-connection session keys, and replay protection (sequence numbers)
- Local fail-safe: release every key and mouse button it holds for the other computer when the link drops (1 s without the other board, T_link), when the other board restarts, when its gate closes, and on `MSG_HOST_GONE` (F6, F8)
- USB guard (`firmware/src/usb_guard.cpp`): works around defects in the USB driver underneath (`docs/platform.md`, section 4). It lays out the transmit FIFOs itself, stops transfers a reset left behind, and reconnects USB when an endpoint stays stuck.
- Planned (Phase 6, not built yet): USB Mass Storage with installer payloads, and entry into an update mode for daemon-mediated firmware updates

**Does not own:**
- Cursor coordinates or screen edge detection
- Screen layout
- Where the focus is (it only keeps the gate its daemon sets)
- User-facing notifications
- Pairing UI (planned; the firmware will do only the cryptography)

### Host daemon

The daemon is where all user-facing logic lives. It is the only component that understands what it means for the cursor to "leave one screen and enter another".

**Owns:**
- Hooking the OS input stream: low-level keyboard and mouse hooks on Windows, a Quartz event tap on macOS. The focus state machine decides for every event whether it passes to this computer, is swallowed and forwarded, or is swallowed and dropped. Mouse deltas must be captured *before* the local OS applies pointer acceleration: the receiving OS accelerates the injected HID reports again, and forwarding already-accelerated movement would accelerate it twice. On Windows, the hooks decide what Windows sees, and movement is measured with Raw Input, which provides the un-accelerated counts (for forwarding and for pushes past the edge). See the batching note in `shared/protocol.md` for the measured effect of acceleration.
- Reading its own screen's edges (the virtual screen on Windows, the display arrangement on macOS). The layout is fixed for now: the PC on the left, the Mac on the right.
- Keeping its view of the focus in step with the other daemon: `MSG_VIEW`, relayed by the boards, every second and on every change (`docs/focus-design.md`, section 1)
- Deciding the switches made on this computer: a push past its facing edge (PC: right, Mac: left) by the edge resistance, with any device, and the switch hotkey on its keyboard (Win+Esc or Scroll Lock on the PC, Command+Esc on the Mac)
- Swallowing its own computer's input, and forwarding it through the board, only while the focus is on the other computer (S5); closing its board's input gate before it forwards, and opening it after it stops
- Placing the pointer (B4): after an edge switch, just inside the facing edge at the same fraction of the screen height; after a hotkey switch it resumes where it stopped. The Mac's pointer is hidden while the Mac is unfocused.
- Sending each key's release where its press went (D7), and `MSG_KEY_STATE` every second while it forwards
- Planned (not built yet): notifications when the link or the other daemon is lost or restored (B9); today it writes log lines
- Planned: detecting full-screen / exclusive-mode applications (lock signal)
- Planned (Phase 6): downloading firmware updates and passing them to the device
- Planned: persisting pairing configuration and user preferences (there is no pairing and there are no settings yet)

**Does not own:**
- Direct radio communication (always goes through firmware over CDC)
- Encryption keys: today it holds none (the key is compiled into the firmware). Planned: any key it stores goes to the OS keychain / DPAPI, never to a plaintext file.

## USB composite device

Each firmware exposes the following interfaces over a single USB connection (the board's "USB" port):

| Interface | Purpose |
|---|---|
| HID keyboard and mouse (one interface) | Inject keystrokes, pointer movement, buttons, scroll |
| CDC serial | Bidirectional control channel with the host daemon |
| Mass Storage | Planned (Phase 6), not built yet: read-only drive containing installers and README |

The HID interface works with the OS's built-in drivers — no installation is required for basic input to function. Planned: the Mass Storage interface will be the entry point for first-time setup: when the user plugs in a fresh device, a small drive named `OMNI-KVM` will appear in their file manager containing platform-specific installers.

The CDC serial channel is invisible to typical users and serves as the daemon's private communication path with the firmware. The board's other port, "UART", carries firmware uploads, the board's log and development commands; it is not needed in use.

## Focus model

Where input goes is the shared focus. The rules are in `docs/requirements.md` (B1–B5); the design is `docs/focus-design.md` (section 2 has the three modes). In short:

- **Split mode**: each computer's own keyboard and mouse drive only that computer, as with no Omni-KVM at all. Both daemons start in split mode, and go back to it when the link, a board or the other daemon is lost (B2, S7, S8).
- **Focus on the PC or on the Mac**: every keyboard and mouse of both computers drives the focused computer, and the unfocused one gets no input (B1; on Windows, apps that read devices directly may be an exception, A8). The unfocused daemon swallows its own computer's input and forwards it through the boards; the focused daemon forwards nothing. The unfocused computer's pointer stays where it was (B4).
- **Switching**: push the pointer past the facing screen edge (the PC's right edge, the Mac's left edge), or press the hotkey (Win+Esc or Scroll Lock on the PC, Command+Esc on the Mac). A hotkey is decided by the computer it is pressed on, so one keyboard can switch both ways (H3). Touching a device never moves the focus.
- **Agreement**: each daemon keeps a numbered view of the focus and sends it to the other every second and on every change; a newer view replaces an older one, so both end up with the same view (S1, S2).

The firmware does not know where the focus is; it only keeps the input gate its daemon sets (F6).

## Key data flows

### Plugging in
1. User connects board A's "USB" port to computer A.
2. OS enumerates the composite device. The HID interface is usable immediately: once the radio link is up, the other computer can drive this one with the hotkey, even before this computer has a daemon (B8).
3. Today the daemon is a console program, built and started by hand (`daemon/README.md`). Planned (Phase 6): the `OMNI-KVM` Mass Storage volume carries installers that copy the daemon, register it as a login item (macOS) or auto-start service (Windows), and launch it.
4. The daemon looks for boards by their USB vendor ID (0x303A) every 2 s while it has none, and opens the board's CDC port. With several boards plugged in, it is told which port to use.
5. The board reports the radio link state. The daemon starts in split mode and sends its view.
6. There is no pairing step: both boards carry the same compiled-in development key (`security.md`). Pairing is planned (Phase 5).

### Plugging in again
1. User connects board A again (or USB reconnects it).
2. The running daemon finds it within 2 s and reconnects (D6). There is no stored identity: whichever board is plugged into this computer is used.
3. The daemon had gone to split mode when the board was lost, and stays there until the user switches (B2).

### Switching the focus
Every event in every state is in `docs/focus-design.md` (sections 3 and 9). An edge switch with the focus on the PC:
1. The PC daemon sees the pointer pushed past the PC's right edge by the edge resistance, by any device: the Mac's too, typed in by the PC's board.
2. It makes a new view (holder: the Mac, with the height at which the pointer left, as a fraction) and sends it as `MSG_VIEW` through both boards to the Mac daemon.
3. It freezes its pointer and closes its board's input gate (command 0x09, with a request number). Its own input is dropped until it forwards.
4. The PC's board drops keyboard reports not sent yet and releases every key and button it holds for the Mac. Once those releases have reached the PC, or at the latest 50 ms after the close, it confirms with status 0x06 and the same number. Without a confirmation within 100 ms, the daemon goes to split mode and reconnects to its board.
5. The PC daemon waits the settle time (20 ms), so that input its board typed in just before cannot be sent back (R2), then swallows the PC's input and forwards it.
6. The Mac daemon adopts the newer view. It was forwarding, so it stops and opens its gate (command 0x08), and shows its pointer just inside its left edge at the same fraction of the height (B4). It is now focused and forwards nothing: the PC's input arrives through its board.

The view goes out before any input forwarded under it, and each direction of the relay keeps order, so the Mac learns of the switch before that input arrives.

A hotkey switch has no entry position: the pointer resumes where it stopped. When the hotkey brings the focus to the computer it was pressed on, that daemon stops forwarding and opens its gate at once, and the other daemon closes its gate when it adopts the view.

### Heartbeat and disconnection
1. Both firmwares exchange a small heartbeat packet every 100 ms.
2. If a board hears nothing authentic from the other for 1 s (10 missed heartbeats, T_link), it declares the link down, drops the session, and tells its daemon at once. A restart of the other board is reported the same way.
3. Each board releases every key and mouse button it holds for the other computer (F8), so nothing stays stuck (B5).
4. Both daemons go to split mode (S7). A daemon that was forwarding stops and opens its gate. Each pointer resumes where it had stopped (B4); the Mac's is shown again.
5. When the boards hear each other again, a new session starts and they report the link up. The daemons exchange views and stay in split mode until the user switches (B2).
6. Planned (B9): notifications such as "peer disconnected" and "connection restored". Today the daemons only log the change.

If a daemon quits or crashes, the operating system closes its board's serial port. That board opens its gate, so the other computer can still drive this one (B8), and sends `MSG_HOST_GONE`; the other board releases everything it holds for this computer (F8). The other daemon hears no view for 3 s and goes to split mode (S8).

### Firmware update
Today each board is flashed over its "UART" port with PlatformIO (esptool). Nothing updates the firmware over USB or the radio.

Planned (Phase 6, not built yet): the daemon checks GitHub Releases (when internet is available) for newer firmware, downloads it, asks the user, and passes a signed binary to the board over USB (`security.md`, "Firmware update security"). `shared/protocol.md` reserves `MSG_DFU_ENTER` for entering an update mode.

The device itself never speaks to the internet. In the planned flow, the daemon acts as a controlled proxy.

## Major design decisions

### Why ESP-NOW (not Wi-Fi, not Bluetooth)

ESP-NOW is a connectionless protocol on the same 2.4 GHz Wi-Fi PHY but without association, IP, or DHCP overhead. Three reasons:

- **No router required.** The whole point of dedicated hardware is to work in environments with no usable Wi-Fi.
- **Low latency.** No connection state machine to traverse on every packet. Measured heartbeat round trip: 1.7–5.5 ms, mean about 2.5 ms (`docs/platform.md`, section 5).
- **Simple firmware.** The firmware never joins a network (no association, IP or DHCP), which keeps complexity and attack surface down.

Bluetooth was rejected because typical BLE HID round-trip is 10–30 ms with significant jitter, and pairing flows are inconsistent across host operating systems.

### Why a composite USB device

The HID interface works without any installation, so the device is always partially useful even before the daemon is set up: the other computer can already drive this one (B8). The CDC serial port gives the daemon its own channel over the same cable. Planned (Phase 6): a Mass Storage interface, the most reliable way to get installer files onto an unfamiliar machine without requiring drivers, network access, or OS-specific installer payloads on every host.

### Why the daemon owns the cursor logic, not the firmware

The firmware has no knowledge of monitor counts, resolutions, scaling factors, or which application is in the foreground. Putting cursor logic in the daemon means we can iterate on UX (edge resistance, edge mapping, pointer placement) without re-flashing the device. Hotkey switching was added this way, in the daemon (`daemon/src/hotkey.rs`); later features (per-application policies, full-screen detection) can follow.

### Why symmetric firmware and daemon

Hardcoding "this device is the master, that one is the slave" would force re-flashing if the user wanted to switch sides, complicate spare-parts logic, and make bidirectional operation harder. The same firmware binary runs on both boards and has no role, so either board works on either computer. The daemons share one source; the only fixed difference is which computer is which (the PC on the left, the Mac on the right), taken from the operating system. The PC's decision also wins ties (S6). Where the focus is, is run-time state (`docs/focus-design.md`).

### Why bidirectional from day one

Retrofitting bidirectional control into a unidirectional design is a large refactor. State management, input suppression, and switching logic all change. Doing it from the start costs little extra and avoids a future rewrite.

## Out of scope (for now)

- Video / display sharing (we are explicitly a K+M, not a full KVM).
- File transfer between hosts.
- Audio routing.
- Mobile devices (iOS / Android) as either side.
- More than two paired devices simultaneously.

These may be revisited after v1.0 if there is demand. They are noted here so reviewers understand they were considered and deferred, not overlooked.
