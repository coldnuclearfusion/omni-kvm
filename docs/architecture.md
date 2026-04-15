# Architecture

This document describes the overall design of Omni-KVM. It complements the project README with an expanded view of components, data flows, and the rationale behind major design decisions.

## System overview

Omni-KVM is composed of four cooperating components: two firmware instances running on the ESP32-S3 devices, and two host daemons running on the connected computers.

```
       Computer A (Windows)               Computer B (macOS)
  ┌────────────────────────────┐    ┌────────────────────────────┐
  │  Host daemon A             │    │  Host daemon B             │
  │  • input hook              │    │  • input hook              │
  │  • monitor layout          │    │  • monitor layout          │
  │  • UX state machine        │    │  • UX state machine        │
  │  • update orchestrator     │    │  • update orchestrator     │
  └────────────┬───────────────┘    └────────────┬───────────────┘
               │ USB CDC                         │ USB CDC
               │ (serial)                        │ (serial)
  ┌────────────▼───────────────┐    ┌────────────▼───────────────┐
  │  Firmware A (ESP32-S3)     │    │  Firmware B (ESP32-S3)     │
  │  • USB composite device    │    │  • USB composite device    │
  │  • ESP-NOW transport       │◄══►│  • ESP-NOW transport       │
  │  • heartbeat               │    │  • heartbeat               │
  │  • encryption              │    │  • encryption              │
  └────────────────────────────┘    └────────────────────────────┘
                          ◄══ 2.4 GHz, AES-128 ══►
```

The system is intentionally symmetric. Both halves run identical firmware and daemon code; their roles are determined dynamically rather than hardcoded.

## Layered model

```
┌─────────────────────────────────────────────────────────────┐
│ L5 — UX            virtual resistance, screen lock, alerts  │ daemon
├─────────────────────────────────────────────────────────────┤
│ L4 — Application   input hooks, monitor layout, state mgmt  │ daemon
├─────────────────────────────────────────────────────────────┤
│ L3 — Protocol      message format, sequencing, encryption   │ shared
├─────────────────────────────────────────────────────────────┤
│ L2 — Transport     ESP-NOW radio, retransmits, heartbeat    │ firmware
├─────────────────────────────────────────────────────────────┤
│ L1 — Physical I/O  USB HID, USB CDC, USB MSC, radio PHY     │ firmware
└─────────────────────────────────────────────────────────────┘
```

Each layer depends only on the one below it. This makes individual layers testable in isolation: L1+L2 can be validated with a wired (UART) link before introducing the radio, L3 can be exercised with simulated packets, and L4+L5 can be developed against a stub firmware.

## Component responsibilities

### Firmware (ESP32-S3)

The firmware is intentionally minimal. It does not understand cursor positions, monitor layouts, or which side is currently "active". Its job is to be a reliable transport between the radio and the USB ports on each side.

**Owns:**
- USB composite device descriptor and enumeration
- Receiving HID reports from radio → injecting them into the host
- Receiving commands from daemon over CDC → encoding and transmitting over radio
- ESP-NOW peer setup, retransmits, and heartbeat
- AES-128 encryption and replay protection (sequence numbers)
- Local fail-safe (e.g. release stuck modifier keys when peer goes silent)
- USB Mass Storage with installer payloads
- DFU mode entry for daemon-mediated firmware updates

**Does not own:**
- Cursor coordinates or screen edge detection
- Monitor layout
- User-facing notifications
- Pairing UI (only the cryptographic primitives)

### Host daemon

The daemon is where all user-facing logic lives. It is the only component that understands what it means for the cursor to "leave one screen and enter another".

**Owns:**
- Hooking the OS input stream (Raw Input on Windows, CGEventTap on macOS)
- Querying and tracking the local monitor layout
- Negotiating the shared virtual desktop with the peer daemon
- Deciding when to hand off control (virtual resistance, edge detection)
- Suppressing local input when this side is the passive receiver
- Showing toast/notification UI for connection state changes
- Detecting full-screen / exclusive-mode applications (lock signal)
- Downloading firmware updates and streaming them to the device
- Persisting pairing configuration and user preferences

**Does not own:**
- Direct radio communication (always goes through firmware over CDC)
- Encryption keys in plaintext on disk (delegated to OS keychain / DPAPI)

## USB composite device

Each firmware exposes the following interfaces over a single USB connection:

| Interface | Purpose |
|---|---|
| HID keyboard | Inject keystrokes into the host |
| HID mouse | Inject pointer movement, buttons, scroll |
| CDC serial | Bidirectional control channel with the host daemon |
| Mass Storage | Read-only drive containing installers and README |

The HID interfaces work with the OS's built-in drivers — no installation is required for basic input to function. The Mass Storage interface is the entry point for first-time setup: when the user plugs in a fresh device, a small drive named `OMNI-KVM` appears in their file manager containing platform-specific installers.

The CDC serial channel is invisible to typical users and serves as the daemon's private communication path with the firmware.

## Activation model

At any moment, exactly one side is the **active source**: the side whose physical keyboard and mouse are being used to drive both computers. The other side is the **passive receiver**.

Activation is a daemon-level concept; the firmware is unaware of it. Rules:

1. On startup, the side that first observes local input becomes the active source.
2. The active source's daemon hooks local input and forwards it to the peer when the cursor is on the peer's territory.
3. The passive receiver's daemon suppresses its own local input from generating events on its host (otherwise both keyboards would fight).
4. When the passive receiver's local keyboard or mouse is touched, that side immediately becomes the active source. The previous active source releases its hold.

This makes the system bidirectional with no explicit "switch direction" command from the user.

## Key data flows

### First-time plug-in
1. User connects firmware A to computer A.
2. OS enumerates the composite device. HID interfaces become usable immediately; the user can type and move the cursor.
3. The Mass Storage volume `OMNI-KVM` mounts. The user opens it and runs the OS-appropriate installer.
4. Installer copies the daemon binary, registers it as a login item (macOS) or auto-start service (Windows), and launches it.
5. Daemon enumerates serial ports, locates firmware A by USB VID/PID, opens CDC connection.
6. Daemon and firmware exchange identity. If no pairing exists, the daemon enters pairing mode (see `security.md`).
7. Once paired, the daemon loads the monitor layout and begins normal operation.

### Subsequent plug-in
1. User connects firmware A.
2. The already-running daemon detects the new CDC port and identifies firmware A by stored identity.
3. Pairing config is reused; no setup required.
4. Operation begins immediately.

### Cursor crossing screen edges
1. Active source daemon (say A) tracks the cursor in its local monitor space.
2. Cursor reaches the edge that maps to the peer's virtual desktop region. Daemon A clamps the cursor at the boundary (virtual resistance).
3. If the user continues pushing in the same direction with at least the configured threshold, daemon A:
   - Records the entry point on the peer's screen
   - Sends a `HANDOFF` message over CDC → radio → firmware B → daemon B
   - Hides its local cursor and stops forwarding pointer events to its OS
4. Daemon B receives `HANDOFF`, becomes the active source, places cursor at the negotiated entry point, begins forwarding input.
5. Subsequent keyboard and mouse events from A's physical input are forwarded as HID reports to B until the reverse handoff occurs.

### Heartbeat and disconnection
1. Both firmwares exchange a small heartbeat packet every 100 ms.
2. If a firmware misses 30 consecutive heartbeats (3 seconds), it declares the link down and notifies its daemon over CDC.
3. Daemon behavior depends on which side held the cursor:
   - **Cursor was on this side**: show a toast ("Omni-KVM: peer disconnected"), do not move the cursor.
   - **Cursor was on the peer side**: this side becomes the active source. The cursor is forced to a safe local position (configurable; default: center of primary monitor) so the user is not left without a controllable pointer.
4. Both daemons release any held modifier keys to prevent stuck keys.
5. When heartbeat resumes, daemons synchronize state and show a "connection restored" notification.

### Firmware update
1. Daemon checks GitHub Releases (when internet is available) for newer firmware.
2. If found, daemon downloads the firmware binary to local storage.
3. Daemon prompts the user for confirmation.
4. On confirmation, daemon sends `ENTER_DFU` over CDC. Firmware reboots into DFU mode and exposes itself as a writable Mass Storage volume.
5. Daemon writes the new firmware binary to the volume.
6. Firmware verifies, applies, and reboots.
7. Daemon waits for the device to reappear and confirms the new version string.

The device itself never speaks to the internet. The daemon acts as a controlled proxy.

## Major design decisions

### Why ESP-NOW (not Wi-Fi, not Bluetooth)

ESP-NOW is a connectionless protocol on the same 2.4 GHz Wi-Fi PHY but without association, IP, or DHCP overhead. Three reasons:

- **No router required.** The whole point of dedicated hardware is to work in environments with no usable Wi-Fi.
- **Low and stable latency.** No connection state machine to traverse on every packet. Round-trip is consistently 2–4 ms in our target range.
- **Simple firmware.** Avoiding the full Wi-Fi stack reduces flash usage, complexity, and attack surface.

Bluetooth was rejected because typical BLE HID round-trip is 10–30 ms with significant jitter, and pairing flows are inconsistent across host operating systems.

### Why a composite USB device with Mass Storage

Mass Storage is the most reliable way to get installer files onto an unfamiliar machine without requiring drivers, network access, or OS-specific installer payloads on every host. The HID interfaces work without any installation, so the device is always partially useful even before the daemon is set up.

### Why the daemon owns the cursor logic, not the firmware

The firmware has no knowledge of monitor counts, resolutions, scaling factors, or which application is in the foreground. Putting cursor logic in the daemon means we can iterate on UX (virtual resistance threshold, edge mappings, full-screen detection) without re-flashing the device. It also lets us add features later (per-application policies, hotkey-driven switching) entirely in software.

### Why symmetric firmware and daemon

Hardcoding "this device is the master, that one is the slave" would force re-flashing if the user wanted to switch sides, complicate spare-parts logic, and make bidirectional operation harder. Symmetric design means the same binary runs on both sides; roles emerge from runtime state.

### Why bidirectional from day one

Retrofitting bidirectional control into a unidirectional design is a large refactor. State management, input suppression, and handoff logic all change. Doing it from the start costs little extra and avoids a future rewrite.

## Out of scope (for now)

- Video / display sharing (we are explicitly a K+M, not a full KVM).
- File transfer between hosts.
- Audio routing.
- Mobile devices (iOS / Android) as either side.
- More than two paired devices simultaneously.

These may be revisited after v1.0 if there is demand. They are noted here so reviewers understand they were considered and deferred, not overlooked.
