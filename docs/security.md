# Security Design

This document describes the security architecture of Omni-KVM. It covers the mechanisms we implement, the guarantees they provide, and their limits. For an explicit analysis of what attacks are and are not mitigated, see `threat-model.md`. Mechanisms marked **Planned** are designed but not built yet.

## Why security matters for this project

Omni-KVM transports raw keyboard and mouse input over a wireless radio link. Keyboard input includes passwords, credit card numbers, private messages, and shell commands. An insecure link would broadcast this data to anyone within radio range (~30–100 m depending on environment). Treating security as an afterthought would make this project dangerous to use.

## Security principles

1. **No internet exposure.** The ESP32-S3 firmware never connects to Wi-Fi access points or the internet. All radio activity is ESP-NOW between the two boards: session handshakes are broadcast until a board has found its peer, and everything else goes to that one peer. This eliminates an entire class of remote attacks.

2. **Encrypt by default.** Every radio packet is sealed with ChaCha20-Poly1305 (encrypted and authenticated), and the firmware drops any radio packet that is not sealed with its key. The only exceptions are benchmark builds in `firmware/platformio.ini`: `bench_none` sends and accepts unencrypted packets, and `bench_espnow` leaves encryption to ESP-NOW's own CCMP. They exist to measure latency and must never carry real input (`firmware/include/secure_packet.h`). Until pairing exists, every build uses a development key generated locally and never committed (see [Encryption](#encryption)).

3. **Minimal attack surface on the firmware.** The firmware does not parse complex formats, run a web server, accept firmware updates (it is flashed over its UART port), or expose any IP-based services. Its radio interface accepts only 64-byte packets with a known magic byte. Until it has a peer, a board handles only session handshakes; the first board whose handshake authenticates with the key becomes its peer until the board restarts, and packets from any other MAC address are then ignored (`firmware/include/radio_link.h`). **Planned** with pairing: the paired peer's MAC address stored and filtered.

4. **Secrets stay in secure storage.** **Planned.** Today the development key is compiled into the firmware and sits unencrypted in the board's flash: no eFuse key, and flash encryption is not enabled in the build. The daemon holds no key. The plan: keys stored in the ESP32-S3's eFuse or NVS (Non-Volatile Storage) with flash encryption enabled; on the host side, any key in the OS keychain (macOS Keychain / Windows DPAPI), never in plaintext config files.

5. **Physical access required for security-critical operations.** **Planned** with pairing (Phase 5). Pairing a new device and factory reset will both require a physical action on the hardware. No software command — local or remote — may trigger these operations. This prevents a compromised daemon from silently re-pairing the device with an attacker's unit. Today there is no pairing or factory reset (the key is compiled in), and the daemon commands that would set a key or start pairing stay unimplemented (`shared/protocol.md`, `MSG_DAEMON_CMD`). The board has no PAIR button, only Boot and Reset (`docs/platform.md`, section 1); which physical action pairing uses is still to be decided.

## Pairing

**Planned (Phase 5), not built yet.** Today both boards are flashed with the same development key (see [Encryption](#encryption)), and a board takes as its peer the first board whose handshake authenticates with it. This section describes how pairing would work. "PAIR" below stands for the physical action the pairing design settles on, since the board has no button for it.

Pairing is the one-time process by which two Omni-KVM devices establish mutual trust and a shared encryption key. It happens once per device pair; the result is persisted and reused on every subsequent connection.

### Pairing flow

```
  Device A                                          Device B
  ────────                                          ────────
  User triggers PAIR
  within 30 seconds...                              User triggers PAIR

  1. A broadcasts MSG_PAIR_REQUEST
     (contains A's public identity)
                                          ────────►
                                                    2. B receives request,
                                                       verifies PAIR was triggered,
                                                       generates challenge

                                          ◄────────
                                                    3. B sends MSG_PAIR_CHALLENGE
                                                       (contains B's public identity
                                                        + random nonce)

  4. A computes response using
     ECDH key exchange or
     shared-secret derivation
                                          ────────►
  5. A sends MSG_PAIR_CONFIRM
     (contains key material)

                                                    6. B verifies, derives
                                                       the same shared key

                                          ◄────────
                                                    7. B sends MSG_PAIR_COMPLETE
                                                       (confirmation)

  Both sides store the 256-bit
  shared key and peer MAC address.
```

### Pairing security properties

- **Physical action required on both sides**: An attacker cannot initiate pairing remotely. Both devices must be in pairing mode simultaneously, which requires physical access to both.
- **30-second window**: Pairing mode automatically exits after 30 seconds if not completed. This minimizes the window during which a device accepts unknown peers.
- **One peer only**: Each device stores exactly one paired peer. Pairing a new peer overwrites the old one. There is no "add another device" — this is a deliberate simplification that prevents confusion and reduces attack surface.
- **Visual feedback**: During pairing, the onboard LED blinks a distinct pattern so the user can confirm which devices are in pairing mode.

### Key derivation

The pairing process will use ECDH (Elliptic Curve Diffie-Hellman) on Curve25519 to establish a shared secret without either side transmitting the key itself. The shared secret is then passed through a key derivation function to derive the 256-bit encryption key. This means:

- Even if an attacker records the entire pairing exchange, they cannot derive the key without one side's private key.
- The key is never transmitted over the radio in any form.

Implementation note: Curve25519 (X25519) and the key derivation will use Monocypher, which the firmware already includes for the radio cipher.

## Encryption

### Algorithm: ChaCha20-Poly1305

Every radio packet is sealed with ChaCha20-Poly1305 (IETF variant, RFC 8439). See `protocol.md` ("Encryption") for the exact byte layout.

- **Mode**: an authenticated encryption (AEAD) cipher. It provides **confidentiality** (the message data is unreadable without the key) and **integrity** (a 16-byte Poly1305 tag covers the ciphertext and the plaintext header, so any altered bit is detected and the packet dropped).
- **Key**: 256-bit. Every build today uses one development key, which `tools/make_dev_key.py` generates locally into `firmware/include/secrets/dev_key.h` (git-ignored) and which is compiled into both boards' firmware. **Planned**: pairing will derive the key on the devices (Phase 5).
- **Nonce**: 12 random bytes from the hardware RNG, fresh for every packet and sent alongside it. Random nonces stay unique across reboots and between the two boards, which share one key.
- **Implementation**: Monocypher 4.0.3, a small audited C library, vendored unmodified in `firmware/lib/monocypher/`. Its release tarball was checked against the SHA-512 published on monocypher.org before vendoring (details in that folder's README). We do not implement any cryptographic primitive ourselves.
- **Cost**: measured ≈ 0.14–0.16 ms to seal and ≈ 0.11 ms to open a packet (`protocol.md`, Phase 2; `docs/platform.md`, section 5).

**Why ChaCha20-Poly1305.** Four options were benchmarked on the hardware (table in `protocol.md`): no encryption, ESP-NOW's built-in CCMP, ChaCha20-Poly1305, and AES-128-GCM via mbedTLS with the AES accelerator. ESP-NOW CCMP was fastest but rejected for the reasons discussed below. ChaCha20-Poly1305 beat AES-GCM by about 0.2 ms per one-way trip (the accelerator's per-call setup dominates for 28-byte payloads), and one library then covers the cipher, session key derivation, and (planned) pairing.

**Why not ESP-NOW's built-in encryption.** It encrypts in the Wi-Fi hardware with almost no CPU cost, but Espressif documents its replay protection in a single sentence, so we cannot verify the property we rely on most; it cannot encrypt broadcast frames; and it would tie security to the radio layer instead of our protocol layer.

**Why the design changed from AES-128-CTR.** The first version of this document specified AES-128-CTR with a nonce built from the sequence number. CTR alone has no integrity check, so an attacker could flip ciphertext bits to change keystrokes without knowing the key, and the sequence number restarts after a reboot, which would reuse nonces under the same key. `protocol.md` explains both flaws in detail.

**Verified on hardware (Phase 2)**, for both AES-GCM and the final ChaCha20-Poly1305 build: two boards with the same key link up with zero authentication failures; when one board is flashed with a different key, every packet from it fails authentication and the boards never accept each other as peers.

### What is encrypted

| Component | Encrypted? | Reason |
|---|---|---|
| Packet header (bytes 0–7) | No (authenticated) | Receiver needs magic, version, type, seq before decryption; the tag covers it |
| Nonce (bytes 8–19) | No | Receiver needs it to decrypt; it is random and reveals nothing |
| Message data (bytes 20–47) | Yes | Contains sensitive input data (keystrokes, mouse movement) and the daemons' messages |
| Tag (bytes 48–63) | No | Receiver checks it before accepting the packet |
| USB CDC traffic (daemon ↔ local firmware) | No | Local USB bus; encrypting adds latency for no security gain |
| Heartbeat payload | Yes | Sealed like every radio packet; its plaintext type still shows it is a heartbeat |

### What an eavesdropper can see

Even with encryption active, an observer within radio range can determine:
- That two Omni-KVM devices are communicating (MAC addresses, magic byte)
- Packet timing and frequency (reveals whether the user is actively typing or idle)
- Packet types, since the header is plaintext (every packet is 64 bytes, but the type byte tells them apart). This shows, for example, focus switches (`MSG_VIEW` outside its once-a-second repeat) and which board forwards input (`MSG_KEY_STATE`, sent once a second by the forwarding side only).

This is explicitly documented in `threat-model.md` as an accepted residual risk.

## Replay protection

Each packet carries a 32-bit sequence number in the plaintext header. The receiver keeps the highest sequence number accepted from its peer in the current session and drops any packet with a sequence number less than or equal to it. (Session handshakes are not sequence-checked; a recorded one cannot complete a handshake, see below.)

This prevents an attacker from recording encrypted packets and replaying them later. Even though the attacker cannot read the contents, replaying old mouse movements or keystrokes could cause unintended actions. The sequence number is in the header, which the authentication tag covers, so it cannot be altered to make an old packet look new.

**Session keys.** A sequence number alone cannot survive a reboot: the rebooted board counts from 1 again, and the receiver cannot tell that apart from a replay. The first firmware with encryption therefore reset the receiver's counter after 3 seconds of link loss, which left a gap: record traffic, jam the channel for 3 seconds, replay. Session keys close it. On every link-up, each board contributes a fresh random nonce in a `MSG_SESSION_HELLO` handshake (sealed with the long-term key), and both derive a session key from the long-term key and the two nonces (BLAKE2b). All other packets are sealed with the session key, so packets from an earlier session fail authentication, and the receiver's sequence check can safely start over with each session. A session is only established when the peer echoes back our fresh nonce, so a recorded HELLO cannot start one. Protocol details: `protocol.md` ("Sessions").

**Verified on hardware (Phase 2)** with a test build in which one board replays its own recorded packets: a replay within the same session was dropped by the sequence check; a replay from the previous session, after a restart, failed authentication; a restarting peer got a new session without a 3-second outage.

The sequence counter wraps at 2^32 (~4 billion). At 1000 packets/second, this takes ~50 days of continuous use before wrapping. Nothing starts a new session at the wrap. A board's counter runs from boot and a new session does not reset it, so after a wrap the board sends small sequence numbers, which the other board drops as replays. Having accepted nothing from it for 1 s (T_link), the other board declares the link lost, and the new session that follows starts its sequence check over. Replay protection holds throughout; the cost is about a second in which that board's packets are lost, then a link loss as described below.

## Fail-safe behavior

Security-relevant fail-safe behaviors:

- **Stuck keys**: Each board releases every key and mouse button it holds down for the other computer, modifiers included, when the radio link is lost (1 s without an authentic packet from the other board, T_link), when the other board restarts (a new session), when its input gate closes, and when the other board reports that its daemon disconnected (`MSG_HOST_GONE`) (`firmware/src/main.cpp`; requirements F6, F8). A daemon that forwards also sends `MSG_KEY_STATE` every second, and the receiving board lets go of anything not listed, which repairs a release lost on the radio. This prevents a "stuck Ctrl" scenario where the user unknowingly sends Ctrl+key combinations after reconnection.
- **Firmware watchdog**: The firmware configures no watchdog of its own. The build's defaults apply: a task watchdog (5 s) that watches only core 0's idle task, and an interrupt watchdog (300 ms) (`docs/platform.md`, section 3). The main loop runs on core 1, so a stalled main loop is not reset automatically: the user presses Reset or unplugs the board (S10). (The USB watchdog in `firmware/src/usb_guard.cpp` only reconnects USB.) Meanwhile the other board hears nothing, declares the link lost within 1 s, and releases what it held. After any restart, a board re-establishes the encrypted link with its compiled-in key (a new session).
- **Daemon crash**: If the host daemon crashes or quits, the operating system closes the board's serial port and removes the daemon's input capture, so that computer's own keyboard and mouse work normally again (S8). The board sees the port close, opens its input gate (the other computer can still drive this one as a plain USB keyboard and mouse, B8) and sends `MSG_HOST_GONE`; the other board then releases every key and button it holds for this computer (F8). The other daemon hears no view for 3 s and goes to split mode (S8). Nothing restarts the daemon yet: it is a console program started by hand (`daemon/README.md`).

## Firmware update security

**Planned (Phase 6), not built yet.** Today each board is flashed over its "UART" port with PlatformIO (esptool); the daemon has no update function, and the firmware accepts no update over USB or the radio.

Firmware updates will be mediated by the host daemon, never received directly from the internet by the device. The update flow:

1. Daemon downloads a signed firmware binary from GitHub Releases.
2. Daemon verifies the binary's Ed25519 signature against a public key embedded in the daemon.
3. Daemon transfers the verified binary to the device over USB CDC.
4. Device writes the binary to its OTA partition, verifies the SHA-256 hash, and reboots into the new firmware.
5. If verification fails at any step, the device remains on its current firmware.

This chain ensures that:
- The device never touches the internet.
- A compromised update server cannot push unsigned firmware.
- A man-in-the-middle between the daemon and GitHub cannot inject malicious firmware (signature check).

The Ed25519 signing key pair will be generated by the project maintainer. The private key is never committed to the repository. The public key is embedded in both the daemon binary and the firmware, allowing cross-verification.

## What this project does NOT protect against

These are documented in detail in `threat-model.md`, but summarized here:

- **Physical access to a running device**: If an attacker can physically access a device that is plugged into a computer, they can read USB traffic, dump firmware, or replace the device entirely. Without flash encryption, dumping the firmware also reveals the radio key (`threat-model.md`, T10). Physical security is the user's responsibility.
- **Compromised host computer**: If the computer running the daemon is compromised (malware, rootkit), the attacker already has access to all keyboard input before it reaches our system. Omni-KVM cannot add security to an already-compromised host.
- **Advanced traffic analysis**: Packet timing reveals user activity patterns. Mitigating this fully would require constant-rate dummy traffic, which increases power consumption and radio congestion. This is a deliberate trade-off for a personal-use device.
- **Hardware-level backdoors in the ESP32-S3**: See `threat-model.md` for a detailed discussion. Summary: no confirmed backdoors exist; the residual risk is accepted and documented.
- **Quantum computing**: Curve25519 (planned for pairing) is not quantum-resistant; the 256-bit ChaCha20 key keeps a comfortable margin even against known quantum attacks on symmetric ciphers. This is irrelevant for the project's threat model (personal use, not state-secret protection) and noted for completeness.
