# Omni-KVM

> A wireless hardware KVM for low-latency keyboard and mouse sharing across operating systems.

**Status**: Early development · **License**: MIT

---

## What is this?

Omni-KVM is a pair of small USB devices that let you control two computers (Windows, macOS, planned: Linux) with a single keyboard and mouse — over a dedicated 2.4 GHz radio link, not Wi-Fi.

Software KVMs like Barrier and Synergy depend on the host network. They suffer from variable latency, dropouts on congested Wi-Fi, and complete failure when no network is available. Omni-KVM avoids all of this by acting as a real USB HID device on each machine and communicating over its own radio channel.

## Goals

- **Plug-and-play HID**: Each device appears as a standard USB keyboard + mouse. Basic input works without any installation.
- **Low latency**: Target end-to-end latency under 7 ms with jitter under 2 ms.
- **Network-independent**: No Wi-Fi router, no internet, no LAN required. Just plug both devices in.
- **Bidirectional**: Either computer can be the active source. Switch direction by moving the cursor across screen edges.
- **Cross-platform**: Windows and macOS at v1.0. Linux on the roadmap.
- **Multi-monitor aware**: Host daemons negotiate a shared virtual desktop layout.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ Layer 5: UX (virtual resistance, screen lock, alerts)   │  Host daemon
├─────────────────────────────────────────────────────────┤
│ Layer 4: Host daemon (input hooks, state, monitor map)  │  Host daemon
├─────────────────────────────────────────────────────────┤
│ Layer 3: Protocol (heartbeat, sequencing, encryption)   │  Both
├─────────────────────────────────────────────────────────┤
│ Layer 2: Transport (ESP-NOW radio, retransmits)         │  Firmware
├─────────────────────────────────────────────────────────┤
│ Layer 1: USB HID (composite device emulation)           │  Firmware
└─────────────────────────────────────────────────────────┘
```

See `docs/architecture.md` for the full design.

## Hardware

- 2× ESP32-S3-DevKitC-1-N8R8 development boards
- 2× USB-C cables (data-capable; one C-to-C and one C-to-A recommended)

Total parts cost: roughly KRW 65,000 / USD 50.

## Security

Wireless input is a sensitive channel. The threat model and mitigations are documented in `docs/security.md` and `docs/threat-model.md`. Summary:

- AES-128 encrypted radio link with sequence numbers (replay protection)
- MAC-address-based peer filtering after a one-time pairing
- Devices never connect to the internet; firmware updates are mediated by the host daemon
- Factory reset requires physical button press

## Roadmap

- [ ] **Phase 0** — Wired proof-of-concept between two ESP32-S3 boards
- [ ] **Phase 1** — USB HID composite device firmware
- [ ] **Phase 2** — ESP-NOW wireless link with encryption
- [ ] **Phase 3** — Windows host daemon + virtual resistance UX
- [ ] **Phase 4** — macOS host daemon + monitor layout negotiation
- [ ] **Phase 5** — Fail-safe behavior, edge cases, pairing flow
- [ ] **Phase 6** — Mass storage installer, GitHub-based updates
- [ ] **v1.1** — Linux X11 support
- [ ] **v2.0** — Linux Wayland support

## Project layout

```
omni-kvm/
├── firmware/         ESP32-S3 firmware (PlatformIO project)
├── daemon-windows/   Windows host daemon
├── daemon-macos/     macOS host daemon
├── shared/           Shared protocol definitions and constants
└── docs/             Architecture, protocol, security documentation
```

## License

MIT — see `LICENSE`.
