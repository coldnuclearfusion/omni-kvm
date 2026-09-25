// ============================================================
// Radio link — ESP-NOW transport between the two boards
// ============================================================
// Both boards send MSG_HEARTBEAT every 100 ms and answer each other's
// heartbeats with MSG_HEARTBEAT_ACK, which gives a round-trip time
// (RTT) measurement. Input packets (keyboard/mouse) queued with
// sendToPeer() are delivered to the peer's input handler.
//
// Temporary until pairing (Phase 5): the first board heard sending a
// valid heartbeat becomes the peer, until reboot. There is no
// encryption or replay protection yet.
// ============================================================

#pragma once

#include <stdint.h>

namespace radio_link {

// Called with each input packet (64 bytes) received from the peer.
using PacketHandler = void (*)(const uint8_t *packet);

// Starts the radio and ESP-NOW. Call once from setup().
void begin(PacketHandler onInput);

// Handles received packets and sends queued ones. Call on every loop pass.
void update();

// True while the peer has been heard within the last 3 seconds.
bool isLinkUp();

// Queues a 64-byte protocol packet for the peer. Its header seq is
// replaced with this board's radio sequence number. Returns false if
// the link is down or the queue is full.
bool sendToPeer(const uint8_t *packet);

}  // namespace radio_link
