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
// the peer over the radio; input packets from the peer are typed into
// this board's host as keyboard/mouse reports while the input gate is
// open. Daemon-to-daemon messages (views) are passed along to the peer's
// daemon unread. The USB guard (usb_guard.h) works around defects in the
// USB driver underneath.
//
// LED: blinks every 500 ms, blue while the radio link is up, green
// otherwise.
//
// Cables:  "USB"  port -> this board's own computer (its daemon, and the
//                         keyboard/mouse the other computer types with)
//          "UART" port -> a computer, for uploads, the log and development
//                         commands; not needed in use
// ============================================================

#include <Arduino.h>
#include "USB.h"
#include "hid_output.h"
#include "protocol.h"
#include "radio_link.h"
#include "usb_guard.h"

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

// A daemon has the serial port open. The Arduino driver's flag alone is
// not enough: it stays set when the host resets the device (after a USB
// reconnection, for one), until a daemon opens and closes the port again.
static bool daemonConnected() {
    return DaemonSerial && usb_guard::serialOpen();
}

// ── Packets to this host's daemon ─────────────────────────
// The CDC transmit buffer holds only 64 bytes, and DaemonSerial.write()
// waits, with no time limit, until the host has taken all it was given.
// So packets for the host wait in toHost and go out as the host reads
// them, a few bytes at a time if need be, without the loop ever waiting.
// If the host stops reading, new packets are dropped whole.
//
// The buffer is also flushed regularly: a byte left in it without a
// flush otherwise stays there until the next write.
//
// Writes also wait for the USB guard (usb_guard.h), which fixes the USB
// driver's FIFO layout after each configuration: with the driver's own,
// data going to and from the host overwrite each other under load, and
// the board eventually never sends to its host again.
static const size_t TO_HOST_LEN = 32 * proto::PACKET_SIZE;
static const uint32_t FLUSH_INTERVAL_MS = 5;
static uint8_t toHost[TO_HOST_LEN];
static size_t toHostHead = 0;
static size_t toHostCount = 0;
static uint32_t hostBytes = 0;          // taken by the USB driver since boot
static uint32_t hostProgressMs = 0;     // when it last took some

static bool queueForHost(const uint8_t *packet) {
    if (!daemonConnected() || TO_HOST_LEN - toHostCount < proto::PACKET_SIZE) return false;
    for (size_t i = 0; i < proto::PACKET_SIZE; i++) {
        toHost[(toHostHead + toHostCount + i) % TO_HOST_LEN] = packet[i];
    }
    toHostCount += proto::PACKET_SIZE;
    return true;
}

static void sendToHost() {
    static uint32_t lastFlushMs = 0;
    if (!daemonConnected()) {
        toHostCount = 0;    // nobody listening: a new connection starts clean
        return;
    }
    while (toHostCount > 0 && usb_guard::hostTxReady()) {
        int space = DaemonSerial.availableForWrite();
        if (space <= 0) break;
        size_t chunk = min<size_t>(min<size_t>(space, toHostCount), TO_HOST_LEN - toHostHead);
        size_t sent = DaemonSerial.write(toHost + toHostHead, chunk);   // fits: returns at once
        if (sent == 0) break;
        toHostHead = (toHostHead + sent) % TO_HOST_LEN;
        toHostCount -= sent;
        hostBytes += sent;
        hostProgressMs = millis();
    }
    uint32_t now = millis();
    if (now - lastFlushMs >= FLUSH_INTERVAL_MS && usb_guard::hostTxReady()) {
        lastFlushMs = now;
        DaemonSerial.flush();
    }
}

// A short MSG_DAEMON_STATUS (status id and one data byte). False if there
// is no room for it (or no daemon); the callers try again later.
static bool sendStatus(uint8_t statusId, uint8_t value) {
    uint8_t packet[proto::PACKET_SIZE] = {};
    proto::Header header = {proto::MAGIC, proto::VERSION, proto::MSG_DAEMON_STATUS, 0, 0};
    memcpy(packet, &header, sizeof(header));
    packet[proto::HEADER_SIZE] = statusId;
    packet[proto::HEADER_SIZE + 1] = value;
    return queueForHost(packet);
}

// ── Input gate ────────────────────────────────────────────
// Input from the peer is typed into this host only while the gate is
// open (shared/protocol.md, "Input gate"). The daemon closes it before it
// starts forwarding its own input and opens it after it stops, so input
// never goes round in a circle. With no daemon connected it stays open:
// the board is then a plain keyboard and mouse for the other computer.
static bool gateOpen = true;

// A close is confirmed (status 0x06) once the releases it caused have
// reached the host: the daemon's settle time starts at the confirmation,
// so it then only has to cover the host's own handling of them. If they
// have not all reached it GATE_CONFIRM_LIMIT_MS after the close, it is
// confirmed anyway, so the switch still goes ahead.
static const uint32_t GATE_CONFIRM_LIMIT_MS = 50;
static bool confirmPending = false;
static uint8_t confirmNumber = 0;
static uint32_t closedAtMs = 0;

static void closeGate(uint8_t request) {
    gateOpen = false;
    hid_output::dropPending();
    hid_output::releaseAll();
    confirmPending = true;
    confirmNumber = request;
    closedAtMs = millis();
}

static void openGate() {
    gateOpen = true;
    confirmPending = false;     // a confirmation now would be out of date
}

static void confirmGateClosed() {
    if (!confirmPending) return;
    if (!hid_output::allSent() && millis() - closedAtMs < GATE_CONFIRM_LIMIT_MS) return;
    if (sendStatus(proto::STATUS_GATE_CLOSED, confirmNumber)) confirmPending = false;
}

// ── Input from the peer → this host ───────────────────────
// Called by radio_link for each input packet the peer sends us. By
// then radio_link has verified, decrypted, and replay-checked it.
static void injectInput(const uint8_t *raw) {
    if (!gateOpen) return;      // still acknowledged: handled, by dropping it

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
        case proto::MSG_KEY_STATE: {
            proto::KeyState k;
            memcpy(&k, payload, sizeof(k));
            hid_output::keepOnly(k.modifiers, k.buttons, k.keys);
            break;
        }
        default:
            break;
    }
}

// Development counters (UART command "host report").
static uint32_t hostGonesSent = 0;
static uint32_t hostGonesReceived = 0;
static uint32_t hostGoneReleases = 0;

// The peer has no daemon (MSG_HOST_GONE, repeated every second): the keys
// and buttons its daemon pressed here can no longer be released by it.
static void onPeerHostGone(const uint8_t *) {
    hostGonesReceived++;
    if (!hid_output::holding()) return;
    hostGoneReleases++;
    hid_output::releaseAll();
    Serial.println("[radio] the other computer's daemon is gone: let go of its keys and buttons");
}

// ── Messages from the peer's daemon → this host's daemon ──
// The board only passes them along.
static void relayToHost(const uint8_t *raw) {
    if (!queueForHost(raw)) {
        Serial.printf("[host] no daemon listening (or not reading): dropped message 0x%02X from the peer\n",
                      raw[offsetof(proto::Header, msg_type)]);
    }
}

// ── Radio link changes ────────────────────────────────────
// If the link drops, or the peer restarts, while it holds a key or button
// down here, its "key up" can never arrive: let go of everything.
//
// The daemon is told of every change (status 0x05). A new session with no
// gap in between (the peer's board restarted within T_link) is told as
// the link going down and up again: the peer lost everything, as in a
// link loss. A report that finds no room goes out on a later pass.
static bool daemonKnowsUp = false;      // what the daemon was last told
static bool lossToReport = false;       // a loss the daemon has not heard of
static bool daemonToldNothing = false;  // just connected: tell it either way

static void watchLink() {
    static bool wasUp = false;
    static uint32_t lastSession = 0;
    bool up = radio_link::isLinkUp();
    uint32_t session = radio_link::totals().session;
    if ((wasUp && !up) || (up && session != lastSession)) {
        hid_output::releaseAll();
        if (daemonKnowsUp) lossToReport = true;
    }
    wasUp = up;
    lastSession = session;

    if (!daemonConnected()) return;
    if (lossToReport) {
        if (!sendStatus(proto::STATUS_LINK_CHANGED, 0)) return;
        lossToReport = false;
        daemonKnowsUp = false;
    }
    if ((up != daemonKnowsUp || daemonToldNothing) && sendStatus(proto::STATUS_LINK_CHANGED, up ? 1 : 0)) {
        daemonKnowsUp = up;
        daemonToldNothing = false;
    }
}

// ── This host's daemon coming and going ───────────────────
// The daemon holds the USB serial port open while it runs; the operating
// system closes it when the daemon quits or crashes.
static uint8_t fromHost[proto::PACKET_SIZE];    // packet being assembled
static size_t fromHostLength = 0;
static bool hostGoneToSend = false;

static void watchDaemon() {
    static bool wasConnected = false;
    bool connected = daemonConnected();
    if (connected == wasConnected) return;
    wasConnected = connected;

    // A new connection starts on a packet boundary: forget any packet the
    // previous one left half sent.
    fromHostLength = 0;
    daemonKnowsUp = false;
    lossToReport = false;
    if (connected) {
        daemonToldNothing = true;
        hostGoneToSend = false;
        Serial.println("[host] daemon connected");
    } else {
        // Nobody to close the gate any more (B8), and nobody to release
        // what the daemon forwarded to the other computer (F8).
        openGate();
        hostGoneToSend = true;
        Serial.println("[host] daemon disconnected: input gate open");
    }
}

// MSG_HOST_GONE goes out behind whatever the daemon queued for the radio
// before it left, and again every HOST_GONE_INTERVAL_MS while no daemon is
// connected and the link is up. It is state ("no daemon here"), like the
// views and the key state: a copy lost on the radio even after every retry
// is repaired a second later (docs/focus-design.md, found by the automated
// check, 4). With the link down it is not needed: the peer lets go of
// everything on link loss anyway.
static const uint32_t HOST_GONE_INTERVAL_MS = 1000;

static void sendHostGone() {
    static uint32_t lastSentMs = 0;
    if (daemonConnected() || !radio_link::isLinkUp()) {
        hostGoneToSend = false;
        return;
    }
    if (!hostGoneToSend && millis() - lastSentMs < HOST_GONE_INTERVAL_MS) return;
    uint8_t packet[proto::PACKET_SIZE] = {};
    proto::Header header = {proto::MAGIC, proto::VERSION, proto::MSG_HOST_GONE, 0, 0};
    memcpy(packet, &header, sizeof(header));
    if (radio_link::sendToPeer(packet)) {
        hostGoneToSend = false;
        lastSentMs = millis();
        hostGonesSent++;
    }
}

// ── Development: diagnostic reports ──────────────────────
// While switched on (DAEMON_CMD 0x0A), the board sends the other board a
// report on its USB serial link every BOARD_REPORT_INTERVAL_MS, which
// that board passes to its host: it gets through even when this board
// can no longer send to its own host.
static const uint32_t BOARD_REPORT_INTERVAL_MS = 500;
static bool boardReports = false;

// Development: while on (DAEMON_CMD 0x0A setting 7), a filler status
// packet (0x7F) for the host every FILLER_INTERVAL_MS, standing in for
// messages relayed from the other board in USB experiments with one board.
static const uint32_t FILLER_INTERVAL_MS = 2;
static bool hostFiller = false;

static void sendHostFiller() {
    static uint32_t lastMs = 0;
    uint32_t now = millis();
    if (!hostFiller || now - lastMs < FILLER_INTERVAL_MS) return;
    lastMs = now;
    sendStatus(0x7F, 0);
}

// The same, as a line in this board's own log (UART).
static void logBoardReport(const uint8_t *d, uint16_t queued, uint16_t waitingMs, uint16_t fixes) {
    uint32_t ctl, size;
    uint16_t mask, ints;
    memcpy(&mask, d + 2, 2);
    memcpy(&ctl, d + 4, 4);
    memcpy(&size, d + 8, 4);
    memcpy(&ints, d + 12, 2);
    Serial.printf("[usb] ep4 EPENA=%u XFERSIZE=%u PKTCNT=%u mask=%u busy=%u fifo-free=%uw diepint=0x%02X "
                  "diepctl=0x%08X | ready=%u susp=%u dtr=%u prevent=%u layout-ok=%u | daemon %u, radio link %u | "
                  "queued %u, waiting %u ms, sent %u, layout fixes %u, reconnects %u, forgotten transfers stopped %u\n",
                  (unsigned)(ctl >> 31), (unsigned)(size & 0x7FFFF), (unsigned)((size >> 19) & 0x3FF),
                  (unsigned)((mask >> 4) & 1), (d[1] >> 6) & 1, d[14], ints, (unsigned)ctl,
                  (d[1] >> 2) & 1, (d[1] >> 1) & 1, (d[1] >> 3) & 1, (d[1] >> 4) & 1, (d[1] >> 5) & 1,
                  d[15] & 1, (d[15] >> 1) & 1, queued, waitingMs, (unsigned)hostBytes, fixes, (unsigned)usb_guard::reconnects(),
                  (unsigned)usb_guard::forgottenTransfersStopped());
}

static bool reportOnce = false;     // development: log one report (UART command)

static void sendBoardReport() {
    static uint32_t lastMs = 0;
    uint32_t now = millis();
    bool periodic = boardReports && now - lastMs >= BOARD_REPORT_INTERVAL_MS;
    if (!periodic && !reportOnce) return;
    reportOnce = false;
    if (periodic) lastMs = now;
    uint8_t packet[proto::PACKET_SIZE] = {};
    proto::Header header = {proto::MAGIC, proto::VERSION, proto::MSG_BOARD_REPORT, 0, 0};
    memcpy(packet, &header, sizeof(header));
    uint8_t *d = packet + proto::HEADER_SIZE;
    d[0] = 1;   // report format
    usb_guard::snapshot(d + 1);
    d[15] = (daemonConnected() ? 1 : 0) | (radio_link::isLinkUp() ? 2 : 0);
    uint16_t queued = toHostCount;
    uint16_t waitingMs = min<uint32_t>(now - hostProgressMs, UINT16_MAX);
    uint16_t fixes = min<uint32_t>(usb_guard::fixes(), UINT16_MAX);
    memcpy(d + 16, &queued, 2);
    memcpy(d + 18, &hostBytes, 4);
    memcpy(d + 22, &waitingMs, 2);
    memcpy(d + 24, &fixes, 2);
    uint16_t reconnects = min<uint32_t>(usb_guard::reconnects(), UINT16_MAX);
    memcpy(d + 26, &reconnects, 2);
    // That fills the 28 data bytes a sealed packet carries: the count of
    // forgotten transfers stopped is only in the log line.
    logBoardReport(d, queued, waitingMs, fixes);
    if (periodic && radio_link::isLinkUp()) radio_link::sendToPeer(packet);
}

static void setUsbGuard(uint8_t setting, bool on) {
    switch (setting) {
        case 1: usb_guard::settings().prevent = on; break;
        case 3: boardReports = on; break;
        case 4: usb_guard::logFifoLayout(); return;
        case 7: hostFiller = on; break;
        case 8: usb_guard::requestReconnect(); return;
        case 9: usb_guard::requestDriverResetImitation(); return;
        default: return;
    }
    Serial.printf("[dev] setting %u %s\n", setting, on ? "on" : "off");
}

// ── Development: commands on the UART port ────────────────
// The daemon holds the USB serial port while it runs, so commands for
// tests with the daemons running come in on the "UART" port, one text
// line each:
//   usb reconnect   disconnect from USB and connect again (setting 8)
//   usb k6          imitate the USB driver's own reset (setting 9)
//   usb layout      print the USB FIFO layout (setting 4)
//   usb report      print one board report line
//   host report     print the daemon connection and MSG_HOST_GONE counts
static void runUartCommand(const char *command) {
    if (strcmp(command, "usb reconnect") == 0) {
        usb_guard::requestReconnect();
    } else if (strcmp(command, "usb k6") == 0) {
        usb_guard::requestDriverResetImitation();
    } else if (strcmp(command, "usb layout") == 0) {
        usb_guard::logFifoLayout();
    } else if (strcmp(command, "usb report") == 0) {
        reportOnce = true;
    } else if (strcmp(command, "host report") == 0) {
        Serial.printf("[host] daemon connected %u | MSG_HOST_GONE sent %u, received %u, let go of something %u\n",
                      (unsigned)daemonConnected(), (unsigned)hostGonesSent, (unsigned)hostGonesReceived,
                      (unsigned)hostGoneReleases);
    } else {
        Serial.printf("[uart] unknown command: %s\n", command);
        return;
    }
    Serial.printf("[uart] %s\n", command);
}

static void pollUartCommands() {
    static char line[32];
    static size_t length = 0;
    static bool overlong = false;
    while (Serial.available() > 0) {
        char c = Serial.read();
        if (c == '\r') continue;
        if (c != '\n') {
            if (length < sizeof(line) - 1) {
                line[length++] = c;
            } else {
                overlong = true;
            }
            continue;
        }
        line[length] = 0;
        if (!overlong && length > 0) runUartCommand(line);
        length = 0;
        overlong = false;
    }
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
    queueForHost(packet);
}

static void handleDaemonCommand(const uint8_t *raw) {
    uint8_t command = raw[proto::HEADER_SIZE];
    uint8_t data = raw[proto::HEADER_SIZE + 1];
    if (command == proto::CMD_GATE_CLOSE) {
        closeGate(data);
    } else if (command == proto::CMD_GATE_OPEN) {
        openGate();
    } else if (command == proto::CMD_REQUEST_LINK_STATS) {
        replyLinkStats();
    } else if (command == proto::CMD_SET_PHY_RATE) {
        radio_link::setPhyRate(data);
    } else if (command == proto::CMD_USB_GUARD) {
        setUsbGuard(data, raw[proto::HEADER_SIZE + 2] != 0);
    } else {
        Serial.printf("[host] unknown daemon command 0x%02X\n", command);
    }
}

// ── Packets from this host ────────────────────────────────
// Input and daemon-to-daemon messages go to the peer; MSG_DAEMON_CMD is
// for this board. Returns false if the bytes cannot be a packet from the
// daemon (wrong version, unknown type, or data past what the radio
// carries): the byte stream has lost its place.
static bool handleHostPacket(const uint8_t *raw) {
    proto::Header header;
    memcpy(&header, raw, sizeof(header));
    if (header.version != proto::VERSION || !proto::fitsSealedData(raw)) return false;

    if (header.msg_type == proto::MSG_DAEMON_CMD) {
        handleDaemonCommand(raw);
        return true;
    }
    if (!proto::isInputMessage(header.msg_type) && !proto::isRelayMessage(header.msg_type)) {
        return false;
    }
    packetsFromHost++;
    if (radio_link::sendToPeer(raw)) {
        packetsForwarded++;
    } else {
        packetsDropped++;       // no radio link, or the send queue is full
    }
    return true;
}

// Reads whatever bytes have arrived and assembles them into 64-byte
// packets. A USB serial link is a plain byte stream with no packet
// boundaries, so we wait for the magic byte to find where a packet
// starts, then collect PACKET_SIZE bytes.
//
// While the radio send queue is full we stop reading, so a burst waits
// in the USB receive buffer. If that buffer overflows too, the USB
// driver drops bytes and packets run into each other; a packet that
// makes no sense is then discarded (and counted as dropped), and reading
// starts over at the next magic byte inside it.
static void pollDaemonLink() {
    while (DaemonSerial.available() > 0) {
        if (fromHostLength == 0 && radio_link::sendQueueSpace() == 0) break;

        uint8_t b = DaemonSerial.read();
        if (fromHostLength == 0 && b != proto::MAGIC) continue;   // not a packet start

        fromHost[fromHostLength++] = b;
        if (fromHostLength < proto::PACKET_SIZE) continue;
        if (handleHostPacket(fromHost)) {
            fromHostLength = 0;
            continue;
        }
        packetsDropped++;
        Serial.println("[host] garbled packet discarded; resynchronizing");
        const uint8_t *next = (const uint8_t *)memchr(fromHost + 1, proto::MAGIC, proto::PACKET_SIZE - 1);
        fromHostLength = next ? proto::PACKET_SIZE - (next - fromHost) : 0;
        if (fromHostLength) memmove(fromHost, next, fromHostLength);
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

    radio_link::begin(injectInput, relayToHost, onPeerHostGone);

    Serial.println("========================================");
    Serial.println("  Omni-KVM Firmware v0.5.4-dev (Phase 4)");
    Serial.println("  Board: ESP32-S3-DevKitC-1-N8R8");
    Serial.println("========================================");
    Serial.println();
    Serial.println("Forwarding this computer's input to the peer, typing the peer's in while the gate is open.");
    Serial.println();
}

// ── Loop: runs repeatedly after setup ─────────────────────
// Nothing here waits, except that a keyboard or mouse report can take up
// to 100 ms to leave (docs/platform.md, K5): packets are read as they
// arrive, and keyboard reports leave at a paced rate from hid_output's
// queue.
void loop() {
    usb_guard::update();    // first: no USB transfer may start with a bad FIFO layout
    if (usb_guard::watch(daemonConnected() && toHostCount > 0, hostBytes)) {
        hid_output::hostLostState();
    }
    updateHeartbeat();
    watchDaemon();
    pollDaemonLink();
    hid_output::update();
    radio_link::update();
    watchLink();
    confirmGateClosed();
    sendHostGone();
    pollUartCommands();
    sendBoardReport();
    sendHostFiller();
    sendToHost();
}
