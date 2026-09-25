// ============================================================
// Session — a fresh key for every radio connection
// ============================================================
// Whenever the link comes up, the two boards exchange fresh random
// nonces in MSG_SESSION_HELLO packets (sealed with the long-term key)
// and both derive a session key from the long-term key and the two
// nonces. Every other radio packet is sealed with the session key, so
// packets recorded during an earlier session fail authentication in a
// later one. Protocol: shared/protocol.md ("Sessions").
//
// A session is only established when the peer echoes back the nonce we
// generated for this handshake, which proves it is live and holds the
// long-term key; a recorded old HELLO carries an old nonce and cannot.
// ============================================================

#pragma once

#include <stdint.h>
#include "protocol.h"
#include "secure_packet.h"

namespace session {

void begin(const uint8_t longTermKey[secure_packet::KEY_SIZE], const uint8_t selfMac[6]);

// The HELLO to send periodically while no session is up.
void makeHello(proto::SessionHello *out);

// Handles a HELLO from the peer, already verified with the long-term
// key. Returns true if `reply` should be sent back to the peer.
bool onHello(const uint8_t peerMac[6], const proto::SessionHello &hello,
             proto::SessionHello *reply);

bool isUp();

// Increases by one every time a new session is established.
uint32_t generation();

// Keys to seal/open packets with. sessionKey() is valid while isUp().
const uint8_t *longTermKey();
const uint8_t *sessionKey();

// The link was lost: forget the current session.
void drop();

}  // namespace session
