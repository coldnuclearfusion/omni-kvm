// ============================================================
// Omni-KVM Firmware — Phase 2 (in progress): radio link
// ============================================================
// The "USB" port is a composite device: HID keyboard + mouse, plus a
// CDC serial channel for the host daemon. Both boards run this same
// firmware:
//
//   host ──CDC──▶ this board ══ESP-NOW══▶ peer board ──HID──▶ peer's host
//
// Input packets (shared/protocol.md) from the host are forwarded to
// the peer over the radio; input packets from the peer are injected
// into this board's host as keyboard/mouse reports. Until the daemon
// exists, tools/hid_test.py plays the host's part.
//
// LED: green = no peer, blue = radio link up.
//
// Cables:  "UART" port -> PC (upload + debug log)
//          "USB"  port -> the computer to control (can be the same PC)
// ============================================================

#include <Arduino.h>
#include "USB.h"
#include "hid_output.h"
#include "protocol.h"
#include "radio_link.h"

// The ESP32-S3-DevKitC-1 has a WS2812 RGB LED on GPIO 38.
// (The PCB silkscreen labels this as "RGB@IO38".)
static const uint8_t LED_PIN = 38;
static const uint32_t BLINK_INTERVAL_MS = 500;

// Room for bursts from the PC. The Arduino USB CDC driver drops bytes
// when this buffer is full (default 256 bytes = only 4 packets).
static const size_t DAEMON_RX_BUFFER = 4096;

// CDC serial channel on the "USB" port (shows up as a COM port).
USBCDC DaemonSerial;

static uint32_t packetsFromHost = 0;
static uint32_t packetsForwarded = 0;
static uint32_t packetsDropped = 0;

// ── Heartbeat LED (non-blocking) ──────────────────────────
// Blinks every BLINK_INTERVAL_MS: blue while the radio link is up,
// green otherwise. Also reports what happened to packets from the host.
static void updateHeartbeat() {
    static uint32_t lastToggle = 0;
    static bool ledOn = false;
    static uint32_t packetsReported = 0;

    uint32_t now = millis();
    if (now - lastToggle < BLINK_INTERVAL_MS) return;
    lastToggle = now;

    ledOn = !ledOn;
    uint8_t level = ledOn ? 20 : 0;
    if (radio_link::isLinkUp()) {
        neopixelWrite(LED_PIN, 0, 0, level);
    } else {
        neopixelWrite(LED_PIN, 0, level, 0);
    }

    if (packetsFromHost != packetsReported) {
        Serial.printf("[host] packets so far: %u received, %u forwarded, %u dropped\n",
                      packetsFromHost, packetsForwarded, packetsDropped);
        packetsReported = packetsFromHost;
    }
}

// ── Input from the peer → this host ───────────────────────
// Called by radio_link for each input packet the peer sends us. By
// then radio_link has verified, decrypted, and replay-checked it.
static void injectInput(const uint8_t *raw) {
    proto::Header header;
    memcpy(&header, raw, sizeof(header));
    const uint8_t *payload = raw + proto::HEADER_SIZE;

    switch (header.msg_type) {
        case proto::MSG_KEY_DOWN: {
            proto::KeyEvent e;
            memcpy(&e, payload, sizeof(e));
            hid_output::keyDown(e.keycode, e.modifiers);
            break;
        }
        case proto::MSG_KEY_UP: {
            proto::KeyEvent e;
            memcpy(&e, payload, sizeof(e));
            hid_output::keyUp(e.keycode, e.modifiers);
            break;
        }
        case proto::MSG_MODIFIER_SYNC: {
            proto::ModifierSync m;
            memcpy(&m, payload, sizeof(m));
            hid_output::syncModifiers(m.modifiers);
            break;
        }
        case proto::MSG_MOUSE_MOVE: {
            proto::MouseMove m;
            memcpy(&m, payload, sizeof(m));
            hid_output::mouseMove(m.dx, m.dy, m.buttons);
            break;
        }
        case proto::MSG_MOUSE_SCROLL: {
            proto::MouseScroll s;
            memcpy(&s, payload, sizeof(s));
            hid_output::mouseScroll(s.vertical, s.horizontal);
            break;
        }
        default:
            break;
    }
}

// ── Input from this host → the peer ───────────────────────
static void forwardFromHost(const uint8_t *raw) {
    proto::Header header;
    memcpy(&header, raw, sizeof(header));

    if (header.version != proto::VERSION || !proto::isInputMessage(header.msg_type)) {
        Serial.printf("[host] ignored packet: version 0x%02X, msg_type 0x%02X\n",
                      header.version, header.msg_type);
        return;
    }
    if (radio_link::sendToPeer(raw)) {
        packetsForwarded++;
    } else {
        packetsDropped++;       // no radio link, or the send queue is full
    }
}

// Reads whatever bytes have arrived and assembles them into 64-byte
// packets. A USB serial link is a plain byte stream with no packet
// boundaries, so we wait for the magic byte to find where a packet
// starts, then collect PACKET_SIZE bytes.
static void pollDaemonLink() {
    static uint8_t buffer[proto::PACKET_SIZE];
    static size_t length = 0;

    while (DaemonSerial.available() > 0) {
        uint8_t b = DaemonSerial.read();
        if (length == 0 && b != proto::MAGIC) continue;   // not a packet start

        buffer[length++] = b;
        if (length == proto::PACKET_SIZE) {
            packetsFromHost++;
            forwardFromHost(buffer);
            length = 0;
        }
    }
}

// ── Setup: runs once at boot ──────────────────────────────
void setup() {
    // UART0 -> "UART" port (see platformio.ini build_flags)
    // With a TX buffer, Serial.print copies into it and returns; without
    // one it can wait until the bytes have left the 128-byte UART FIFO,
    // stalling the loop (and any input waiting in it) for ~10 ms per log
    // line. It made no measurable RTT difference in Phase 2, but logging
    // should never be able to delay input.
    Serial.setTxBufferSize(1024);
    Serial.begin(115200);

    // By default, a special DTR/RTS sequence on this port reboots the
    // chip into the bootloader. This port belongs to the daemon, so turn
    // that off; uploads go through the "UART" port.
    DaemonSerial.enableReboot(false);
    DaemonSerial.setRxBufferSize(DAEMON_RX_BUFFER);
    DaemonSerial.begin();

    // Register keyboard + mouse + CDC, then start USB. The names show
    // up in Device Manager / System Information on the host.
    hid_output::begin();
    USB.productName("Omni-KVM");
    USB.manufacturerName("Omni-KVM Project");
    USB.begin();

    radio_link::begin(injectInput);

    Serial.println("========================================");
    Serial.println("  Omni-KVM Firmware v0.2.0-dev (Phase 2)");
    Serial.println("  Board: ESP32-S3-DevKitC-1-N8R8");
    Serial.println("========================================");
    Serial.println();
    Serial.println("Forwarding host input to the peer over ESP-NOW.");
    Serial.println();
}

// ── Loop: runs repeatedly after setup ─────────────────────
// Nothing here waits: packets are read as they arrive, and keyboard
// reports leave at a paced rate from hid_output's queue.
void loop() {
    updateHeartbeat();
    pollDaemonLink();
    hid_output::update();
    radio_link::update();
}
