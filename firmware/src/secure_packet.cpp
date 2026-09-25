// ============================================================
// Secure packet — see include/secure_packet.h
// ============================================================

#include "secure_packet.h"

#include <stddef.h>
#include <string.h>
#include <esp_random.h>
#include "mbedtls/gcm.h"
#include "protocol.h"

namespace secure_packet {

static const size_t FLAGS_OFFSET = offsetof(proto::Header, flags);

static mbedtls_gcm_context gcm;

void begin(const uint8_t key[16]) {
    mbedtls_gcm_init(&gcm);
    mbedtls_gcm_setkey(&gcm, MBEDTLS_CIPHER_ID_AES, key, 128);
}

bool seal(uint8_t *packet) {
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

    // A fresh random nonce for every packet. GCM breaks down if a nonce is
    // ever reused with the same key; random nonces stay unique across
    // reboots and between the two boards, which share the key.
    esp_fill_random(nonce, proto::NONCE_SIZE);

    return mbedtls_gcm_crypt_and_tag(&gcm, MBEDTLS_GCM_ENCRYPT, sizeof(data),
                                     nonce, proto::NONCE_SIZE,
                                     packet, proto::HEADER_SIZE,   // authenticated, not encrypted
                                     data, ciphertext,
                                     proto::TAG_SIZE, tag) == 0;
}

bool open(uint8_t *packet) {
    if (!(packet[FLAGS_OFFSET] & proto::FLAG_ENCRYPTED)) return false;   // plaintext is never accepted

    uint8_t *payload = packet + proto::HEADER_SIZE;
    const uint8_t *nonce = payload;
    const uint8_t *ciphertext = nonce + proto::NONCE_SIZE;
    const uint8_t *tag = ciphertext + proto::SEALED_DATA_SIZE;

    uint8_t data[proto::SEALED_DATA_SIZE];
    if (mbedtls_gcm_auth_decrypt(&gcm, sizeof(data),
                                 nonce, proto::NONCE_SIZE,
                                 packet, proto::HEADER_SIZE,
                                 tag, proto::TAG_SIZE,
                                 ciphertext, data) != 0) {
        return false;
    }

    memset(payload, 0, proto::PAYLOAD_SIZE);
    memcpy(payload, data, sizeof(data));
    packet[FLAGS_OFFSET] &= ~proto::FLAG_ENCRYPTED;
    return true;
}

}  // namespace secure_packet
