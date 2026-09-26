# Omni-KVM

> A wireless hardware KVM for low-latency keyboard and mouse sharing across operating systems.

**Status**: Early development: a Windows PC and a Mac share one keyboard-and-mouse focus over the radio; pairing, installers and updates are not built yet · **License**: MIT

---

## What is this?

Omni-KVM is a pair of small USB devices that let you control two computers (Windows, macOS, planned: Linux) with a single keyboard and mouse — over a dedicated 2.4 GHz radio link, not Wi-Fi.

Software KVMs like Barrier and Synergy depend on the host network. They suffer from variable latency, dropouts on congested Wi-Fi, and complete failure when no network is available. Omni-KVM avoids all of this by acting as a real USB HID device on each machine and communicating over its own radio channel.

## Goals

- **Plug-and-play HID**: Each device appears as a standard USB keyboard + mouse. Basic input works without any installation.
- **Low latency**: Target end-to-end latency under 7 ms with jitter under 2 ms.
- **Network-independent**: No Wi-Fi router, no internet, no LAN required. Just plug both devices in.
- **Shared focus**: The keyboards and mice of both computers drive whichever computer has the focus. Switch by pushing the pointer past the facing screen edge, or with a hotkey (Win+Esc or Scroll Lock on Windows, Command+Esc on the Mac).
- **Cross-platform**: Windows and macOS at v1.0. Linux on the roadmap.
- **Screen layout**: Fixed for now (the Mac to the right of the PC); the pointer enters the other screen at the same relative height.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Layer 5: UX (virtual resistance, hotkeys; alerts later) │  Host daemon
├─────────────────────────────────────────────────────────┤
│ Layer 4: Host daemon (input capture, shared focus)      │  Host daemon
├─────────────────────────────────────────────────────────┤
│ Layer 3: Protocol (heartbeat, sequencing, encryption)   │  Both
├─────────────────────────────────────────────────────────┤
│ Layer 2: Transport (ESP-NOW radio, retransmits)         │  Firmware
├─────────────────────────────────────────────────────────┤
│ Layer 1: USB HID (composite device emulation)           │  Firmware
└─────────────────────────────────────────────────────────┘
```

Documents:

- `docs/requirements.md`: what the system must do, and the experiments behind it
- `docs/focus-design.md`: the shared focus, checked against every order of events
- `shared/protocol.md`: every message on the radio and over USB
- `docs/platform.md`: facts and limits of the board, the chip and the software under the firmware, with sources
- `docs/architecture.md`: overview of the layers
- `docs/security.md`, `docs/threat-model.md`: security design and threat analysis
- `docs/acceptance-tests.md`: acceptance test runs and results

## Hardware

* 2× ESP32-S3-DevKitC-1-N8R8 development boards
* 2× Micro-USB data cables for development (USB-A to Micro-USB; standard Android-style cables work)
* For deployment: Micro-USB to USB-A (for desktops) and Micro-USB to USB-C (for modern laptops)

Total parts cost: roughly KRW 65,000 / USD 50.

## Security

Wireless input is a sensitive channel. The threat model and mitigations are documented in `docs/security.md` and `docs/threat-model.md`. Summary:

- Built: authenticated encryption (ChaCha20-Poly1305) on every radio packet, with a fresh session key per connection and sequence numbers for replay protection. Both boards are built with the same development key for now.
- The devices never connect to the internet.
- Planned: a one-time pairing with MAC-address-based peer filtering, firmware updates through the host daemon (today: flashed over the board's UART port), and a factory reset that needs a physical button press.

## Roadmap

- [x] **Phase 0** — Development environment + hello-world firmware on both boards
- [x] **Phase 1** — USB HID composite device firmware
- [x] **Phase 2** — ESP-NOW wireless link with encryption
- [x] **Phase 3** — Windows host daemon + virtual resistance UX
- [x] **Phase 4** — macOS host daemon and the shared focus (accepted 2026-09-26, `docs/acceptance-tests.md`)
- [ ] **Phase 5** — Fail-safe behavior, edge cases, pairing flow
  - A leaner model checker (compact states, one order of independent events) to finish the deeper check of the focus design (`docs/focus-design.md`, section 10)
  - Replace the precompiled USB device driver (TinyUSB 0.16 `dcd_esp32sx`) with a corrected build, fixing its FIFO layout, unlocked register updates, its reset on an unknown interrupt flag and its reset handling at the source (see `docs/platform.md`, K1, K2, K6, K7); the firmware works around them for now
- [ ] **Phase 6** — Mass storage installer, GitHub-based updates
- [ ] **v1.1** — Linux X11 support
- [ ] **v2.0** — Linux Wayland support

## Project layout

```
omni-kvm/
├── firmware/         ESP32-S3 firmware (PlatformIO project)
├── daemon/           Host daemon (Rust): shared logic + per-OS modules (Windows, macOS)
├── shared/           Shared protocol definitions and constants
├── tools/            Development and test utilities
└── docs/             Requirements, design, platform facts, security, test results
```

## License

MIT — see `LICENSE`.
