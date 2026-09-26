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

// True once every queued keyboard report has reached the host.
bool allSent();

void keyDown(uint8_t keycode, uint8_t modifiers);
void keyUp(uint8_t keycode, uint8_t modifiers);
void syncModifiers(uint8_t modifiers);

void mouseMove(int16_t dx, int16_t dy, uint8_t buttons);
void mouseScroll(int16_t vertical, int16_t horizontal);

// Lets go of every key, modifier and mouse button, e.g. when the peer
// that pressed them is gone and its "key up" can never arrive. A
// Windows/Command key let go of this way (or by keepOnly) gets a Ctrl
// press first, so that Windows does not take it for a lone Windows-key
// tap and open the Start menu; the Ctrl goes up with the rest.
void releaseAll();

// The host let go of everything: the device was disconnected from it.
// The keyboard state it should have goes to it again once it is back;
// queued reports stay queued (delayed, not lost), and mouse buttons come
// back with the next mouse report.
void hostLostState();

// Drops keyboard reports not sent yet; the host keeps the state of the
// last one it received, which releaseAll() then starts from.
void dropPending();

// Lets go of every key, modifier and button not listed; never presses
// anything (MSG_KEY_STATE).
void keepOnly(uint8_t modifiers, uint8_t buttons, const uint8_t keys[6]);

// Counters since boot, for MSG_DAEMON_STATUS.
struct Stats {
    uint32_t keyboardStalls = 0;    // times keyboard output had to wait for USB
    uint32_t mouseDropped = 0;      // mouse reports dropped: USB not ready
};
Stats stats();

}  // namespace hid_output
