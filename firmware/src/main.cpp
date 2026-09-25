// ============================================================
// Omni-KVM Firmware — Phase 1: USB HID keyboard + mouse
// ============================================================
// Purpose: Make the board show up as a standard USB keyboard +
//          mouse on its "USB" port, with no driver install.
//          Pressing the BOOT button types a test line and moves
//          the mouse in a small square.
//
// Cables:  "UART" port -> PC (upload + serial monitor)
//          "USB"  port -> the computer to control (can be the same PC)
// ============================================================

#include <Arduino.h>
#include "USB.h"
#include "USBHIDKeyboard.h"
#include "USBHIDMouse.h"

// The ESP32-S3-DevKitC-1 has a WS2812 RGB LED on GPIO 38.
// (The PCB silkscreen labels this as "RGB@IO38".)
static const uint8_t LED_PIN = 38;

// The BOOT button is wired to GPIO 0. After boot it is an ordinary
// input: HIGH when released (internal pull-up), LOW when pressed.
static const uint8_t BOOT_BUTTON_PIN = 0;

static const uint32_t BLINK_INTERVAL_MS = 500;
static const uint32_t DEBOUNCE_MS = 50;
static const uint32_t KEY_HOLD_MS = 10;   // how long each key stays down
static const uint32_t KEY_GAP_MS = 10;    // pause between key events

USBHIDKeyboard Keyboard;
USBHIDMouse Mouse;

// ── Heartbeat LED (non-blocking) ──────────────────────────
// Phase 0 used delay(), which freezes the whole program. Here we
// check the clock on every loop pass and only toggle the LED once
// enough time has passed, so the loop can also watch the button.
static void updateHeartbeat() {
    static uint32_t lastToggle = 0;
    static bool ledOn = false;

    uint32_t now = millis();
    if (now - lastToggle < BLINK_INTERVAL_MS) return;
    lastToggle = now;

    ledOn = !ledOn;
    neopixelWrite(LED_PIN, 0, ledOn ? 20 : 0, 0);
}

// ── BOOT button (debounced) ───────────────────────────────
// Returns true once per press. Mechanical contacts "bounce" (flicker
// for a few ms) when pressed, so a change only counts after the
// reading has stayed the same for DEBOUNCE_MS.
static bool bootButtonPressed() {
    static bool stableState = HIGH;
    static bool lastReading = HIGH;
    static uint32_t lastChange = 0;

    bool reading = digitalRead(BOOT_BUTTON_PIN);
    if (reading != lastReading) {
        lastReading = reading;
        lastChange = millis();
    }
    if (millis() - lastChange < DEBOUNCE_MS || reading == stableState) return false;

    stableState = reading;
    return stableState == LOW;
}

// ── Typing like a person ──────────────────────────────────
// Keyboard.print() sends key-down and key-up reports ~1 ms apart,
// with Shift in the same report as the key. On Windows (Korean IME,
// Notepad) that came out garbled 3 times out of 3: dropped letters,
// Shift landing on the wrong keys, a lost Enter. Typing like a person
// (Shift first, keys held for a few ms) came out right 3 out of 3.
static bool needsShift(char c) {
    return isupper(c) || strchr("~!@#$%^&*()_+{}|:\"<>?", c) != nullptr;
}

static void typeLikeAPerson(const char *text) {
    for (const char *p = text; *p; p++) {
        bool shift = needsShift(*p);
        if (shift) {
            Keyboard.press(KEY_LEFT_SHIFT);
            delay(KEY_GAP_MS);
        }
        Keyboard.press(*p);
        delay(KEY_HOLD_MS);
        Keyboard.release(*p);   // also lifts Shift for shifted characters
        delay(KEY_GAP_MS);
    }
}

// ── HID demo ──────────────────────────────────────────────
static void runHidDemo() {
    Serial.println("[hid] typing test line");
    typeLikeAPerson("Hello from Omni-KVM!\n");

    Serial.println("[hid] moving mouse in a square");
    const int8_t step = 5;
    const int8_t directions[4][2] = { {step, 0}, {0, step}, {-step, 0}, {0, -step} };
    for (const auto &dir : directions) {
        for (int i = 0; i < 20; i++) {
            Mouse.move(dir[0], dir[1]);
            delay(10);
        }
    }
    Serial.println("[hid] done");
}

// ── Setup: runs once at boot ──────────────────────────────
void setup() {
    // UART0 -> "UART" port (see platformio.ini build_flags)
    Serial.begin(115200);
    pinMode(BOOT_BUTTON_PIN, INPUT_PULLUP);

    // Register keyboard + mouse, then start USB. The names show up
    // in Device Manager / System Information on the host.
    Keyboard.begin();
    Mouse.begin();
    USB.productName("Omni-KVM");
    USB.manufacturerName("Omni-KVM Project");
    USB.begin();

    Serial.println("========================================");
    Serial.println("  Omni-KVM Firmware v0.1.0 (Phase 1)");
    Serial.println("  Board: ESP32-S3-DevKitC-1-N8R8");
    Serial.println("========================================");
    Serial.println();
    Serial.println("Press BOOT to type a test line and move the mouse.");
    Serial.println();
}

// ── Loop: runs repeatedly after setup ─────────────────────
void loop() {
    updateHeartbeat();
    if (bootButtonPressed()) runHidDemo();
}
