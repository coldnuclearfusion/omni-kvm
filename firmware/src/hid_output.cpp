// ============================================================
// HID output — see include/hid_output.h
// ============================================================

#include "hid_output.h"

#include <Arduino.h>
#include "USBHIDKeyboard.h"
#include "USBHIDMouse.h"
#include "usb_guard.h"

namespace hid_output {

// Minimum gap between keyboard reports. Reports ~1 ms apart came out
// garbled on Windows in Phase 1 testing (Korean IME input); 10 ms worked
// (docs/platform.md, section 5).
static const uint32_t KEY_REPORT_GAP_MS = 10;
static const size_t KEY_QUEUE_LEN = 128;

static USBHIDKeyboard Keyboard;
static USBHIDMouse Mouse;

// The keyboard and mouse share one USB HID interface. If it isn't ready
// (e.g. the host has suspended the device), the library drops reports
// without telling us, so we check first.
static USBHID hidInterface;
static Stats counters;
static bool keyboardStalled = false;

// keyState is the keyboard state the host should end up seeing. Every
// change queues a full snapshot of it; update() sends the snapshots one
// gap apart.
static KeyReport keyState = {};
static KeyReport lastSent = {};     // what the host last received
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
// Mouse reports are not queued: a late movement is worse than a lost one
// (it makes the pointer jump), so they are dropped, and counted, when the
// interface isn't ready.
// Ready for a report: the interface is, and the USB FIFOs are laid out
// safely (usb_guard.h).
static bool usbReady() {
    return hidInterface.ready() && usb_guard::fifosReady();
}

void mouseMove(int16_t dx, int16_t dy, uint8_t buttons) {
    if (!usbReady()) {
        counters.mouseDropped++;
        return;
    }
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
    if (!usbReady()) {
        counters.mouseDropped++;
        return;
    }
    while (vertical != 0 || horizontal != 0) {
        int8_t stepV = constrain(vertical, -127, 127);
        int8_t stepH = constrain(horizontal, -127, 127);
        Mouse.move(0, 0, stepV, stepH);
        vertical -= stepV;
        horizontal -= stepH;
    }
}

// ── Letting go without being told ─────────────────────────
// Windows opens the Start menu when a Windows key goes down and up with
// no other key in between. When the board lets go of one on its own, a
// Ctrl press goes first so that Windows does not take it for such a tap.
static const uint8_t LEFT_CTRL = 0x01;
static const uint8_t RIGHT_CTRL = 0x10;
static const uint8_t GUI_MODIFIERS = 0x08 | 0x80;   // left and right

// Lets go of keys until the state is `target`, which holds nothing that
// keyState does not.
static void letGoTo(const KeyReport &target) {
    if (memcmp(&target, &keyState, sizeof(target)) == 0) return;
    uint8_t freeCtrl = ~keyState.modifiers & (LEFT_CTRL | RIGHT_CTRL);
    if ((keyState.modifiers & ~target.modifiers & GUI_MODIFIERS) && freeCtrl) {
        keyState.modifiers |= (freeCtrl & LEFT_CTRL) ? LEFT_CTRL : RIGHT_CTRL;
        queueKeyState();
    }
    keyState = target;
    queueKeyState();
}

void releaseAll() {
    letGoTo(KeyReport{});
    if (mouseButtons != 0) mouseMove(0, 0, 0);
}

bool holding() {
    static const KeyReport none = {};
    return memcmp(&keyState, &none, sizeof(keyState)) != 0 || mouseButtons != 0;
}

void dropPending() {
    queueCount = 0;
    keyState = lastSent;
}

void hostLostState() {
    lastSent = KeyReport{};
    if (queueCount == 0 && memcmp(&keyState, &lastSent, sizeof(keyState)) != 0) queueKeyState();
}

void keepOnly(uint8_t modifiers, uint8_t buttons, const uint8_t keys[6]) {
    KeyReport target = keyState;
    target.modifiers &= modifiers;
    for (uint8_t &k : target.keys) {
        bool listed = false;
        for (int i = 0; i < 6; i++) listed = listed || keys[i] == k;
        if (!listed) k = 0;
    }
    letGoTo(target);
    uint8_t keep = mouseButtons & buttons;
    if (keep != mouseButtons) mouseMove(0, 0, keep);
}

// ── Setup and pacing ──────────────────────────────────────
void begin() {
    Keyboard.begin();
    Mouse.begin();
}

void update() {
    if (queueCount == 0) return;
    if (millis() - lastKeyReportMs < KEY_REPORT_GAP_MS) return;

    // Not ready: keep the report and try again on the next pass, so a
    // keystroke is delayed rather than lost.
    if (!usbReady()) {
        if (!keyboardStalled) counters.keyboardStalls++;
        keyboardStalled = true;
        return;
    }
    keyboardStalled = false;

    Keyboard.sendReport(&keyQueue[queueHead]);
    lastSent = keyQueue[queueHead];
    queueHead = (queueHead + 1) % KEY_QUEUE_LEN;
    queueCount--;
    lastKeyReportMs = millis();
}

bool allSent() {
    return queueCount == 0;
}

Stats stats() {
    return counters;
}

}  // namespace hid_output
