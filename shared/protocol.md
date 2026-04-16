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
8       4     timestamp   Local millisecond counter (uint32) — used to measure RTT
12      1     link_quality RSSI or link quality metric (uint8, 0–100)
13      51    (padding)   Zero-filled
```

Heartbeat is sent by both sides every 100 ms. If 30 consecutive heartbeats are missed (3 seconds), the link is declared down.

The timestamp field allows each side to compute round-trip time. It is not a synchronized clock — each side has its own epoch. RTT is measured by noting when a heartbeat was sent, and when the corresponding ACK returns.

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

When the `ENCRYPTED` flag (bit 2 of `flags`) is set:

- Bytes 0–7 (header) are sent **in plaintext**. The receiver needs the header to identify the message and sequence number before decryption.
- Bytes 8–63 (payload) are encrypted with AES-128-CTR using:
  - **Key**: The 128-bit shared secret established during pairing.
  - **Nonce/IV**: Constructed from the seq field (4 bytes) + a fixed device-pair identifier (12 bytes), totaling 16 bytes. Since seq is monotonically increasing and never reused, this guarantees a unique nonce per packet.

AES-128-CTR is chosen because:
- It is a stream mode — no padding issues with our fixed-size payloads.
- It is fast on ESP32 hardware (ESP32-S3 has an AES accelerator).
- Nonce construction from the sequence number is natural and prevents reuse.

The header being plaintext means an eavesdropper can see packet types and timing, but not content. This is an acceptable trade-off; see `threat-model.md` for analysis.

## Sequencing and reliability

ESP-NOW provides a basic ACK at the MAC layer, but we add application-level sequencing for:

1. **Replay protection**: Receiver maintains `last_seen_seq` per peer. Any packet with `seq <= last_seen_seq` is dropped.
2. **Ordering**: Input events must be applied in order. If packet N+1 arrives before N, the receiver buffers N+1 briefly (up to 5 ms) waiting for N. If N doesn't arrive, N+1 is applied and N is considered lost.
3. **Selective retransmit**: For critical messages (HANDOFF, PAIR_*, LOCK/UNLOCK), the sender sets `ACK_REQUESTED`. If no application-level ACK arrives within 10 ms, the sender retransmits up to 3 times.
4. **Input events are not retransmitted**: Mouse movements and key events are time-sensitive. A 10 ms-old mouse delta is worse than a dropped one (it causes a delayed "jump"). If lost, the next event naturally corrects the state. Modifier sync provides additional self-healing.

## Rate and bandwidth

At peak usage (fast mouse movement), the system generates roughly:
- Mouse: up to 1000 reports/second × 64 bytes = 64 KB/s
- Keyboard: typically < 50 events/second × 64 bytes = 3.2 KB/s

ESP-NOW theoretical throughput is ~1 Mbps. Our peak load of ~67 KB/s (~540 kbps) is well within budget, leaving room for heartbeats and control messages.

In practice, mouse reports will be batched: if multiple deltas accumulate between radio transmissions, they are summed into a single packet. The firmware targets a radio transmission interval of 1 ms, matching the USB HID polling rate.

## Future extensions

The `version` field and reserved bits in `flags` allow backward-compatible extensions:
- New message types can be added without changing existing ones.
- Payload formats can be extended by using currently-padded bytes.
- A future version bump signals incompatible changes.
