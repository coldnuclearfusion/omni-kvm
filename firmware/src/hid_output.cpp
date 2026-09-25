// ============================================================
// HID output — see include/hid_output.h
// ============================================================

#include "hid_output.h"

#include <Arduino.h>
#include "USBHIDKeyboard.h"
#include "USBHIDMouse.h"

namespace hid_output {

// Minimum gap between keyboard reports. Reports ~1 ms apart came out
// garbled on Windows in Phase 1 testing; 10 ms worked. The real
// minimum is still to be measured against the latency budget.
static const uint32_t KEY_REPORT_GAP_MS = 10;
static const size_t KEY_QUEUE_LEN = 128;

static USBHIDKeyboard Keyboard;
static USBHIDMouse Mouse;

// keyState is the keyboard state the host should end up seeing. Every
// change queues a full snapshot of it; update() sends the snapshots one
// gap apart.
static KeyReport keyState = {};
static KeyReport keyQueue[KEY_QUEUE_LEN];
static size_t queueHead = 0;
static size_t queueCount = 0;
static uint32_t lastKeyReportMs = 0;

static uint8_t mouseButtons = 0;

// ── Keyboard ──────────────────────────────────────────────
static void queueKeyState() {
    if (queueCount == KEY_QUEUE_LEN) {
        // Full: replace the newest snapshot instead of dropping this one.
        // Snapshots are full states, so the host still ends up in the
        // right state (no stuck keys); only some keystrokes are lost.
        keyQueue[(queueHead + queueCount - 1) % KEY_QUEUE_LEN] = keyState;
        return;
    }
    keyQueue[(queueHead + queueCount) % KEY_QUEUE_LEN] = keyState;
    queueCount++;
}

static void setModifiers(uint8_t modifiers) {
    if (keyState.modifiers == modifiers) return;
    keyState.modifiers = modifiers;
    queueKeyState();
}

// Modifier keys (0xE0–0xE7) travel in the modifiers byte, not as keys.
static bool isModifierKey(uint8_t keycode) {
    return keycode >= 0xE0 && keycode <= 0xE7;
}

void keyDown(uint8_t keycode, uint8_t modifiers) {
    setModifiers(modifiers);    // modifiers first, in their own report
    if (keycode == 0 || isModifierKey(keycode)) return;

    for (uint8_t k : keyState.keys) {
        if (k == keycode) return;   // already down
    }
    for (uint8_t &k : keyState.keys) {
        if (k == 0) {
            k = keycode;
            queueKeyState();
            return;
        }
    }
    // A HID keyboard report holds at most 6 keys; extra keys are ignored.
}

void keyUp(uint8_t keycode, uint8_t modifiers) {
    bool changed = false;
    for (uint8_t &k : keyState.keys) {
        if (k == keycode) {
            k = 0;
            changed = true;
        }
    }
    if (changed) queueKeyState();
    setModifiers(modifiers);    // modifiers last, after the key is up
}

void syncModifiers(uint8_t modifiers) {
    setModifiers(modifiers);
}

// ── Mouse ─────────────────────────────────────────────────
void mouseMove(int16_t dx, int16_t dy, uint8_t buttons) {
    buttons &= MOUSE_ALL;
    if (buttons != mouseButtons) {
        Mouse.release(mouseButtons & ~buttons);
        Mouse.press(buttons & ~mouseButtons);
        mouseButtons = buttons;
    }
    // HID mouse reports carry 8-bit deltas; split larger moves.
    while (dx != 0 || dy != 0) {
        int8_t stepX = constrain(dx, -127, 127);
        int8_t stepY = constrain(dy, -127, 127);
        Mouse.move(stepX, stepY);
        dx -= stepX;
        dy -= stepY;
    }
}

void mouseScroll(int16_t vertical, int16_t horizontal) {
    while (vertical != 0 || horizontal != 0) {
        int8_t stepV = constrain(vertical, -127, 127);
        int8_t stepH = constrain(horizontal, -127, 127);
        Mouse.move(0, 0, stepV, stepH);
        vertical -= stepV;
        horizontal -= stepH;
    }
}

// ── Setup and pacing ──────────────────────────────────────
void begin() {
    Keyboard.begin();
    Mouse.begin();
}

void update() {
    if (queueCount == 0) return;
    if (millis() - lastKeyReportMs < KEY_REPORT_GAP_MS) return;

    Keyboard.sendReport(&keyQueue[queueHead]);
    queueHead = (queueHead + 1) % KEY_QUEUE_LEN;
    queueCount--;
    lastKeyReportMs = millis();
}

}  // namespace hid_output
