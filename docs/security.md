# Security Design

This document describes the security architecture of Omni-KVM. It covers the mechanisms we implement, the guarantees they provide, and their limits. For an explicit analysis of what attacks are and are not mitigated, see `threat-model.md`.

## Why security matters for this project

Omni-KVM transports raw keyboard and mouse input over a wireless radio link. Keyboard input includes passwords, credit card numbers, private messages, and shell commands. An insecure link would broadcast this data to anyone within radio range (~30–100 m depending on environment). Treating security as an afterthought would make this project dangerous to use.

## Security principles

1. **No internet exposure.** The ESP32-S3 firmware never connects to Wi-Fi access points or the internet. All radio activity is limited to ESP-NOW peer-to-peer communication with a single paired device. This eliminates an entire class of remote attacks.

2. **Encrypt by default.** After initial pairing, all payload data is AES-128-CTR encrypted. There is no "unencrypted mode" in production firmware. Debug builds may disable encryption for development, but they are clearly marked and refuse to pair with production devices.

3. **Minimal attack surface on the firmware.** The firmware does not parse complex formats, run a web server, accept OTA updates from the network, or expose any IP-based services. Its radio interface accepts only packets from a single known MAC address, with a known magic byte, in a fixed-size format.

4. **Secrets stay in secure storage.** Encryption keys are stored in the ESP32-S3's eFuse or NVS (Non-Volatile Storage) with flash encryption enabled where supported. On the host side, keys are stored in the OS keychain (macOS Keychain / Windows DPAPI), never in plaintext config files.

5. **Physical access required for security-critical operations.** Pairing a new device and factory reset both require pressing a physical button on the hardware. No software command — local or remote — can trigger these operations. This prevents a compromised daemon from silently re-pairing the device with an attacker's unit.

## Pairing

Pairing is the one-time process by which two Omni-KVM devices establish mutual trust and a shared encryption key. It happens once per device pair; the result is persisted and reused on every subsequent connection.

### Pairing flow

```
  Device A                                          Device B
  ────────                                          ────────
  User presses PAIR button
  within 30 seconds...                              User presses PAIR button

  1. A broadcasts MSG_PAIR_REQUEST
     (contains A's public identity)
                                          ────────►
                                                    2. B receives request,
                                                       verifies button was pressed,
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

  Both sides store the 128-bit
  shared key and peer MAC address.
```

### Pairing security properties

- **Button press required on both sides**: An attacker cannot initiate pairing remotely. Both devices must be in pairing mode simultaneously, which requires physical access to both.
- **30-second window**: Pairing mode automatically exits after 30 seconds if not completed. This minimizes the window during which a device accepts unknown peers.
- **One peer only**: Each device stores exactly one paired peer. Pairing a new peer overwrites the old one. There is no "add another device" — this is a deliberate simplification that prevents confusion and reduces attack surface.
- **Visual feedback**: During pairing, the onboard LED blinks a distinct pattern so the user can confirm which devices are in pairing mode.

### Key derivation

The pairing process uses ECDH (Elliptic Curve Diffie-Hellman) on Curve25519 to establish a shared secret without either side transmitting the key itself. The shared secret is then passed through HKDF-SHA256 to derive the 128-bit AES key. This means:

- Even if an attacker records the entire pairing exchange, they cannot derive the key without one side's private key.
- The key is never transmitted over the radio in any form.

Implementation note: ESP32-S3 has hardware acceleration for SHA-256. Curve25519 operations will use a lightweight library (e.g., micro-ecc or monocypher) that fits in the firmware's memory budget.

## Encryption

### Algorithm: AES-128-CTR

All radio payloads (bytes 8–63 of each packet) are encrypted after pairing is complete. See `protocol.md` for the packet structure.

- **Mode**: CTR (Counter Mode). A stream cipher mode that XORs plaintext with a keystream generated from the key and a nonce. No padding needed for our fixed-size payloads.
- **Key**: 128-bit, derived during pairing, stored in device flash.
- **Nonce construction**: `seq (4 bytes) || device_pair_id (12 bytes)` = 16-byte IV. Since `seq` is monotonically increasing and never reused within a session, nonce uniqueness is guaranteed.
- **Hardware acceleration**: ESP32-S3 includes an AES hardware accelerator, so encryption adds negligible latency (< 0.01 ms per packet).

### What is encrypted

| Component | Encrypted? | Reason |
|---|---|---|
| Packet header (bytes 0–7) | No | Receiver needs magic, version, type, seq before decryption |
| Packet payload (bytes 8–63) | Yes | Contains sensitive input data (keystrokes, mouse movement) |
| USB CDC traffic (daemon ↔ local firmware) | No | Local USB bus; encrypting adds latency for no security gain |
| Heartbeat payload | Yes | Prevents traffic analysis of heartbeat vs. input patterns |

### What an eavesdropper can see

Even with encryption active, an observer within radio range can determine:
- That two Omni-KVM devices are communicating (MAC addresses, magic byte)
- Packet timing and frequency (reveals whether the user is actively typing or idle)
- Packet types (header is plaintext), though heartbeats and input both look like 64-byte packets at the radio level

This is explicitly documented in `threat-model.md` as an accepted residual risk.

## Replay protection

Each packet carries a 32-bit sequence number in the plaintext header. The receiver maintains the highest sequence number seen from each peer and drops any packet with a sequence number less than or equal to the stored value.

This prevents an attacker from recording encrypted packets and replaying them later. Even though the attacker cannot read the contents, replaying old mouse movements or keystrokes could cause unintended actions.

The sequence counter wraps at 2^32 (~4 billion). At 1000 packets/second, this takes ~50 days of continuous use before wrapping. On wrap, both sides perform a brief re-key handshake to reset the counter and derive a fresh session key, preventing nonce reuse.

## Fail-safe behavior

Security-relevant fail-safe behaviors:

- **Link loss**: When the radio link drops, both sides immediately release all held modifier keys (Ctrl, Shift, Alt, GUI). This prevents a "stuck Ctrl" scenario where the user unknowingly sends Ctrl+key combinations after reconnection.
- **Firmware watchdog**: If the firmware hangs or enters an unexpected state, the hardware watchdog timer resets it within 2 seconds. After reset, the device re-establishes the encrypted link using stored keys (no re-pairing needed).
- **Daemon crash**: If the host daemon crashes, the firmware continues operating as a basic USB HID device. No input is forwarded to the peer, so the user retains local control. The daemon's auto-restart mechanism (OS login item / service) will restart it shortly.

## Firmware update security

Firmware updates are mediated by the host daemon, never received directly from the internet by the device. The update flow:

1. Daemon downloads a signed firmware binary from GitHub Releases.
2. Daemon verifies the binary's Ed25519 signature against a public key embedded in the daemon.
3. Daemon transfers the verified binary to the device over USB CDC.
4. Device writes the binary to its OTA partition, verifies the SHA-256 hash, and reboots into the new firmware.
5. If verification fails at any step, the device remains on its current firmware.

This chain ensures that:
- The device never touches the internet.
- A compromised update server cannot push unsigned firmware.
- A man-in-the-middle between the daemon and GitHub cannot inject malicious firmware (signature check).

The Ed25519 signing key pair is generated by the project maintainer. The private key is never committed to the repository. The public key is embedded in both the daemon binary and the firmware, allowing cross-verification.

## What this project does NOT protect against

These are documented in detail in `threat-model.md`, but summarized here:

- **Physical access to a running device**: If an attacker can physically access a device that is plugged into a computer, they can read USB traffic, dump firmware, or replace the device entirely. Physical security is the user's responsibility.
- **Compromised host computer**: If the computer running the daemon is compromised (malware, rootkit), the attacker already has access to all keyboard input before it reaches our system. Omni-KVM cannot add security to an already-compromised host.
- **Advanced traffic analysis**: Packet timing reveals user activity patterns. Mitigating this fully would require constant-rate dummy traffic, which increases power consumption and radio congestion. This is a deliberate trade-off for a personal-use device.
- **Hardware-level backdoors in the ESP32-S3**: See `threat-model.md` for a detailed discussion. Summary: no confirmed backdoors exist; the residual risk is accepted and documented.
- **Quantum computing**: AES-128 and Curve25519 are not quantum-resistant. This is irrelevant for the project's threat model (personal use, not state-secret protection) and noted for completeness.
