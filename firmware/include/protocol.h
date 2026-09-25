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
    MSG_HANDOFF          = 0x10,
    MSG_HANDOFF_ACK      = 0x11,
    MSG_HEARTBEAT        = 0x20,
    MSG_HEARTBEAT_ACK    = 0x21,
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

// Used by both MSG_HEARTBEAT and MSG_HEARTBEAT_ACK. The ACK echoes the
// heartbeat's timestamp so the original sender can compute the RTT.
struct __attribute__((packed)) Heartbeat {
    uint32_t timestamp;     // sender's micros()
    uint8_t link_quality;   // 0 = not measured
};

}  // namespace proto
