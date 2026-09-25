// ============================================================
// Omni-KVM Firmware — Phase 1: USB HID driven by protocol packets
// ============================================================
// Purpose: The "USB" port is a composite device: HID keyboard +
//          mouse, plus a CDC serial channel. 64-byte protocol
//          packets (shared/protocol.md) arriving on the CDC channel
//          are turned into keyboard/mouse input on the same host.
//
//          In the final design the CDC channel belongs to the host
//          daemon and input packets come from the peer board over
//          the radio. For now the PC sends input packets itself
//          (tools/hid_test.py) and the board handles them as if
//          they came from the radio.
//
// Cables:  "UART" port -> PC (upload + debug log)
//          "USB"  port -> the computer to control (can be the same PC)
// ============================================================

#include <Arduino.h>
#include "USB.h"
#include "hid_output.h"
#include "protocol.h"

// The ESP32-S3-DevKitC-1 has a WS2812 RGB LED on GPIO 38.
// (The PCB silkscreen labels this as "RGB@IO38".)
static const uint8_t LED_PIN = 38;
static const uint32_t BLINK_INTERVAL_MS = 500;

// Room for bursts from the PC. The Arduino USB CDC driver drops bytes
// when this buffer is full (default 256 bytes = only 4 packets).
static const size_t DAEMON_RX_BUFFER = 4096;

// CDC serial channel on the "USB" port (shows up as a COM port).
USBCDC DaemonSerial;

static uint32_t packetsReceived = 0;

// ── Heartbeat LED (non-blocking) ──────────────────────────
// Toggles the LED every BLINK_INTERVAL_MS and, while at it, reports
// how many packets arrived since the last report.
static void updateHeartbeat() {
    static uint32_t lastToggle = 0;
    static bool ledOn = false;
    static uint32_t packetsReported = 0;

    uint32_t now = millis();
    if (now - lastToggle < BLINK_INTERVAL_MS) return;
    lastToggle = now;

    ledOn = !ledOn;
    neopixelWrite(LED_PIN, 0, ledOn ? 20 : 0, 0);

    if (packetsReceived != packetsReported) {
        Serial.printf("[link] %u packets received so far\n", packetsReceived);
        packetsReported = packetsReceived;
    }
}

// ── Packet handling ───────────────────────────────────────
// Replay protection (sequence numbers) belongs to the radio path and
// arrives in Phase 2. The CDC channel is a cable to the trusted host.
static void handlePacket(const uint8_t *raw) {
    proto::Header header;
    memcpy(&header, raw, sizeof(header));
    const uint8_t *payload = raw + proto::HEADER_SIZE;

    if (header.version != proto::VERSION) {
        Serial.printf("[link] dropped packet: version 0x%02X\n", header.version);
        return;
    }
    if (header.flags & proto::FLAG_ENCRYPTED) {
        Serial.println("[link] dropped packet: encryption not supported yet");
        return;
    }

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
            Serial.printf("[link] ignored msg_type 0x%02X\n", header.msg_type);
            break;
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
            packetsReceived++;
            handlePacket(buffer);
            length = 0;
        }
    }
}

// ── Setup: runs once at boot ──────────────────────────────
void setup() {
    // UART0 -> "UART" port (see platformio.ini build_flags)
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

    Serial.println("========================================");
    Serial.println("  Omni-KVM Firmware v0.1.0 (Phase 1)");
    Serial.println("  Board: ESP32-S3-DevKitC-1-N8R8");
    Serial.println("========================================");
    Serial.println();
    Serial.println("Waiting for protocol packets on the USB port.");
    Serial.println();
}

// ── Loop: runs repeatedly after setup ─────────────────────
// Nothing here waits: packets are read as they arrive, and keyboard
// reports leave at a paced rate from hid_output's queue.
void loop() {
    updateHeartbeat();
    pollDaemonLink();
    hid_output::update();
}
