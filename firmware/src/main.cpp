// ============================================================
// Omni-KVM Firmware
// ============================================================
// The "USB" port is a composite device: HID keyboard + mouse, plus a
// CDC serial channel for the host daemon. Both boards run this same
// firmware:
//
//   host ──CDC──▶ this board ══ESP-NOW══▶ peer board ──HID──▶ peer's host
//                                                    └──CDC──▶ peer's daemon
//
// Input packets (shared/protocol.md) from the host are forwarded to
// the peer over the radio; input packets from the peer are injected
// into this board's host as keyboard/mouse reports. Daemon-to-daemon
// messages (handoff) are passed along to the peer's daemon unread.
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

// If the link drops, or the peer restarts, while it holds a key or
// button down here, its "key up" can never arrive: let go of everything.
static void releaseInputOnLinkLoss() {
    static bool wasUp = false;
    static uint32_t lastSession = 0;
    bool up = radio_link::isLinkUp();
    uint32_t session = radio_link::totals().session;
    if ((wasUp && !up) || (up && session != lastSession)) hid_output::releaseAll();
    wasUp = up;
    lastSession = session;
}

// ── Messages from the peer's daemon → this host's daemon ──
// The board only passes them along. With no daemon listening, or one
// that has stopped reading, the message is dropped rather than letting
// the write stall the loop (and input) for the CDC timeout.
static void relayToHost(const uint8_t *raw) {
    if (DaemonSerial.availableForWrite() < (int)proto::PACKET_SIZE) {
        Serial.printf("[host] no daemon listening: dropped message 0x%02X from the peer\n",
                      raw[offsetof(proto::Header, msg_type)]);
        return;
    }
    DaemonSerial.write(raw, proto::PACKET_SIZE);
}

// ── Requests from this host to the board itself ──────────
static void replyLinkStats() {
    radio_link::Totals t = radio_link::totals();
    proto::LinkStats s = {};
    s.status_id = proto::STATUS_LINK_STATS;
    s.layout = proto::LINK_STATS_LAYOUT;
    s.link_up = radio_link::isLinkUp();
    s.phy_rate = t.phyRate;
    s.session = t.session;
    s.host_received = packetsFromHost;
    s.host_dropped = packetsDropped;
    s.radio_sent = t.inputSent;
    s.radio_retransmits = t.inputRetransmits;
    s.radio_gave_up = t.inputGaveUp;
    s.radio_received = t.inputReceived;
    s.radio_rx_overflow = t.rxOverflow;
    s.auth_failures = t.authFailures;
    s.replays_dropped = t.replaysDropped;
    s.frames_acked = t.framesAcked;
    s.frames_failed = t.framesFailed;
    hid_output::Stats h = hid_output::stats();
    s.hid_stalls = min<uint32_t>(h.keyboardStalls, UINT16_MAX);
    s.hid_mouse_dropped = min<uint32_t>(h.mouseDropped, UINT16_MAX);

    uint8_t packet[proto::PACKET_SIZE] = {};
    proto::Header header = {proto::MAGIC, proto::VERSION, proto::MSG_DAEMON_STATUS, 0, 0};
    memcpy(packet, &header, sizeof(header));
    memcpy(packet + proto::HEADER_SIZE, &s, sizeof(s));
    DaemonSerial.write(packet, sizeof(packet));
}

static void handleDaemonCommand(const uint8_t *raw) {
    uint8_t command = raw[proto::HEADER_SIZE];
    if (command == proto::CMD_REQUEST_LINK_STATS) {
        replyLinkStats();
    } else if (command == proto::CMD_SET_PHY_RATE) {
        radio_link::setPhyRate(raw[proto::HEADER_SIZE + 1]);
    } else {
        Serial.printf("[host] unknown daemon command 0x%02X\n", command);
    }
}

// ── Packets from this host ────────────────────────────────
// Input and daemon-to-daemon messages go to the peer; MSG_DAEMON_CMD is
// for this board.
static void handleHostPacket(const uint8_t *raw) {
    proto::Header header;
    memcpy(&header, raw, sizeof(header));

    if (header.version == proto::VERSION && header.msg_type == proto::MSG_DAEMON_CMD) {
        handleDaemonCommand(raw);
        return;
    }
    bool forPeer = proto::isInputMessage(header.msg_type) || proto::isRelayMessage(header.msg_type);
    if (header.version != proto::VERSION || !forPeer) {
        Serial.printf("[host] ignored packet: version 0x%02X, msg_type 0x%02X\n",
                      header.version, header.msg_type);
        return;
    }
    packetsFromHost++;
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
//
// While the radio send queue is full we stop reading, so a burst waits
// in the USB receive buffer instead of being dropped.
static void pollDaemonLink() {
    static uint8_t buffer[proto::PACKET_SIZE];
    static size_t length = 0;

    while (DaemonSerial.available() > 0) {
        if (length == 0 && radio_link::sendQueueSpace() == 0) break;

        uint8_t b = DaemonSerial.read();
        if (length == 0 && b != proto::MAGIC) continue;   // not a packet start

        buffer[length++] = b;
        if (length == proto::PACKET_SIZE) {
            handleHostPacket(buffer);
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

    radio_link::begin(injectInput, relayToHost);

    Serial.println("========================================");
    Serial.println("  Omni-KVM Firmware v0.4.0-dev (Phase 4)");
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
    releaseInputOnLinkLoss();
}
