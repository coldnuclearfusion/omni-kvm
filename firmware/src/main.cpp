// ============================================================
// Omni-KVM Firmware — Phase 0: Hello World
// ============================================================
// Purpose: Verify that the dev environment, board, and upload
//          pipeline all work. If the onboard RGB LED blinks
//          green and "heartbeat" appears in the serial monitor,
//          everything is ready for Phase 1.
// ============================================================

#include <Arduino.h>

// The ESP32-S3-DevKitC-1 has a WS2812 RGB LED on GPIO 38.
// (The PCB silkscreen labels this as "RGB@IO38".)
static const uint8_t LED_PIN = 38;

// ── Setup: runs once at boot ──────────────────────────────
void setup() {
    // Initialize serial over USB CDC (see platformio.ini build_flags)
    Serial.begin(115200);

    // Wait briefly for the USB serial connection to establish
    delay(1000);

    // Announce ourselves
    Serial.println("========================================");
    Serial.println("  Omni-KVM Firmware v0.0.1 (Phase 0)");
    Serial.println("  Board: ESP32-S3-DevKitC-1-N8R8");
    Serial.println("========================================");
    Serial.println();
    Serial.println("If you see this, your dev environment works!");
    Serial.println("The onboard LED should be blinking green.");
    Serial.println();
}

// ── Loop: runs repeatedly after setup ─────────────────────
void loop() {
    // LED on: green (R=0, G=20, B=0) — low brightness to avoid blinding
    neopixelWrite(LED_PIN, 0, 20, 0);
    Serial.println("[heartbeat] LED ON");

    delay(500);

    // LED off
    neopixelWrite(LED_PIN, 0, 0, 0);
    Serial.println("[heartbeat] LED OFF");

    delay(500);
}
