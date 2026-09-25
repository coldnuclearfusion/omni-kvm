// ============================================================
// Omni-KVM Protocol v0.1 — packet layout and constants
// ============================================================
// The specification lives in shared/protocol.md and is the single
// source of truth. Keep this file in sync with it.
//
// All multi-byte fields are little-endian. The ESP32-S3 is a
// little-endian CPU too, so a packet can be copied straight into
// these structs without any byte swapping.
// ============================================================

#pragma once

#include <stddef.h>
#include <stdint.h>

namespace proto {

constexpr uint8_t MAGIC = 0x4B;    // 'K' for KVM
constexpr uint8_t VERSION = 0x01;
constexpr size_t PACKET_SIZE = 64;
constexpr size_t HEADER_SIZE = 8;
constexpr size_t PAYLOAD_SIZE = PACKET_SIZE - HEADER_SIZE;

enum MsgType : uint8_t {
    MSG_MOUSE_MOVE       = 0x01,
    MSG_MOUSE_SCROLL     = 0x02,
    MSG_KEY_DOWN         = 0x03,
    MSG_KEY_UP           = 0x04,
    MSG_MODIFIER_SYNC    = 0x05,
    MSG_INPUT_ACK        = 0x06,   // receiver -> sender: "input packet <seq> processed"
    MSG_HANDOFF          = 0x10,
    MSG_HANDOFF_ACK      = 0x11,
    MSG_HEARTBEAT        = 0x20,
    MSG_HEARTBEAT_ACK    = 0x21,
    MSG_SESSION_HELLO    = 0x22,
    MSG_PAIR_REQUEST     = 0x30,
    MSG_PAIR_CHALLENGE   = 0x31,
    MSG_PAIR_CONFIRM     = 0x32,
    MSG_PAIR_COMPLETE    = 0x33,
    MSG_DAEMON_CMD       = 0x40,
    MSG_DAEMON_STATUS    = 0x41,
    MSG_LOCK             = 0x50,
    MSG_UNLOCK           = 0x51,
    MSG_VERSION_MISMATCH = 0xF0,
    MSG_ERROR            = 0xF1,
    MSG_DFU_ENTER        = 0xFF,
};

// Keyboard/mouse input: forwarded from the active side to the peer
// and injected there as HID reports.
inline bool isInputMessage(uint8_t msgType) {
    return msgType >= MSG_MOUSE_MOVE && msgType <= MSG_MODIFIER_SYNC;
}

// Header flags (bits 3–7 are reserved and must be 0)
constexpr uint8_t FLAG_ACK_REQUESTED = 1 << 0;
constexpr uint8_t FLAG_IS_ACK        = 1 << 1;
constexpr uint8_t FLAG_ENCRYPTED     = 1 << 2;

// Sealed (encrypted) radio packets — see shared/protocol.md "Encryption":
//   header(8) | nonce(12) | ciphertext(28) | tag(16)
constexpr size_t NONCE_SIZE = 12;
constexpr size_t TAG_SIZE = 16;
constexpr size_t SEALED_DATA_SIZE = PAYLOAD_SIZE - NONCE_SIZE - TAG_SIZE;   // 28

// "packed" tells the compiler not to insert padding bytes between
// fields, so the struct matches the byte layout in protocol.md.
struct __attribute__((packed)) Header {
    uint8_t magic;
    uint8_t version;
    uint8_t msg_type;
    uint8_t flags;
    uint32_t seq;
};
static_assert(sizeof(Header) == HEADER_SIZE, "Header must be 8 bytes");

struct __attribute__((packed)) MouseMove {
    int16_t dx;
    int16_t dy;
    uint8_t buttons;    // bit0=left, bit1=right, bit2=middle, bit3–4=side
};

struct __attribute__((packed)) MouseScroll {
    int16_t vertical;   // positive = up / away from the user
    int16_t horizontal; // positive = right
};

struct __attribute__((packed)) KeyEvent {
    uint8_t keycode;    // USB HID Usage ID
    uint8_t modifiers;  // full modifier state, USB HID modifier bitfield
};

struct __attribute__((packed)) ModifierSync {
    uint8_t modifiers;
};

// MSG_SESSION_HELLO: session handshake, sealed with the long-term key.
// See shared/protocol.md ("Sessions").
constexpr size_t SESSION_NONCE_SIZE = 12;
constexpr uint8_t HELLO_FLAG_ESTABLISHED = 1 << 0;   // sender has derived the session key

struct __attribute__((packed)) SessionHello {
    uint8_t nonce[SESSION_NONCE_SIZE];   // sender's handshake nonce
    uint8_t echo[SESSION_NONCE_SIZE];    // latest nonce received from the peer (zeros if none)
    uint8_t flags;
};
static_assert(sizeof(SessionHello) <= SEALED_DATA_SIZE, "HELLO must fit in a sealed packet");

// MSG_DAEMON_CMD / MSG_DAEMON_STATUS: between a host and its own board
// over USB CDC only, never over the radio (so not limited to 28 bytes).
constexpr uint8_t CMD_REQUEST_LINK_STATS = 0x04;
constexpr uint8_t CMD_SET_PHY_RATE = 0x07;      // development: data[0] = wifi_phy_rate_t
constexpr uint8_t STATUS_LINK_STATS = 0x03;
constexpr uint8_t LINK_STATS_LAYOUT = 3;        // bump whenever LinkStats changes

// Counters since boot. "host_*" are input packets from this board's host;
// "radio_*" are input packets over the radio.
struct __attribute__((packed)) LinkStats {
    uint8_t status_id;              // STATUS_LINK_STATS
    uint8_t layout;                 // LINK_STATS_LAYOUT, so readers can detect a mismatch
    uint8_t link_up;
    uint8_t phy_rate;               // current radio TX rate (wifi_phy_rate_t)
    uint32_t session;               // session generation (0 = none yet)
    uint32_t host_received;
    uint32_t host_dropped;          // not queued for the radio: link down, or queue full
    uint32_t radio_sent;            // input packets handed to ESP-NOW, retransmissions included
    uint32_t radio_retransmits;     // input packets sent again after a failed delivery
    uint32_t radio_gave_up;         // input packets still undelivered after every attempt
    uint32_t radio_received;        // authentic input packets from the peer
    uint32_t radio_rx_overflow;     // any packet dropped because the receive queue was full
    uint32_t auth_failures;
    uint32_t replays_dropped;
    uint32_t frames_acked;          // radio frames (heartbeats too) delivered at the MAC layer
    uint32_t frames_failed;         // ...not acknowledged, even after MAC retries
    uint16_t hid_stalls;            // keyboard output had to wait for USB (saturates)
    uint16_t hid_mouse_dropped;     // mouse reports dropped, USB not ready (saturates)
};
static_assert(sizeof(LinkStats) <= PAYLOAD_SIZE, "LinkStats must fit in a packet");
static_assert(sizeof(LinkStats) == 56, "Layout changed: bump LINK_STATS_LAYOUT and update the readers "
                                       "(tools/hid_test.py, daemon/src/protocol.rs)");

// MSG_INPUT_ACK: sent by the receiving board after it has processed an
// input packet that carried FLAG_ACK_REQUESTED.
struct __attribute__((packed)) InputAck {
    uint32_t seq;           // the acknowledged packet's header seq
};

// Used by both MSG_HEARTBEAT and MSG_HEARTBEAT_ACK. The ACK echoes the
// heartbeat's timestamp so the original sender can compute the RTT.
struct __attribute__((packed)) Heartbeat {
    uint32_t timestamp;     // sender's micros()
    uint8_t link_quality;   // 0 = not measured
};

}  // namespace proto
