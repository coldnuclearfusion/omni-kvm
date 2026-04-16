# Threat Model

This document catalogs the threats considered during the design of Omni-KVM, states whether each is mitigated, and explains the rationale. It is intended to be read alongside `security.md`, which describes the mechanisms in detail.

## Scope

Omni-KVM is a **personal-use hardware KVM** for sharing keyboard and mouse input between two computers over a dedicated 2.4 GHz radio link. The threat model assumes:

- The user is an individual (not an organization with nation-state adversaries).
- The physical environment is semi-trusted (home, office, dormitory, café).
- The computers themselves are reasonably secured (not already compromised).
- The attacker's goal is one or more of: eavesdropping on keystrokes, injecting false input, disrupting operation.

## Threat catalog

### T1 — Passive eavesdropping (keystroke capture)

**Threat**: An attacker within radio range (~30–100 m) captures encrypted packets and attempts to read keystroke content.

**Mitigation**: AES-128-CTR encryption on all payloads. Key established via ECDH during pairing (never transmitted). Without the key, captured packets are indistinguishable from random data.

**Residual risk**: If AES-128 is broken (no known practical attack exists as of 2025), or if the pairing exchange was observed AND the ECDH implementation has a flaw. **Risk level: very low.**

### T2 — Replay attack

**Threat**: Attacker records encrypted packets and retransmits them later to cause unintended keystrokes or mouse movements on the victim's computer.

**Mitigation**: Monotonically increasing 32-bit sequence numbers. Receiver drops any packet with seq ≤ last seen. Re-key on sequence wrap.

**Residual risk**: None, assuming correct implementation. Sequence numbers are in the plaintext header, so the receiver can reject replays before attempting decryption. **Risk level: none (if implemented correctly).**

### T3 — Man-in-the-middle during pairing

**Threat**: Attacker intercepts the pairing exchange and establishes separate keys with each device, relaying traffic between them.

**Mitigation**: ECDH key exchange. The attacker would need to substitute their own public keys during the exchange. Since pairing requires physical button presses on both devices within a 30-second window, the attacker must be physically present and act within that window. Additionally, a future enhancement can add numeric confirmation (both devices display a short code; user verifies they match).

**Residual risk**: Possible if the attacker is physically adjacent during the brief pairing window and the user does not verify device identity. Pairing is a one-time event in a presumably safe environment (user's own room). **Risk level: low.**

### T4 — Rogue device impersonation

**Threat**: Attacker builds or reprograms an ESP32-S3 to impersonate a legitimate Omni-KVM device and pairs with the victim's device.

**Mitigation**: Pairing requires physical button press on the legitimate device. MAC address filtering after pairing. The attacker's rogue device cannot trigger the button press on the victim's real device.

**Residual risk**: If the attacker steals the victim's physical device, replaces it with a rogue one, and the victim doesn't notice. This is a physical security issue, not a wireless protocol issue. **Risk level: very low for the target user profile.**

### T5 — Jamming / denial of service

**Threat**: Attacker floods the 2.4 GHz band with noise, preventing the two devices from communicating.

**Mitigation**: Not directly mitigable — any 2.4 GHz device is vulnerable to broadband jamming. The fail-safe system ensures graceful degradation: heartbeat timeout triggers cursor recovery to a safe position, modifier keys are released, and the user is notified.

**Residual risk**: Communication disrupted as long as jamming continues. No data loss or security breach — only availability is affected. **Risk level: low (jamming requires proximity and intent).**

### T6 — Traffic analysis (timing side-channel)

**Threat**: Attacker observes packet timing to infer user activity. For example: a burst of 8 short-interval packets likely corresponds to typing an 8-character password.

**Mitigation**: Heartbeat packets are sent at a constant 100 ms interval regardless of user activity, providing a baseline of traffic. Input packets occur on top of this baseline. However, input bursts are still distinguishable from idle periods.

**Full mitigation** would require constant-rate transmission (sending dummy packets to mask activity), which increases power consumption and radio channel usage. This is not implemented.

**Residual risk**: An attacker can determine when the user is actively typing/moving vs. idle, and roughly estimate typing speed. They cannot determine what is typed (content is encrypted). **Risk level: low — useful mainly for targeted surveillance, not opportunistic attacks.**

### T7 — Compromised host computer

**Threat**: Malware on one of the connected computers intercepts keystrokes before or after they pass through Omni-KVM.

**Mitigation**: **None. This is out of scope.** If the host is compromised, the attacker already has access to everything the user types, regardless of whether Omni-KVM exists. Our system cannot add security to an insecure host; it can only avoid making a secure host less secure.

**Risk level: N/A (out of scope).**

### T8 — Firmware supply chain attack

**Threat**: An attacker compromises the firmware binary (on GitHub, during download, or during transfer to the device) to inject malicious code.

**Mitigation**: Ed25519 signature verification. The daemon verifies the firmware signature before transferring it to the device. The signing private key is held offline by the maintainer. The device verifies a SHA-256 hash after writing.

**Residual risk**: If the maintainer's signing key is compromised. Standard open-source supply chain risk, not unique to this project. **Risk level: very low.**

### T9 — ESP32-S3 hardware backdoor

**Threat**: Espressif (the chip manufacturer) has embedded a hardware-level backdoor that exfiltrates data or allows remote access.

**Assessment**: This is a supply chain trust question that applies to all commercial silicon. Relevant facts:

- ESP32-S3 has been analyzed by multiple independent security researchers.
- In March 2025, undocumented HCI commands were found in the ESP32 Bluetooth stack. Subsequent analysis determined these were debug/manufacturing commands, not remotely exploitable backdoors. Our project does not use Bluetooth.
- No confirmed instance of data exfiltration via ESP32 hardware has been published.
- Our firmware initializes Wi-Fi in ESP-NOW-only mode. Station mode (AP association) is never enabled. Even if a backdoor existed in the Wi-Fi stack, it would need to autonomously establish an internet connection through an unknown access point — a detectable and unlikely operation.

**Mitigation**: Post-build verification. The user (or any reviewer) can monitor the device's radio emissions with a spectrum analyzer or packet sniffer to verify that only ESP-NOW traffic to the known peer MAC address is present.

**Residual risk**: Undetected silicon-level backdoors are possible in theory for any chip from any manufacturer. This risk is accepted. **Risk level: very low (no evidence, high scrutiny, architectural mitigations in place).**

### T10 — Lost or stolen device

**Threat**: An attacker obtains one of the user's paired devices.

**Assessment**:
- The device contains: firmware binary (open-source, not secret), paired peer MAC address, and the AES-128 encryption key in flash.
- If flash encryption is enabled (ESP32-S3 supports this), extracting the key requires invasive hardware attacks (decapping the chip).
- If flash encryption is NOT enabled, the key can be read with a JTAG debugger. This is a known limitation.
- With the key and peer MAC, the attacker could impersonate the lost device to the remaining one. However, the remaining device will only communicate with the paired MAC address, and the attacker would need to be within radio range.

**Mitigation**: Flash encryption (enabled by default in production builds). Physical proximity requirement. User can re-pair the remaining device with a replacement, which invalidates the old key.

**Residual risk**: Moderate if flash encryption is disabled; very low if enabled. **Risk level: low with recommended configuration.**

## Summary matrix

| Threat | Mitigated? | Residual Risk |
|---|---|---|
| T1 Eavesdropping | Yes | Very low |
| T2 Replay | Yes | None |
| T3 MITM pairing | Mostly | Low |
| T4 Rogue device | Yes | Very low |
| T5 Jamming | Graceful degradation | Low |
| T6 Traffic analysis | Partial | Low |
| T7 Compromised host | Out of scope | N/A |
| T8 Supply chain | Yes | Very low |
| T9 Hardware backdoor | Accepted | Very low |
| T10 Lost device | Yes (with flash encryption) | Low |
