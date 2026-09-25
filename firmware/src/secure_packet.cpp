// ============================================================
// Secure packet — see include/secure_packet.h
// ============================================================

#include "secure_packet.h"

#include <stddef.h>
#include <string.h>
#include <esp_random.h>
#include "monocypher.h"
#include "protocol.h"

#if OMNI_RADIO_CRYPTO != OMNI_CRYPTO_CHACHAPOLY
#warning "Benchmark build: secure_packet does not encrypt. Never use for real input."
#endif

namespace secure_packet {

static const size_t FLAGS_OFFSET = offsetof(proto::Header, flags);

static uint8_t key[KEY_SIZE];

const char *modeName() {
    switch (OMNI_RADIO_CRYPTO) {
        case OMNI_CRYPTO_NONE:       return "none (benchmark)";
        case OMNI_CRYPTO_CHACHAPOLY: return "ChaCha20-Poly1305";
        case OMNI_CRYPTO_ESPNOW:     return "ESP-NOW CCMP (benchmark)";
        default:                     return "unknown";
    }
}

void begin(const uint8_t newKey[KEY_SIZE]) {
    memcpy(key, newKey, KEY_SIZE);
}

bool seal(uint8_t *packet) {
#if OMNI_RADIO_CRYPTO != OMNI_CRYPTO_CHACHAPOLY
    return true;    // benchmark: leave the packet as plaintext
#else
    uint8_t *payload = packet + proto::HEADER_SIZE;
    for (size_t i = proto::SEALED_DATA_SIZE; i < proto::PAYLOAD_SIZE; i++) {
        if (payload[i] != 0) return false;
    }
    packet[FLAGS_OFFSET] |= proto::FLAG_ENCRYPTED;   // set before sealing: the header is authenticated

    uint8_t data[proto::SEALED_DATA_SIZE];
    memcpy(data, payload, sizeof(data));

    uint8_t *nonce = payload;
    uint8_t *ciphertext = nonce + proto::NONCE_SIZE;
    uint8_t *tag = ciphertext + proto::SEALED_DATA_SIZE;

    // A fresh random nonce for every packet. AEAD ciphers break down if a
    // nonce is ever reused with the same key; random nonces stay unique
    // across reboots and between the two boards, which share the key.
    esp_fill_random(nonce, proto::NONCE_SIZE);

    crypto_aead_ctx ctx;
    crypto_aead_init_ietf(&ctx, key, nonce);
    crypto_aead_write(&ctx, ciphertext, tag, packet, proto::HEADER_SIZE,
                      data, sizeof(data));
    crypto_wipe(&ctx, sizeof(ctx));
    crypto_wipe(data, sizeof(data));
    return true;
#endif
}

bool open(uint8_t *packet) {
#if OMNI_RADIO_CRYPTO != OMNI_CRYPTO_CHACHAPOLY
    return !(packet[FLAGS_OFFSET] & proto::FLAG_ENCRYPTED);   // benchmark: plaintext only
#else
    if (!(packet[FLAGS_OFFSET] & proto::FLAG_ENCRYPTED)) return false;   // plaintext is never accepted

    uint8_t *payload = packet + proto::HEADER_SIZE;
    const uint8_t *nonce = payload;
    const uint8_t *ciphertext = nonce + proto::NONCE_SIZE;
    const uint8_t *tag = ciphertext + proto::SEALED_DATA_SIZE;

    uint8_t data[proto::SEALED_DATA_SIZE];
    crypto_aead_ctx ctx;
    crypto_aead_init_ietf(&ctx, key, nonce);
    int result = crypto_aead_read(&ctx, data, tag, packet, proto::HEADER_SIZE,
                                  ciphertext, sizeof(data));
    crypto_wipe(&ctx, sizeof(ctx));
    if (result != 0) return false;   // tag mismatch: altered, corrupted, or another key

    memset(payload, 0, proto::PAYLOAD_SIZE);
    memcpy(payload, data, sizeof(data));
    crypto_wipe(data, sizeof(data));
    packet[FLAGS_OFFSET] &= ~proto::FLAG_ENCRYPTED;
    return true;
#endif
}

}  // namespace secure_packet
