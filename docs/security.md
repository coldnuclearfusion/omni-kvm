# Security Design

This document describes the security architecture of Omni-KVM. It covers the mechanisms we implement, the guarantees they provide, and their limits. For an explicit analysis of what attacks are and are not mitigated, see `threat-model.md`.

## Why security matters for this project

Omni-KVM transports raw keyboard and mouse input over a wireless radio link. Keyboard input includes passwords, credit card numbers, private messages, and shell commands. An insecure link would broadcast this data to anyone within radio range (~30–100 m depending on environment). Treating security as an afterthought would make this project dangerous to use.

## Security principles

1. **No internet exposure.** The ESP32-S3 firmware never connects to Wi-Fi access points or the internet. All radio activity is limited to ESP-NOW peer-to-peer communication with a single paired device. This eliminates an entire class of remote attacks.

2. **Encrypt by default.** Every radio packet is sealed with ChaCha20-Poly1305 (encrypted and authenticated). There is no "unencrypted mode": the firmware drops any radio packet that is not sealed with its key. Until pairing exists, development builds use a locally generated key that is never committed.

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

  Both sides store the 256-bit
  shared key and peer MAC address.
```

### Pairing security properties

- **Button press required on both sides**: An attacker cannot initiate pairing remotely. Both devices must be in pairing mode simultaneously, which requires physical access to both.
- **30-second window**: Pairing mode automatically exits after 30 seconds if not completed. This minimizes the window during which a device accepts unknown peers.
- **One peer only**: Each device stores exactly one paired peer. Pairing a new peer overwrites the old one. There is no "add another device" — this is a deliberate simplification that prevents confusion and reduces attack surface.
- **Visual feedback**: During pairing, the onboard LED blinks a distinct pattern so the user can confirm which devices are in pairing mode.

### Key derivation

The pairing process uses ECDH (Elliptic Curve Diffie-Hellman) on Curve25519 to establish a shared secret without either side transmitting the key itself. The shared secret is then passed through a key derivation function to derive the 256-bit encryption key. This means:

- Even if an attacker records the entire pairing exchange, they cannot derive the key without one side's private key.
- The key is never transmitted over the radio in any form.

Implementation note: Curve25519 (X25519) and the key derivation will use Monocypher, which the firmware already includes for the radio cipher.

## Encryption

### Algorithm: ChaCha20-Poly1305

Every radio packet is sealed with ChaCha20-Poly1305 (IETF variant, RFC 8439). See `protocol.md` ("Encryption") for the exact byte layout.

- **Mode**: an authenticated encryption (AEAD) cipher. It provides **confidentiality** (the payload is unreadable without the key) and **integrity** (a 16-byte Poly1305 tag covers the ciphertext and the plaintext header, so any altered bit is detected and the packet dropped).
- **Key**: 256-bit. Development builds use a key generated locally by `tools/make_dev_key.py` (git-ignored); pairing will derive it on the devices (Phase 5).
- **Nonce**: 12 random bytes from the hardware RNG, fresh for every packet and sent alongside it. Random nonces stay unique across reboots and between the two boards, which share one key.
- **Implementation**: Monocypher 4.0.3, a small audited C library, vendored unmodified in `firmware/lib/monocypher/`. Its release tarball was checked against the SHA-512 published on monocypher.org before vendoring (details in that folder's README). We do not implement any cryptographic primitive ourselves.
- **Cost**: measured ≈ 0.14 ms to seal and ≈ 0.10 ms to open a packet.

**Why ChaCha20-Poly1305.** Four options were benchmarked on the hardware (table in `protocol.md`): no encryption, ESP-NOW's built-in CCMP, ChaCha20-Poly1305, and AES-128-GCM via mbedTLS with the AES accelerator. ESP-NOW CCMP was fastest but rejected for the reasons discussed below. ChaCha20-Poly1305 beat AES-GCM by about 0.2 ms per one-way trip (the accelerator's per-call setup dominates for 28-byte payloads), and one library then covers the cipher, session key derivation, and pairing.

**Why not ESP-NOW's built-in encryption.** It encrypts in the Wi-Fi hardware with almost no CPU cost, but Espressif documents its replay protection in a single sentence, so we cannot verify the property we rely on most; it cannot encrypt broadcast frames; and it would tie security to the radio layer instead of our protocol layer.

**Why the design changed from AES-128-CTR.** The first version of this document specified AES-128-CTR with a nonce built from the sequence number. CTR alone has no integrity check, so an attacker could flip ciphertext bits to change keystrokes without knowing the key, and the sequence number restarts after a reboot, which would reuse nonces under the same key. `protocol.md` explains both flaws in detail.

**Verified on hardware (Phase 2)**, for both AES-GCM and the final ChaCha20-Poly1305 build: two boards with the same key link up with zero authentication failures; when one board is flashed with a different key, every packet from it fails authentication and the boards never accept each other as peers.

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

This prevents an attacker from recording encrypted packets and replaying them later. Even though the attacker cannot read the contents, replaying old mouse movements or keystrokes could cause unintended actions. The sequence number is in the header, which the authentication tag covers, so it cannot be altered to make an old packet look new.

**Known gap (current firmware).** When a board reboots, its sequence number starts again at 1. To let it reconnect, the receiver accepts the peer's sequence number afresh once the link has been down for 3 seconds. An attacker who records traffic, then jams the channel for 3 seconds, could replay the recording during that window.

**Planned fix: session keys.** On every link-up, each board contributes a fresh random value, and both derive a session key from the long-term key and the two values. Packets from an earlier session then fail authentication, so the sequence number can safely restart for each session.

The sequence counter wraps at 2^32 (~4 billion). At 1000 packets/second, this takes ~50 days of continuous use before wrapping. On wrap, both sides start a new session.

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
- **Quantum computing**: Curve25519 (planned for pairing) is not quantum-resistant; the 256-bit ChaCha20 key keeps a comfortable margin even against known quantum attacks on symmetric ciphers. This is irrelevant for the project's threat model (personal use, not state-secret protection) and noted for completeness.
