// ============================================================
// Session — see include/session.h
// ============================================================

#include "session.h"

#include <string.h>
#include <esp_random.h>
#include "monocypher.h"

namespace session {

static const size_t N = proto::SESSION_NONCE_SIZE;
static const size_t KEY_SIZE = secure_packet::KEY_SIZE;
static const char KDF_LABEL[] = "omni-kvm session v1";

static uint8_t ltKey[KEY_SIZE];
static uint8_t selfMac[6];

// Our nonce for the next handshake. Replaced as soon as a session is
// established with it, so no nonce ever completes two handshakes.
static uint8_t handshakeNonce[N];

// The latest nonce the peer sent us, echoed in our periodic HELLOs.
static uint8_t peerNonceSeen[N];
static bool havePeerNonce = false;

static bool up = false;
static uint32_t gen = 0;
static uint8_t key[KEY_SIZE];
static uint8_t sessionSelfNonce[N];   // the nonces the current session key
static uint8_t sessionPeerNonce[N];   // was derived from

static bool equal(const uint8_t *a, const uint8_t *b) {
    return memcmp(a, b, N) == 0;
}

static bool isZero(const uint8_t *a) {
    for (size_t i = 0; i < N; i++) {
        if (a[i] != 0) return false;
    }
    return true;
}

static void fill(proto::SessionHello *out, const uint8_t *nonce, const uint8_t *echo, bool established) {
    memcpy(out->nonce, nonce, N);
    if (echo) {
        memcpy(out->echo, echo, N);
    } else {
        memset(out->echo, 0, N);
    }
    out->flags = established ? proto::HELLO_FLAG_ESTABLISHED : 0;
}

// session key = BLAKE2b-256, keyed with the long-term key, over a label
// and both nonces. Both boards must order the nonces the same way: the
// board with the lower MAC address goes first.
static void deriveKey(const uint8_t peerMac[6]) {
    bool selfFirst = memcmp(selfMac, peerMac, 6) < 0;
    const size_t labelLen = sizeof(KDF_LABEL) - 1;
    uint8_t message[labelLen + 2 * N];
    memcpy(message, KDF_LABEL, labelLen);
    memcpy(message + labelLen, selfFirst ? sessionSelfNonce : sessionPeerNonce, N);
    memcpy(message + labelLen + N, selfFirst ? sessionPeerNonce : sessionSelfNonce, N);
    crypto_blake2b_keyed(key, KEY_SIZE, ltKey, KEY_SIZE, message, sizeof(message));
    crypto_wipe(message, sizeof(message));
}

void begin(const uint8_t longTermKey[KEY_SIZE], const uint8_t mac[6]) {
    memcpy(ltKey, longTermKey, KEY_SIZE);
    memcpy(selfMac, mac, 6);
    esp_fill_random(handshakeNonce, N);
}

void makeHello(proto::SessionHello *out) {
    fill(out, handshakeNonce, havePeerNonce ? peerNonceSeen : nullptr, false);
}

bool onHello(const uint8_t peerMac[6], const proto::SessionHello &hello,
             proto::SessionHello *reply) {
    memcpy(peerNonceSeen, hello.nonce, N);
    havePeerNonce = true;

    // 1. The peer is finishing the handshake of our current session. If it
    //    hasn't established yet (our confirmation was lost), repeat it.
    if (up && equal(hello.nonce, sessionPeerNonce) && equal(hello.echo, sessionSelfNonce)) {
        if (hello.flags & proto::HELLO_FLAG_ESTABLISHED) return false;   // both done
        fill(reply, sessionSelfNonce, sessionPeerNonce, true);
        return true;
    }

    // 2. The peer echoed our handshake nonce: it is live, holds the key, and
    //    has seen our nonce. Establish the new session.
    if (!isZero(hello.echo) && equal(hello.echo, handshakeNonce)) {
        memcpy(sessionSelfNonce, handshakeNonce, N);
        memcpy(sessionPeerNonce, hello.nonce, N);
        deriveKey(peerMac);
        up = true;
        gen++;
        esp_fill_random(handshakeNonce, N);     // never reuse it for another session
        fill(reply, sessionSelfNonce, sessionPeerNonce, true);
        return true;
    }

    // 3. A new handshake from the peer (it booted, or lost the link). Answer
    //    with our handshake nonce. A current session stays in use until the
    //    new one is established, so a replayed HELLO cannot tear it down.
    fill(reply, handshakeNonce, hello.nonce, false);
    return true;
}

bool isUp() {
    return up;
}

uint32_t generation() {
    return gen;
}

const uint8_t *longTermKey() {
    return ltKey;
}

const uint8_t *sessionKey() {
    return key;
}

void drop() {
    up = false;
    crypto_wipe(key, sizeof(key));
}

}  // namespace session
