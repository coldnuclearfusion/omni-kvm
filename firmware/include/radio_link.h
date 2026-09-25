// ============================================================
// Radio link — ESP-NOW transport between the two boards
// ============================================================
// Phase 2 step 1: both boards send MSG_HEARTBEAT every 100 ms and
// answer each other's heartbeats with MSG_HEARTBEAT_ACK, which gives
// a round-trip time (RTT) measurement. Stats go to the debug log.
//
// Temporary until pairing (Phase 5): the first board heard sending a
// valid heartbeat becomes the peer, until reboot. There is no
// encryption or replay protection yet.
// ============================================================

#pragma once

namespace radio_link {

// Starts the radio and ESP-NOW. Call once from setup().
void begin();

// Handles received packets and sends heartbeats. Call on every loop pass.
void update();

// True while the peer has been heard within the last 3 seconds.
bool isLinkUp();

}  // namespace radio_link
