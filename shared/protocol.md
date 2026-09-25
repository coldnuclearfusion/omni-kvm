# Omni-KVM Radio Protocol v0.1

This document defines the binary format of every message exchanged between the two ESP32-S3 firmware instances over ESP-NOW. It is the single source of truth: both firmware and daemon code must conform to these definitions.

## Conventions

- All multi-byte integers are **little-endian** (native byte order for ESP32 and x86/ARM hosts).
- All packets are **exactly 64 bytes**, zero-padded if the payload is shorter.
- Keyboard key codes follow the **USB HID Usage Tables** (HID Usage ID, document version 1.4+). For example: A = 0x04, Enter = 0x28, Left Ctrl = 0xE0.
- Mouse coordinates are **relative (delta)**, matching standard USB HID mouse behavior.

## Packet structure

Every 64-byte packet shares a common 8-byte header:

```
Offset  Size  Field           Description
──────  ────  ──────────────  ─────────────────────────────────────────
0       1     magic           Fixed: 0x4B ('K' for KVM)
1       1     version         Protocol version. Currently 0x01.
2       1     msg_type        Message type (see table below)
3       1     flags           Bitfield (see below)
4       4     seq             Sequence number (monotonically increasing, wraps at 2^32)
──────  ────  ──────────────  ─────────────────────────────────────────
8       56    payload         Message-type-specific data (zero-padded)
```

### Header fields

**magic (1 byte):** Always `0x4B`. Packets with a different magic byte are silently dropped. This catches corrupted or foreign packets early.

**version (1 byte):** Currently `0x01`. If a device receives a version it does not understand, it replies with `MSG_VERSION_MISMATCH` and drops the packet. This allows future protocol evolution without bricking paired devices.

**msg_type (1 byte):** Identifies the payload format. See the message type table below.

**flags (1 byte):**
```
Bit 0: ACK_REQUESTED  — sender wants an explicit ACK for this packet
Bit 1: IS_ACK         — this packet is an ACK for the seq in the payload
Bit 2: ENCRYPTED      — payload bytes 8–63 are AES-128 encrypted
Bits 3–7: reserved (must be 0)
```

**seq (4 bytes, uint32, little-endian):** Monotonically increasing per-sender counter. Used for:
- Replay protection: receiver tracks the highest seen seq and rejects anything ≤ previous.
- ACK matching: an ACK carries the seq of the packet it acknowledges.
- Ordering: if two packets arrive out of order, seq resolves which is newer.

## Message types

```
Value  Name                  Direction       Description
─────  ────────────────────  ──────────────  ──────────────────────────────────
0x01   MSG_MOUSE_MOVE        source → sink   Relative mouse movement + buttons
0x02   MSG_MOUSE_SCROLL      source → sink   Scroll wheel event
0x03   MSG_KEY_DOWN          source → sink   Key press
0x04   MSG_KEY_UP            source → sink   Key release
0x05   MSG_MODIFIER_SYNC     source → sink   Full modifier state snapshot

0x10   MSG_HANDOFF           either → either Cursor control transfer request
0x11   MSG_HANDOFF_ACK       either → either Confirms handoff accepted

0x20   MSG_HEARTBEAT         both ↔ both     Keepalive ping
0x21   MSG_HEARTBEAT_ACK     both ↔ both     Keepalive pong

0x30   MSG_PAIR_REQUEST      either → either Initiate pairing
0x31   MSG_PAIR_CHALLENGE    either → either Pairing challenge (crypto)
0x32   MSG_PAIR_CONFIRM      either → either Pairing confirmation
0x33   MSG_PAIR_COMPLETE     either → either Pairing success

0x40   MSG_DAEMON_CMD        daemon → fw     Command from daemon to local firmware
0x41   MSG_DAEMON_STATUS     fw → daemon     Status report from firmware to daemon

0x50   MSG_LOCK              source → sink   Request input lock (full-screen app)
0x51   MSG_UNLOCK            source → sink   Release input lock

0xF0   MSG_VERSION_MISMATCH  either → either Protocol version incompatible
0xF1   MSG_ERROR             either → either Generic error report

0xFF   MSG_DFU_ENTER         daemon → fw     Enter firmware update mode
```

"source" = currently active side, "sink" = currently passive side. For symmetric messages (heartbeat, pairing), either side can send.

## Payload definitions

These describe the plaintext payload. On the radio, message data is limited to 28 bytes and travels encrypted; see [Encryption](#encryption).

### MSG_MOUSE_MOVE (0x01)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       2     dx          Relative X movement (int16, negative = left)
10      2     dy          Relative Y movement (int16, negative = up)
12      1     buttons     Bitfield: bit0=left, bit1=right, bit2=middle, bit3–4=side buttons
13      43    (padding)   Zero-filled
```

The dx/dy range of int16 (±32767) is more than sufficient; real deltas per report rarely exceed ±127. The extra range accommodates high-DPI mice and accumulated deltas when reports are batched.

### MSG_MOUSE_SCROLL (0x02)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       2     vertical    Scroll amount (int16, positive = up/away from user)
10      2     horizontal  Scroll amount (int16, positive = right)
12      52    (padding)   Zero-filled
```

### MSG_KEY_DOWN (0x03) / MSG_KEY_UP (0x04)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     keycode     USB HID Usage ID of the key
9       1     modifiers   Current modifier state (see modifier bitfield below)
10      54    (padding)   Zero-filled
```

Modifier bitfield (matches USB HID modifier byte):
```
Bit 0: Left Ctrl
Bit 1: Left Shift
Bit 2: Left Alt
Bit 3: Left GUI (Win key / Cmd key)
Bit 4: Right Ctrl
Bit 5: Right Shift
Bit 6: Right Alt
Bit 7: Right GUI
```

Sending the full modifier state with every key event (rather than relying on separate modifier key-down/key-up tracking) provides self-healing: if a modifier event is lost, the next key event corrects the state.

### MSG_MODIFIER_SYNC (0x05)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     modifiers   Current modifier state (same bitfield as above)
9       55    (padding)   Zero-filled
```

Sent periodically (every ~500 ms) and on every handoff to prevent stuck modifiers. Also sent when the link recovers from disconnection.

### MSG_HANDOFF (0x10)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     edge        Which edge the cursor exited from:
                          0x01=right, 0x02=left, 0x03=top, 0x04=bottom
9       1     reserved    0x00
10      2     entry_x     Suggested entry X on the receiving side (uint16, pixels)
12      2     entry_y     Suggested entry Y on the receiving side (uint16, pixels)
14      2     exit_y      Y coordinate where cursor left the source screen (uint16)
                          (useful if receiver wants to compute its own entry point)
16      1     modifiers   Current modifier state at the moment of handoff
17      47    (padding)   Zero-filled
```

The entry coordinates are **absolute pixel positions on the receiving side's monitor space**. They are computed by the sending daemon based on the negotiated virtual desktop layout. The receiving daemon may adjust them if the layout has changed since last negotiation.

### MSG_HANDOFF_ACK (0x11)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     accepted    0x01 = accepted, 0x00 = rejected (e.g. locked)
9       55    (padding)   Zero-filled
```

If rejected (e.g. full-screen lock active on the receiving side), the sending daemon keeps the cursor and shows a visual indicator to the user.

### MSG_HEARTBEAT (0x20) / MSG_HEARTBEAT_ACK (0x21)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       4     timestamp   Local microsecond counter (uint32) — used to measure RTT
12      1     link_quality RSSI or link quality metric (uint8, 0–100; 0 = not measured)
13      51    (padding)   Zero-filled
```

Heartbeat is sent by both sides every 100 ms. If 30 consecutive heartbeats are missed (3 seconds), the link is declared down.

The timestamp field allows each side to compute round-trip time. It is not a synchronized clock — each side has its own epoch. The sender puts its own clock in `timestamp`; the peer's `MSG_HEARTBEAT_ACK` echoes that value back unchanged, so RTT = (sender's clock when the ACK arrives) − `timestamp`.

The counter is in microseconds because the expected RTT (2–4 ms) is too short to measure meaningfully in milliseconds. A uint32 microsecond counter wraps every ~71.6 minutes; unsigned subtraction still gives the right difference across a wrap, and only differences are ever used.

### MSG_LOCK (0x50) / MSG_UNLOCK (0x51)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     reason      0x01 = full-screen app, 0x02 = user manual lock
9       55    (padding)   Zero-filled
```

When a side is locked, incoming handoff requests are rejected with `MSG_HANDOFF_ACK(accepted=0x00)`.

### MSG_DAEMON_CMD (0x40)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     cmd_id      Sub-command:
                          0x01 = request peer MAC address
                          0x02 = set encryption key (16 bytes follow)
                          0x03 = request firmware version
                          0x04 = request link statistics
                          0x05 = trigger pairing mode
9       55    (data)      Sub-command-specific data
```

This is a local-only message (daemon ↔ its own firmware over USB CDC). It never goes over the radio.

### MSG_DAEMON_STATUS (0x41)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     status_id   Sub-status:
                          0x01 = peer MAC address (6 bytes follow)
                          0x02 = firmware version (semver string, max 16 bytes)
                          0x03 = link statistics (packet counts, avg RTT)
                          0x04 = pairing state change
                          0x05 = link up / link down event
9       55    (data)      Sub-status-specific data
```

Also local-only (firmware → daemon over USB CDC).

## Encryption

Every packet sent over the radio is **sealed with AES-128-GCM**, an authenticated encryption mode: it hides the content *and* detects any modification. Packets without the `ENCRYPTED` flag, or whose tag does not verify, are dropped. The USB CDC channel between a daemon and its own firmware is not encrypted (local cable).

```
Plaintext (as built by the sender):
  Offset  Size  Field
  0       8     header
  8       28    message data (the payload definitions above)
  36      28    zero padding

Sealed (on the air):
  Offset  Size  Field       Notes
  0       8     header      plaintext, ENCRYPTED flag set; authenticated as GCM "additional data"
  8       12    nonce       random, fresh for every packet
  20      28    ciphertext  the 28 bytes of message data, encrypted
  48      16    tag         GCM authentication tag over header + ciphertext
```

- **Key**: 128-bit. Until pairing exists (Phase 5), both boards are built with the same development key from `firmware/include/secrets/dev_key.h`, generated by `tools/make_dev_key.py` and never committed.
- **Nonce**: 12 bytes from the ESP32-S3 hardware random number generator (truly random while Wi-Fi is on). GCM is catastrophically broken if a nonce repeats under the same key; random nonces stay unique across reboots and between the two boards, which share the key.
- **Message size limit**: sealed packets carry at most 28 bytes of message data. Every radio message defined above fits (the largest, `MSG_HANDOFF`, uses 9).
- **Header**: sent in plaintext because the receiver reads it first, but any change to it (type, flags, seq) makes the tag fail.

### Why not AES-128-CTR

v0.1 of this document specified AES-128-CTR with a nonce built from `seq` and a fixed device-pair ID. That design had two flaws:

1. **No integrity.** CTR XORs the plaintext with a keystream, so flipping a ciphertext bit flips the same plaintext bit. Since this layout is public, an attacker could turn one keycode into another without knowing the key.
2. **Nonce reuse after reboot.** `seq` restarts at 1 when a board reboots, so the same (key, nonce) pair would encrypt different packets. With CTR, XORing two such ciphertexts cancels the keystream and exposes the XOR of the plaintexts.

### Measured cost

Phase 2, ESP32-S3 at 240 MHz with the mbedTLS hardware-AES build: seal ≈ 215 µs, open ≈ 213 µs per packet, i.e. ≈ 0.43 ms added to each one-way trip, and heartbeat RTT rose from ~2.8 ms to ~4.0 ms minimum. This is well above the originally estimated < 0.01 ms, but within the latency budget. A software AEAD such as ChaCha20-Poly1305 may be faster for such small packets; to be measured.

The header being plaintext means an eavesdropper can see packet types and timing, but not content. This is an acceptable trade-off; see `threat-model.md` for analysis.

## Sequencing and reliability

ESP-NOW provides a basic ACK at the MAC layer, but we add application-level sequencing for:

1. **Replay protection**: Receiver maintains `last_seen_seq` per peer. Any authentic packet with `seq <= last_seen_seq` is dropped. Current limitation: after the link has been down (3 s without an authentic packet), the receiver accepts the peer's seq afresh so that a rebooted peer, which counts from 1 again, can reconnect. Per-connection session keys will remove this gap (see `docs/security.md`).
2. **Ordering**: Input events must be applied in order. If packet N+1 arrives before N, the receiver buffers N+1 briefly (up to 5 ms) waiting for N. If N doesn't arrive, N+1 is applied and N is considered lost.
3. **Selective retransmit**: For critical messages (HANDOFF, PAIR_*, LOCK/UNLOCK), the sender sets `ACK_REQUESTED`. If no application-level ACK arrives within 10 ms, the sender retransmits up to 3 times.
4. **Input events are not retransmitted**: Mouse movements and key events are time-sensitive. A 10 ms-old mouse delta is worse than a dropped one (it causes a delayed "jump"). If lost, the next event naturally corrects the state. Modifier sync provides additional self-healing.

## Rate and bandwidth

At peak usage (fast mouse movement), the system generates roughly:
- Mouse: up to 1000 reports/second × 64 bytes = 64 KB/s
- Keyboard: typically < 50 events/second × 64 bytes = 3.2 KB/s

ESP-NOW theoretical throughput is ~1 Mbps. Our peak load of ~67 KB/s (~540 kbps) is well within budget, leaving room for heartbeats and control messages.

In practice, mouse reports will be batched: if multiple deltas accumulate between radio transmissions, they are summed into a single packet. The firmware targets a radio transmission interval of 1 ms, matching the USB HID polling rate.

**Keep batching windows short.** The receiving OS applies pointer acceleration to each report based on its size and the recent movement history, so one large delta does not move the pointer as far as the same motion delivered in small steps. Measured in Phase 1 on Windows with "Enhance pointer precision" on (the default), sending 150 counts per side of a square:

| Delivery | Pointer movement per side | Square closes? |
|---|---|---|
| One 150-count report per side | 236, 360, 360, 360 px | No, off by 124 px |
| 30 × 5-count reports, 8 ms apart | 123, 123, 123, 123 px | Yes, exactly |

A few deltas summed within ~1 ms is harmless, since a real mouse reports at that rate anyway. But a backlog that builds up during a radio stall must be replayed as small steps over time, not summed into one jump.

## Future extensions

The `version` field and reserved bits in `flags` allow backward-compatible extensions:
- New message types can be added without changing existing ones.
- Payload formats can be extended by using currently-padded bytes.
- A future version bump signals incompatible changes.
