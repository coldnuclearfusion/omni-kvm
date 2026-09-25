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
Bit 2: ENCRYPTED      — payload is sealed with ChaCha20-Poly1305 (see Encryption)
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
0x06   MSG_INPUT_ACK         sink → source   "Input packet <seq> processed"

0x10   MSG_HANDOFF           either → either Cursor control transfer request
0x11   MSG_HANDOFF_ACK       either → either Confirms handoff accepted

0x20   MSG_HEARTBEAT         both ↔ both     Keepalive ping
0x21   MSG_HEARTBEAT_ACK     both ↔ both     Keepalive pong
0x22   MSG_SESSION_HELLO     both ↔ both     Session handshake (see Sessions)

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

### MSG_INPUT_ACK (0x06)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       4     seq         Header seq of the input packet being acknowledged (uint32)
12      52    (padding)   Zero-filled
```

Sent by the receiving board after it has **processed** an input packet that carried `ACK_REQUESTED`. See [Sequencing and reliability](#sequencing-and-reliability).

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

Heartbeat is sent by both sides every 100 ms while a session is up (before that, `MSG_SESSION_HELLO` takes its place). If 30 consecutive heartbeats are missed (3 seconds), the link is declared down and the session dropped.

The timestamp field allows each side to compute round-trip time. It is not a synchronized clock — each side has its own epoch. The sender puts its own clock in `timestamp`; the peer's `MSG_HEARTBEAT_ACK` echoes that value back unchanged, so RTT = (sender's clock when the ACK arrives) − `timestamp`.

The counter is in microseconds because the expected RTT (2–4 ms) is too short to measure meaningfully in milliseconds. A uint32 microsecond counter wraps every ~71.6 minutes; unsigned subtraction still gives the right difference across a wrap, and only differences are ever used.

### MSG_SESSION_HELLO (0x22)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       12    nonce       Sender's handshake nonce (random)
20      12    echo        Latest nonce received from the peer (zeros if none)
32      1     flags       Bit 0: ESTABLISHED — sender has derived the session key
                          from (nonce, echo)
33      31    (padding)   Zero-filled
```

Sealed with the long-term key, never the session key. See [Sessions](#sessions).

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
                          0x02 = (reserved, see note) set encryption key
                          0x03 = request firmware version
                          0x04 = request link statistics → MSG_DAEMON_STATUS 0x03
                          0x05 = (reserved, see note) trigger pairing mode
                          0x06 = (retired: set radio TX window)
                          0x07 = development: set radio TX rate
                                 (data[0] = ESP-IDF wifi_phy_rate_t, e.g. 0x0A = 12 Mbps)
9       55    (data)      Sub-command-specific data
```

This is a local-only message (daemon ↔ its own firmware over USB CDC). It never goes over the radio.

**Note on 0x02 and 0x05**: `docs/security.md` principle 5 says pairing can only be started by the physical button, never by a software command, so that a compromised host cannot silently pair the device with an attacker's. A host that can set the key (0x02) or start pairing (0x05) would break that. Both stay unimplemented until the pairing design (Phase 5) settles this.

### MSG_DAEMON_STATUS (0x41)

```
Offset  Size  Field       Description
──────  ────  ──────────  ────────────────────────────────────────
8       1     status_id   Sub-status:
                          0x01 = peer MAC address (6 bytes follow)
                          0x02 = firmware version (semver string, max 16 bytes)
                          0x03 = link statistics (layout below)
                          0x04 = pairing state change
                          0x05 = link up / link down event
9       55    (data)      Sub-status-specific data
```

Also local-only (firmware → daemon over USB CDC).

**Link statistics (status 0x03)**, counters since boot, little-endian (`proto::LinkStats` in `firmware/include/protocol.h`; `tools/hid_test.py stats` and the daemon print them):

```
Offset  Size  Field               Description
──────  ────  ──────────────────  ────────────────────────────────────────
8       1     status_id           0x03
9       1     layout              Layout version of this block (currently 3)
10      1     link_up             1 while the radio link is up
11      1     phy_rate            Current radio TX rate (ESP-IDF wifi_phy_rate_t)
12      4     session             Session generation (0 = none yet)
16      4     host_received       Input packets from this board's host
20      4     host_dropped        ...not queued for the radio (link down, queue full)
24      4     radio_sent          Input packets handed to ESP-NOW, retransmissions included
28      4     radio_retransmits   Input packets sent again: no MSG_INPUT_ACK in time
32      4     radio_gave_up       Input packets never acknowledged after every attempt
36      4     radio_received      Authentic input packets from the peer
40      4     radio_rx_overflow   Radio packets lost: receive queue full
44      4     auth_failures       Radio packets whose tag did not verify
48      4     replays_dropped     Authentic packets with an old sequence number
52      4     frames_acked        Radio frames (incl. heartbeats) acknowledged by the peer's radio
56      4     frames_failed       ...not acknowledged, even after MAC retries
60      2     hid_stalls          Keyboard output had to wait for USB (saturates)
62      2     hid_mouse_dropped   Mouse reports dropped, USB not ready (saturates)
```

Readers must check `layout` and refuse unknown values rather than misread the counters.

## Encryption

Every packet sent over the radio is **sealed with ChaCha20-Poly1305** (IETF variant, RFC 8439), an authenticated encryption (AEAD) cipher: it hides the content *and* detects any modification. Packets without the `ENCRYPTED` flag, or whose tag does not verify, are dropped. The USB CDC channel between a daemon and its own firmware is not encrypted (local cable). The implementation is [Monocypher](https://monocypher.org) 4.0.3, vendored in `firmware/lib/monocypher/` with its provenance checks recorded there.

```
Plaintext (as built by the sender):
  Offset  Size  Field
  0       8     header
  8       28    message data (the payload definitions above)
  36      28    zero padding

Sealed (on the air):
  Offset  Size  Field       Notes
  0       8     header      plaintext, ENCRYPTED flag set; authenticated as AEAD "additional data"
  8       12    nonce       random, fresh for every packet
  20      28    ciphertext  the 28 bytes of message data, encrypted
  48      16    tag         Poly1305 authentication tag over header + ciphertext
```

- **Keys**: 256-bit. `MSG_SESSION_HELLO` is sealed with the long-term key; everything else with the current session key derived from it (see [Sessions](#sessions)). Until pairing exists (Phase 5), both boards are built with the same development long-term key from `firmware/include/secrets/dev_key.h`, generated by `tools/make_dev_key.py` and never committed.
- **Nonce**: 12 bytes from the ESP32-S3 hardware random number generator (truly random while Wi-Fi is on). ChaCha20-Poly1305 is catastrophically broken if a nonce repeats under the same key; random nonces stay unique across reboots and between the two boards, which share the key.
- **Message size limit**: sealed packets carry at most 28 bytes of message data. Every radio message defined above fits (the largest, `MSG_HANDOFF`, uses 9).
- **Header**: sent in plaintext because the receiver reads it first, but any change to it (type, flags, seq) makes the tag fail.

### Why not AES-128-CTR

v0.1 of this document specified AES-128-CTR with a nonce built from `seq` and a fixed device-pair ID. That design had two flaws:

1. **No integrity.** CTR XORs the plaintext with a keystream, so flipping a ciphertext bit flips the same plaintext bit. Since this layout is public, an attacker could turn one keycode into another without knowing the key.
2. **Nonce reuse after reboot.** `seq` restarts at 1 when a board reboots, so the same (key, nonce) pair would encrypt different packets. With CTR, XORing two such ciphertexts cancels the keystream and exposes the XOR of the plaintexts.

### Cipher choice (measured)

AES-128-GCM (mbedTLS, using the ESP32-S3 AES accelerator) was implemented first, in commit `c5a7abe`. Its cost turned out far above the < 0.01 ms originally assumed, so four options were benchmarked side by side in Phase 2: both boards ~30 cm apart, channel 1, ESP-NOW's default 1 Mbps, heartbeat RTT over ~12 s from both boards. `platformio.ini` keeps the `bench_none` and `bench_espnow` builds for future latency tests; the AES-GCM code is in commit `c5a7abe`.

| Radio encryption | RTT min | RTT mean | Seal / open per packet |
|---|---|---|---|
| None (baseline) | 2.81 ms | 3.22 ms | — |
| ESP-NOW built-in CCMP | 3.07 ms | 3.56 ms | in the Wi-Fi hardware |
| ChaCha20-Poly1305 (Monocypher) | 3.44 ms | 4.08 ms | 135 / 110 µs |
| AES-128-GCM (mbedTLS, hardware AES) | 3.98 ms | 4.99 ms | 207 / 220 µs |

- **ESP-NOW CCMP** costs almost no CPU; its +0.26 ms RTT matches the extra 16 bytes per frame at 1 Mbps, as predicted before measuring. It was still not chosen: its replay protection is described in a single sentence in Espressif's documentation and cannot be verified, it cannot encrypt broadcast frames, and it ties security to the radio layer (see `docs/security.md`).
- **ChaCha20-Poly1305** was chosen: about 0.2 ms faster per one-way trip than AES-GCM, a 256-bit key, and the same library (Monocypher) will also provide the key derivation for session keys and X25519 for pairing.
- The hardware AES accelerator does not help for 28-byte payloads: per-call setup dominates.

The header being plaintext means an eavesdropper can see packet types and timing, but not content. This is an acceptable trade-off; see `threat-model.md` for analysis.

## Sessions

Every radio connection uses its own **session key**. `MSG_SESSION_HELLO` packets are sealed with the long-term key; every other radio packet is sealed with the session key. Packets recorded during one session therefore fail authentication in any later session, even though sequence numbers start over.

**Handshake.** Each board keeps a random 12-byte *handshake nonce*. While it has no session, it sends `HELLO{nonce, echo = latest nonce seen from the peer}` every 100 ms (broadcast until it knows its peer). On receiving a HELLO, a board applies the first matching rule:

1. It matches the current session (`nonce` = the peer's session nonce, `echo` = ours): the peer is finishing the handshake. If the peer's `ESTABLISHED` flag is clear, repeat our confirmation `HELLO{our nonce, their nonce, ESTABLISHED}`; otherwise stay silent.
2. `echo` equals our current handshake nonce: the peer is live, holds the long-term key, and has seen our nonce. **Establish the session** from the two nonces, reply `HELLO{our nonce, their nonce, ESTABLISHED}`, and pick a new handshake nonce for any future session.
3. Otherwise the peer is starting a handshake: reply `HELLO{our handshake nonce, their nonce}`. A running session stays in use until a new one is established, so a replayed HELLO cannot tear it down.

```
  A                                               B
  HELLO{nA, echo: -}              ──────►         rule 3
  rule 2: session(nA, nB)         ◄──────         HELLO{nB, echo: nA}
  HELLO{nA, echo: nB, ESTABLISHED} ─────►         rule 2: session(nA, nB)
  (rule 1, silent)                ◄──────         HELLO{nB, echo: nA, ESTABLISHED}
```

A recorded HELLO carries an old nonce, which never matches a board's current handshake nonce, so it cannot complete a handshake.

**Key derivation.** `session_key = BLAKE2b-256(key = long-term key, message = "omni-kvm session v1" ‖ nonce₁ ‖ nonce₂)`, where nonce₁ belongs to the board with the lower MAC address, so both boards compute the same key. (Monocypher `crypto_blake2b_keyed`.)

**Lifetime.** A session ends when the link times out (3 s without an authentic session packet). If the peer restarts, its HELLOs start a new handshake through rule 3, and the surviving board switches to the new session as soon as it is established, without waiting for the timeout.

**Verified on hardware (Phase 2)**, using a test build (`-DOMNI_TEST_REPLAY=1`) in which one board records one of its own sealed heartbeats and re-sends it raw:

| Test | Expected | Observed on the other board |
|---|---|---|
| Replay within the same session | dropped by the sequence check | `replays dropped 1` |
| Replay after a restart, into a new session | dropped by authentication | `auth failures 1` |
| Peer restarts while the link is up | new session without a 3 s outage | `session N established, replacing a running one` |
| Different long-term key | no session ever forms | every HELLO fails authentication |

A board establishes its first session about 0.25 s after booting (Wi-Fi start-up included).

## Sequencing and reliability

ESP-NOW provides a basic ACK at the MAC layer, but we add application-level sequencing for:

1. **Replay protection**: Receiver maintains `last_seen_seq` per peer and session. Any authentic packet with `seq <= last_seen_seq` is dropped. The counter restarts only with a new session, whose new key makes packets from earlier sessions fail authentication (see [Sessions](#sessions)).
2. **Ordering**: Input events must be applied in order. If packet N+1 arrives before N, the receiver buffers N+1 briefly (up to 5 ms) waiting for N. If N doesn't arrive, N+1 is applied and N is considered lost.
3. **Acknowledged input**: key events, modifier syncs, and mouse moves that change the button state go out with `ACK_REQUESTED`. The receiving board applies the packet, then answers `MSG_INPUT_ACK{seq}`. The sender keeps at most one such packet outstanding and sends nothing queued after it until it is acknowledged, retransmitting it (with a new sequence number) if no acknowledgement arrives within 25 ms, up to 6 attempts in total. This preserves order, so the peer never sees "key up" before a late "key down", and a lost acknowledgement is harmless: the copy is applied again, and input messages carry state ("key A is down"), not toggles. The same mechanism will serve HANDOFF, PAIR_*, LOCK/UNLOCK.
4. **Plain mouse movement is not acknowledged or retransmitted**: a 10 ms-old delta is worse than a dropped one (it makes the pointer jump), and the next movement corrects the position anyway.

**Why end-to-end.** ESP-NOW already retries at the MAC layer and reports each frame as delivered or failed, but "delivered" only means the peer's radio received it. A first version resent key events when ESP-NOW reported a failure, matching reports to frames in send order; a key was still lost with every frame reported delivered. With `MSG_INPUT_ACK`, a line typed into a Mac at 24 Mbps from the worst spot, with 20% of frames failing, arrived intact after 27 retransmissions.

## Rate and bandwidth

At peak usage (fast mouse movement), the system generates roughly:
- Mouse: up to 1000 reports/second × 64 bytes = 64 KB/s
- Keyboard: typically < 50 events/second × 64 bytes = 3.2 KB/s

What matters is **airtime per packet**, not raw bit rate: every 64-byte packet also carries radio headers and is followed by a MAC-layer acknowledgement. At ESP-NOW's default 1 Mbps that is roughly 1.4 ms per packet (estimate), so 1000 packets/second could not fit at all. At the 24 Mbps we use (see [Radio settings](#radio-settings)), it is roughly 0.1 ms, so peak mouse traffic uses about 10% of the airtime and leaves room for heartbeats, control messages, and retries.

In practice, mouse reports will be batched: if multiple deltas accumulate between radio transmissions, they are summed into a single packet. The firmware targets a radio transmission interval of 1 ms, matching the USB HID polling rate.

**Keep batching windows short.** The receiving OS applies pointer acceleration to each report based on its size and the recent movement history, so one large delta does not move the pointer as far as the same motion delivered in small steps. Measured in Phase 1 on Windows with "Enhance pointer precision" on (the default), sending 150 counts per side of a square:

| Delivery | Pointer movement per side | Square closes? |
|---|---|---|
| One 150-count report per side | 236, 360, 360, 360 px | No, off by 124 px |
| 30 × 5-count reports, 8 ms apart | 123, 123, 123, 123 px | Yes, exactly |

A few deltas summed within ~1 ms is harmless, since a real mouse reports at that rate anyway. But a backlog that builds up during a radio stall must be replayed as small steps over time, not summed into one jump.

**Bursts from the host.** Measured in Phase 2 with the test tool sending a whole line of text (70–76 packets) or a mouse square (120 packets) at once, with the link statistics above read before and after:

- A 64-packet radio send queue silently dropped the tail of a 70-packet burst (the last three characters). Fixed: the queue holds 128 packets and the firmware stops reading from USB while it is full, so a burst waits in the 4 KB USB receive buffer instead.
- With both boards on one desk, over six runs (~1,600 input packets), one packet was lost in the air, and limiting frames in flight to 1 or 4 made no measurable difference. With one board next to a MacBook the picture changed completely (see [Radio settings](#radio-settings)), which led to acknowledged input (above) and a lower PHY rate.

## Radio settings

Both boards must use the same channel. Current defaults (in `firmware/src/radio_link.cpp`, overridable with build flags; the rate also at run time with `MSG_DAEMON_CMD` 0x07): **channel 6, PHY rate 12 Mbps**. The first experiment below picked 24 Mbps; the second one, next to a laptop, moved the default down.

They come from a Phase 2 jitter experiment: boards ~30 cm apart, ChaCha20-Poly1305 on, ~500 heartbeat RTT samples per configuration from both boards, changing one setting at a time. No heartbeat was lost in any configuration.

| Configuration | min | median | p90 | p99 | max | > 5 ms |
|---|---|---|---|---|---|---|
| E0 channel 1, 1 Mbps, defaults | 3.59 | 3.72 | 5.69 | 7.92 | 13.1 | 14% |
| E1 + non-blocking serial logging | 3.64 | 3.76 | 5.61 | 7.44 | 13.1 | 14% |
| E2 + Wi-Fi modem sleep off | 3.66 | 3.79 | 5.92 | 10.66 | 33.8 | 13% |
| E3 channel 6 (E1 settings) | 3.64 | 3.79 | 5.71 | 7.31 | 9.5 | 19% |
| E3 channel 11 (E1 settings) | 3.64 | 3.91 | 6.16 | 9.05 | 13.1 | 27% |
| E4 channel 6, 11 Mbps | 2.06 | 2.26 | 4.46 | 8.12 | 11.5 | 6% |
| **E4 channel 6, 24 Mbps** | **1.65** | **1.92** | **3.79** | **6.20** | **9.3** | **2%** |
| E4 channel 6, 54 Mbps | 1.63 | 1.79 | 3.90 | 6.28 | 11.2 | 3% |

RTT in ms. Findings:

- **PHY rate is the lever.** At 1 Mbps the RTT distribution has a second cluster about 2 ms above the first, which matches the cost of one retransmission at that rate; many "spikes" are collisions and retries. Faster rates shorten both the frame and each retry.
- **24 Mbps over 54 Mbps**: nearly the same median, fewer slow samples, and more range margin (higher rates need a stronger signal).
- **Blocking serial logs and Wi-Fi sleep were not the cause** of the spikes (both hypotheses rejected; sleep-off even looked worse in this run, but one run is not enough to say it hurts).
- **Channel differences were small** compared with the run-to-run variation on the same channel, and depend on the neighbours' Wi-Fi. Automatic channel selection is a possible future improvement.
- **Distance check**: with the boards ~1.5–1.8 m apart, line of sight, no obstacles, 300 samples from one board: no losses, min 1.49, median 1.58, p90 3.29, p99 5.44, max 7.4 ms, 2% over 5 ms, i.e. no worse than at 30 cm. Not yet tested: through walls or at longer range.

**Next to a laptop (Phase 3).** With board 1 on the Windows PC and board 2 on a MacBook Air ~1 m away, the link got much worse at 24 Mbps, and depended heavily on where board 2 lay. Frames failing (after MAC retries), board 1 → board 2:

| Board 2 placement | 24 Mbps | 11 Mbps | 6 Mbps | 1 Mbps |
|---|---|---|---|---|
| A: first spot, beside the MacBook | 1.3% (earlier), 78.9% (later) | 0% | 0% | 0% |
| B: away from the MacBook, a wired speaker between the boards | 9.2% | – | – | – |
| C: away from the MacBook, antenna turned toward board 1 | 12.3–99.5% | 0% | 0% | 0% |

The MacBook's own Wi-Fi was on 5 GHz (channel 40), so not a co-channel source. The physical layout mattered more than distance: a speaker in the line of sight, the orientation of the module's PCB antenna, possibly noise near the laptop. The same spot swung from 12% to 99% at 24 Mbps between runs.

Then, at spot C, one 60-character line (120 acknowledged input packets) per rate, measuring the average time per acknowledged packet (it includes the test tool's polling overhead, so compare rates, not absolute values):

| Rate | Time per acknowledged packet | Frames failed | Retransmissions | Given up |
|---|---|---|---|---|
| 24 Mbps | 9.97 ms | 20% | 27 | 0 |
| 18 Mbps | 3.99 ms | 0% | 0 | 0 |
| 12 Mbps | 4.30 ms | 0% | 0 | 0 |
| 11 Mbps | 4.55 ms | 0% | 0 | 0 |
| 6 Mbps | 4.55 ms | 0% | 0 | 0 |

All five lines arrived intact. 24 Mbps is the slowest here because of retransmissions. 18 Mbps was the sweet spot at this spot, but it is only one step below a rate that failed, so the default is **12 Mbps**: more signal margin for ~0.3 ms per packet. The sweet spot differs by place, so **automatic rate adaptation** (step down on failures, probe upward when clean, as Wi-Fi access points do) is the proper long-term answer.

## Future extensions

The `version` field and reserved bits in `flags` allow backward-compatible extensions:
- New message types can be added without changing existing ones.
- Payload formats can be extended by using currently-padded bytes.
- A future version bump signals incompatible changes.
