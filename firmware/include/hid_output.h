// ============================================================
// HID output — turns protocol input events into USB HID reports
// ============================================================
// Events can arrive in bursts (from the radio, or from the PC in
// Phase 1 tests). Keyboard reports are therefore queued and sent at
// a paced rate by update(); see docs/architecture.md ("Pacing HID
// reports"). Mouse reports are sent immediately.
// ============================================================

#pragma once

#include <stdint.h>

namespace hid_output {

// Registers the keyboard + mouse. Call before USB.begin().
void begin();

// Sends at most one queued keyboard report. Call on every loop pass.
void update();

void keyDown(uint8_t keycode, uint8_t modifiers);
void keyUp(uint8_t keycode, uint8_t modifiers);
void syncModifiers(uint8_t modifiers);

void mouseMove(int16_t dx, int16_t dy, uint8_t buttons);
void mouseScroll(int16_t vertical, int16_t horizontal);

// Counters since boot, for MSG_DAEMON_STATUS.
struct Stats {
    uint32_t keyboardStalls = 0;    // times keyboard output had to wait for USB
    uint32_t mouseDropped = 0;      // mouse reports dropped: USB not ready
};
Stats stats();

}  // namespace hid_output
