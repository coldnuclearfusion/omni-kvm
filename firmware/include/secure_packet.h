// ============================================================
// Secure packet — AES-128-GCM sealing for radio packets
// ============================================================
// Every packet sent over the radio is sealed: the first
// SEALED_DATA_SIZE bytes of the payload are encrypted, and a 16-byte
// tag authenticates both the ciphertext and the whole header, so any
// change to either is detected. Details: shared/protocol.md
// ("Encryption") and docs/security.md.
//
//   plaintext:   header(8) | message data(28) | zero padding(28)
//   on the air:  header(8) | nonce(12) | ciphertext(28) | tag(16)
// ============================================================

#pragma once

#include <stdint.h>

namespace secure_packet {

// Sets the 128-bit key. Call once, after the radio is on (the hardware
// random number generator is only truly random while Wi-Fi is running).
void begin(const uint8_t key[16]);

// Encrypts a 64-byte plaintext packet in place and sets FLAG_ENCRYPTED.
// Returns false if the message data doesn't fit in SEALED_DATA_SIZE
// bytes (the rest of the payload must be zero).
bool seal(uint8_t *packet);

// Verifies and decrypts a sealed 64-byte packet in place, and clears
// FLAG_ENCRYPTED. Returns false if the tag doesn't match: the packet
// was altered, corrupted, or sealed with a different key.
bool open(uint8_t *packet);

}  // namespace secure_packet
