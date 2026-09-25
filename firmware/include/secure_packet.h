// ============================================================
// Secure packet — ChaCha20-Poly1305 sealing for radio packets
// ============================================================
// Every packet sent over the radio is sealed: the first
// SEALED_DATA_SIZE bytes of the payload are encrypted, and a 16-byte
// tag authenticates both the ciphertext and the whole header, so any
// change to either is detected. Details: shared/protocol.md
// ("Encryption") and docs/security.md.
//
//   plaintext:   header(8) | message data(28) | zero padding(28)
//   on the air:  header(8) | nonce(12) | ciphertext(28) | tag(16)
//
// Cipher: ChaCha20-Poly1305 (IETF variant, RFC 8439) from Monocypher.
// ============================================================

#pragma once

#include <stdint.h>

// Cipher selection, set per build environment in platformio.ini.
// ChaCha20-Poly1305 is the real one. The others exist only for latency
// benchmarks and are NOT secure configurations:
//   NONE    — no encryption at all (baseline)
//   ESPNOW  — our layer does nothing; ESP-NOW's built-in CCMP encrypts
#define OMNI_CRYPTO_NONE        0
#define OMNI_CRYPTO_CHACHAPOLY  1
#define OMNI_CRYPTO_ESPNOW      2
#ifndef OMNI_RADIO_CRYPTO
#define OMNI_RADIO_CRYPTO OMNI_CRYPTO_CHACHAPOLY
#endif

namespace secure_packet {

constexpr unsigned KEY_SIZE = 32;   // 256-bit ChaCha20 key

// Human-readable name of the compiled-in cipher, for logs.
const char *modeName();

// Encrypts a 64-byte plaintext packet in place with `key` and sets
// FLAG_ENCRYPTED. Returns false if the message data doesn't fit in
// SEALED_DATA_SIZE bytes (the rest of the payload must be zero).
// Only call while the radio is on: the hardware random number
// generator used for nonces is only truly random while Wi-Fi runs.
bool seal(uint8_t *packet, const uint8_t key[KEY_SIZE]);

// Verifies and decrypts a sealed 64-byte packet in place with `key`,
// and clears FLAG_ENCRYPTED. Returns false if the tag doesn't match:
// the packet was altered, corrupted, or sealed with a different key.
bool open(uint8_t *packet, const uint8_t key[KEY_SIZE]);

}  // namespace secure_packet
