// ============================================================
// Radio link — ESP-NOW transport between the two boards
// ============================================================
// Until a session is up, each board sends MSG_SESSION_HELLO every
// 100 ms to agree on a fresh session key (see session.h). Then both
// send MSG_HEARTBEAT every 100 ms and answer each other's heartbeats
// with MSG_HEARTBEAT_ACK, which gives a round-trip time (RTT)
// measurement. Input packets (keyboard/mouse) queued with sendToPeer()
// are delivered to the peer's input handler, and daemon-to-daemon
// messages (proto::isRelayMessage) to its relay handler.
//
// Every radio packet is sealed (secure_packet.h); within a session, a
// packet with an old sequence number is dropped as a replay.
//
// Temporary until pairing (Phase 5): the first board heard sending an
// authentic HELLO becomes the peer, until reboot.
// ============================================================

#pragma once

#include <stddef.h>
#include <stdint.h>

namespace radio_link {

// Called with each input, relayed or MSG_HOST_GONE packet (64 bytes)
// received from the peer.
using PacketHandler = void (*)(const uint8_t *packet);

// Starts the radio and ESP-NOW. Call once from setup().
void begin(PacketHandler onInput, PacketHandler onRelay, PacketHandler onHostGone);

// Handles received packets and sends queued ones. Call on every loop pass.
void update();

// True while the peer has been heard within the last second (T_link).
bool isLinkUp();

// Queues a 64-byte protocol packet for the peer. Its header seq is
// replaced with this board's radio sequence number. Packets go out in
// order; a key event, mouse button change or relayed message whose
// delivery fails is sent again before anything queued after it. Returns
// false if the link is down, the queue is full, or the packet has data
// past what the radio carries (proto::fitsSealedData).
bool sendToPeer(const uint8_t *packet);

// Free slots in the send queue. Callers can stop reading input while it
// is zero instead of dropping packets.
size_t sendQueueSpace();

// Counters since boot, for MSG_DAEMON_STATUS.
struct Totals {
    uint32_t session;           // session generation
    uint32_t inputSent;
    uint32_t inputRetransmits;
    uint32_t inputGaveUp;
    uint32_t inputReceived;
    uint32_t rxOverflow;
    uint32_t authFailures;
    uint32_t replaysDropped;
    uint32_t framesAcked;
    uint32_t framesFailed;
    uint8_t phyRate;
};
Totals totals();

// Sets this board's radio TX rate (a wifi_phy_rate_t value). Returns
// false for rates we don't allow. Development knob for range experiments.
bool setPhyRate(uint8_t rate);

}  // namespace radio_link
