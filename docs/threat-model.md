# Threat Model

This document catalogs the threats considered during the design of Omni-KVM, states whether each is mitigated, and explains the rationale. It is intended to be read alongside `security.md`, which describes the mechanisms in detail. Mitigations marked **Planned** are designed but not built yet.

## Scope

Omni-KVM is a **personal-use hardware KVM** for sharing keyboard and mouse input between two computers over a dedicated 2.4 GHz radio link. The threat model assumes:

- The user is an individual (not an organization with nation-state adversaries).
- The physical environment is semi-trusted (home, office, dormitory, café).
- The computers themselves are reasonably secured (not already compromised).
- The attacker's goal is one or more of: eavesdropping on keystrokes, injecting false input, disrupting operation.

## Threat catalog

### T1 — Passive eavesdropping (keystroke capture)

**Threat**: An attacker within radio range (~30–100 m) captures encrypted packets and attempts to read keystroke content.

**Mitigation**: ChaCha20-Poly1305 authenticated encryption (256-bit key) on all radio packets, with a fresh random nonce per packet and a fresh session key per connection. Today the key is a locally generated development key, compiled into both boards and never committed. **Planned**: the key established via ECDH during pairing, never transmitted. Without the key, captured message data is indistinguishable from random data.

**Residual risk**: If ChaCha20-Poly1305 is broken (no known practical attack exists as of 2026), if the key leaks (today anyone who can read a board's flash has it, T10), or, once pairing exists, if the pairing exchange was observed AND the ECDH implementation has a flaw. **Risk level: very low.**

### T2 — Replay attack

**Threat**: Attacker records encrypted packets and retransmits them later to cause unintended keystrokes or mouse movements on the victim's computer.

**Mitigation**: Two layers. (1) Per-connection session keys: every link-up derives a fresh key from both boards' random nonces, so packets recorded in an earlier session fail authentication. (2) Within a session, monotonically increasing 32-bit sequence numbers: the receiver drops any authentic packet with seq ≤ last seen. The sequence number is in the header, which the authentication tag covers, so it cannot be changed without detection. Nothing special happens when a board's counter wraps (after 2^32 packets, ~50 days at 1000 packets/s): its packets are dropped as replays until the other board declares the link lost after 1 s and a new session starts. Replay protection holds; the cost is a short outage and a link loss (`security.md`, "Replay protection").

**Residual risk**: None known, assuming correct implementation. An earlier firmware reset the sequence check after 3 s of link loss (record, jam for 3 s, replay); session keys removed that gap. Both layers were exercised on hardware with a test build that replays recorded packets (within a session: dropped as a replay; into a later session: failed authentication). **Risk level: none (if implemented correctly).**

### T3 — Man-in-the-middle during pairing

**Threat**: Attacker intercepts the pairing exchange and establishes separate keys with each device, relaying traffic between them.

**Applies once pairing is built (Planned, Phase 5).** Today no key is exchanged over the radio: both boards are flashed with the same key.

**Mitigation** (planned): ECDH key exchange. The attacker would need to substitute their own public keys during the exchange. Since pairing will require a physical action on both devices within a 30-second window, the attacker must be physically present and act within that window. Additionally, a future enhancement can add numeric confirmation (both devices display a short code; user verifies they match).

**Residual risk**: Possible if the attacker is physically adjacent during the brief pairing window and the user does not verify device identity. Pairing is a one-time event in a presumably safe environment (user's own room). **Risk level: low.**

### T4 — Rogue device impersonation

**Threat**: Attacker builds or reprograms an ESP32-S3 to impersonate a legitimate Omni-KVM device and pairs with the victim's device.

**Mitigation**: Every packet must authenticate with the shared key (Poly1305 tag), and a board only accepts a peer whose handshake authenticates. Today a board takes as its peer the first board whose handshake authenticates after it starts, and then ignores every other MAC address until it restarts. A rogue device without the key can neither become the peer nor get a packet accepted. **Planned** with pairing: a physical action on the legitimate device, and MAC address filtering after pairing; since MAC addresses can be spoofed, packets must still authenticate.

**Residual risk**: If the attacker has the key (read from a board, T10), or steals the victim's physical device, replaces it with a rogue one, and the victim doesn't notice. This is a physical security issue, not a wireless protocol issue. **Risk level: very low for the target user profile.**

### T5 — Jamming / denial of service

**Threat**: Attacker floods the 2.4 GHz band with noise, preventing the two devices from communicating.

**Mitigation**: Not directly mitigable — any 2.4 GHz device is vulnerable to broadband jamming. The fail-safe system ensures graceful degradation: after 1 s without the other board (T_link), both computers are in split mode (S7), each pointer stays where it was (B4), and each board releases every key and button it held for the other computer (F8). User notifications are **Planned** (B9); today the daemons only log the link change.

**Residual risk**: Communication disrupted as long as jamming continues; input sent toward the other computer just before the link is declared lost may not arrive. No security breach — only availability is affected. **Risk level: low (jamming requires proximity and intent).**

### T6 — Traffic analysis (timing side-channel)

**Threat**: Attacker observes packet timing to infer user activity. For example: a burst of 8 short-interval packets likely corresponds to typing an 8-character password.

**Mitigation**: Heartbeat packets are sent at a constant 100 ms interval regardless of user activity, providing a baseline of traffic. Input packets occur on top of this baseline. However, input bursts are still distinguishable from idle periods. The message type is in the plaintext header, too, so an observer also sees focus switches (`MSG_VIEW` outside its once-a-second repeat) and which board forwards input (`MSG_KEY_STATE`, sent once a second by the forwarding side only).

**Full mitigation** would require constant-rate transmission (sending dummy packets to mask activity), which increases power consumption and radio channel usage. This is not implemented.

**Residual risk**: An attacker can determine when the user is actively typing/moving vs. idle, roughly estimate typing speed, and tell when the focus moves and which computer's devices are being forwarded. They cannot determine what is typed (content is encrypted). **Risk level: low — useful mainly for targeted surveillance, not opportunistic attacks.**

### T7 — Compromised host computer

**Threat**: Malware on one of the connected computers intercepts keystrokes before or after they pass through Omni-KVM.

**Mitigation**: **None. This is out of scope.** If the host is compromised, the attacker already has access to everything the user types, regardless of whether Omni-KVM exists. Our system cannot add security to an insecure host; it can only avoid making a secure host less secure.

**Risk level: N/A (out of scope).**

### T8 — Firmware supply chain attack

**Threat**: An attacker compromises the firmware binary (on GitHub, during download, or during transfer to the device) to inject malicious code.

**Mitigation**: Today there is no update path: the user builds the firmware from source and flashes it over the board's UART port. The cryptographic library is vendored after checking its release against the published SHA-512 (`firmware/lib/monocypher/`). **Planned** (Phase 6): updates through the daemon with Ed25519 signature verification. The daemon verifies the firmware signature before transferring it to the device. The signing private key is held offline by the maintainer. The device verifies a SHA-256 hash after writing.

**Residual risk**: Today, a compromised source repository or build toolchain. Once signed updates exist, a compromised signing key. Standard open-source supply chain risk, not unique to this project. **Risk level: very low.**

### T9 — ESP32-S3 hardware backdoor

**Threat**: Espressif (the chip manufacturer) has embedded a hardware-level backdoor that exfiltrates data or allows remote access.

**Assessment**: This is a supply chain trust question that applies to all commercial silicon. Relevant facts:

- ESP32-S3 has been analyzed by multiple independent security researchers.
- In March 2025, undocumented HCI commands were found in the ESP32 Bluetooth stack. Subsequent analysis determined these were debug/manufacturing commands, not remotely exploitable backdoors. Our project does not use Bluetooth.
- No confirmed instance of data exfiltration via ESP32 hardware has been published.
- ESP-NOW requires the Wi-Fi radio to run in station mode, but our firmware never associates with an access point: it never calls `WiFi.begin()` (the only path to `esp_wifi_connect()`), turns off the Arduino library's auto-reconnect (which would otherwise call `WiFi.begin()` on a disconnect event), and erases any network credentials left in flash by earlier firmware. Even if a backdoor existed in the Wi-Fi stack, it would need to autonomously establish an internet connection through an unknown access point — a detectable and unlikely operation.

**Mitigation**: Post-build verification. The user (or any reviewer) can monitor the device's radio emissions with a spectrum analyzer or packet sniffer to verify that only ESP-NOW traffic between the two boards is present (session handshakes are broadcast until a board has found its peer).

**Residual risk**: Undetected silicon-level backdoors are possible in theory for any chip from any manufacturer. This risk is accepted. **Risk level: very low (no evidence, high scrutiny, architectural mitigations in place).**

### T10 — Lost or stolen device

**Threat**: An attacker obtains one of the user's devices.

**Assessment**:
- The device contains: the firmware binary (open-source, not secret) with the 256-bit development key compiled in, in flash. It stores no peer MAC address: a board finds its peer at run time.
- Flash encryption is not enabled (`security.md`, principle 4), so the key can be read out of flash, over the board's UART port with esptool or with a JTAG debugger. This is a known limitation. With flash encryption (the ESP32-S3 supports it), extracting the key would require invasive hardware attacks (decapping the chip).
- Both boards share the key. With it, an attacker within radio range could impersonate either board to the other (MAC addresses can be spoofed), and decrypt recorded traffic (the session nonces travel in handshakes sealed with that key).

**Mitigation**: Physical proximity requirement. Recovery today is by hand: generate a new development key (`python tools/make_dev_key.py --force`) and reflash the remaining board and its replacement, so the lost board's key no longer works. **Planned**: flash encryption, and pairing, after which re-pairing the remaining device with a replacement invalidates the old key.

**Residual risk**: Moderate while flash encryption is disabled (today); very low once it is enabled. **Risk level: moderate today; low with the planned configuration.**

### T11 — Packet tampering (bit-flipping)

**Threat**: An attacker within radio range captures a packet, flips bits in the ciphertext, and retransmits it, hoping to change a keystroke or mouse movement without knowing the key. Against a cipher mode with no integrity check (such as plain AES-CTR, which the first version of this design used), this works: the protocol layout is public, so the attacker knows which bytes hold the keycode.

**Mitigation**: Poly1305 authentication tag (ChaCha20-Poly1305) over the ciphertext and the header. Any modified bit makes verification fail and the packet is dropped. Verified on hardware: packets sealed with a different key are rejected 100% of the time.

**Residual risk**: None beyond breaking ChaCha20-Poly1305 itself, provided nonces are never reused (they are random per packet). **Risk level: very low.**

## Summary matrix

| Threat | Mitigated? | Residual Risk |
|---|---|---|
| T1 Eavesdropping | Yes | Very low |
| T2 Replay | Yes (session keys + sequence numbers) | None |
| T3 MITM pairing | Planned (no pairing yet) | Low |
| T4 Rogue device | Yes | Very low |
| T5 Jamming | Graceful degradation | Low |
| T6 Traffic analysis | Partial | Low |
| T7 Compromised host | Out of scope | N/A |
| T8 Supply chain | Planned (signed updates; today built from source) | Very low |
| T9 Hardware backdoor | Accepted | Very low |
| T10 Lost device | Planned (flash encryption, pairing) | Moderate today; low when built |
| T11 Packet tampering | Yes | Very low |
