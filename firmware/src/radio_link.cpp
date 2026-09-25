// ============================================================
// Radio link — see include/radio_link.h
// ============================================================

#include "radio_link.h"

#include <atomic>
#include <Arduino.h>
#include <WiFi.h>
#include <esp_now.h>
#include <esp_wifi.h>
#include "protocol.h"
#include "secure_packet.h"
#include "session.h"

#if __has_include("secrets/dev_key.h")
#include "secrets/dev_key.h"
#else
#error "No radio key. Run: python tools/make_dev_key.py (then flash both boards)"
#endif
static_assert(sizeof(DEV_RADIO_KEY) == secure_packet::KEY_SIZE,
              "Old 128-bit key. Run: python tools/make_dev_key.py --force (then flash both boards)");

// Radio settings, overridable with build flags for experiments. Chosen
// from the Phase 2/3 experiments (shared/protocol.md, "Radio settings"):
// 24 Mbps gave the lowest latency on an open desk, but next to a laptop
// it lost 12-99% of frames. At the worst spot, 18 Mbps and below lost
// none; 12 Mbps keeps more signal margin than 18 for ~0.3 ms per packet.
#ifndef OMNI_RADIO_CHANNEL
#define OMNI_RADIO_CHANNEL 6
#endif
#ifndef OMNI_RADIO_PHY_RATE
#define OMNI_RADIO_PHY_RATE WIFI_PHY_RATE_12M
#endif
// OMNI_LOG_RTT_SAMPLES: 1 prints every RTT sample ("rtt <us>").

namespace radio_link {

static const uint8_t RADIO_CHANNEL = OMNI_RADIO_CHANNEL;   // both boards must match
static const uint32_t HEARTBEAT_INTERVAL_MS = 100;   // also the HELLO interval
static const uint32_t LINK_TIMEOUT_MS = 3000;        // 30 missed heartbeats
static const uint32_t STATS_INTERVAL_MS = 2000;
static const size_t RX_QUEUE_LEN = 32;
static const size_t TX_QUEUE_LEN = 128;
static const uint8_t BROADCAST_MAC[6] = {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF};

static const uint8_t MAX_INPUT_ATTEMPTS = 6;         // first try + 5 retransmissions
static const uint32_t INPUT_ACK_TIMEOUT_MS = 25;     // a round trip normally takes ~2 ms

static PacketHandler inputHandler = nullptr;
static PacketHandler relayHandler = nullptr;

// Input packets waiting to go out, in order.
struct Outgoing {
    uint8_t packet[proto::PACKET_SIZE];
    bool needsAck;          // key event or mouse button change: must arrive
};
static Outgoing txQueue[TX_QUEUE_LEN];
static size_t txHead = 0;
static size_t txCount = 0;

// The input packet waiting for the peer's MSG_INPUT_ACK. Only one such
// packet is outstanding at a time, and nothing queued after it is sent
// until it is acknowledged, so the peer never sees "key up" before a
// late "key down". Every attempt goes out with a new sequence number;
// an acknowledgement of any of them counts.
struct AwaitingAck {
    bool active = false;
    uint8_t packet[proto::PACKET_SIZE];     // plaintext, for retransmission
    uint8_t attempts = 0;
    uint32_t seqs[MAX_INPUT_ATTEMPTS];
    uint32_t sentMs = 0;
};
static AwaitingAck awaiting;
static uint8_t lastQueuedButtons = 0;       // mouse buttons in the last queued MOUSE_MOVE

static uint8_t phyRate = OMNI_RADIO_PHY_RATE;

// ESP-NOW calls onReceive() from the Wi-Fi task, which runs at the same
// time as loop(). onReceive() only copies the packet into this queue;
// update() does the actual work, so the two never share other state.
struct Received {
    uint8_t mac[6];
    uint8_t data[proto::PACKET_SIZE];
};
static QueueHandle_t rxQueue = nullptr;

static uint32_t txSeq = 0;
static bool havePeer = false;
static uint8_t peerMac[6];
static uint32_t lastHeardMs = 0;
static uint32_t lastPeerSeq = 0;    // highest seq accepted from the peer in this session
static uint32_t sessionGeneration = 0;
static uint32_t linkDownSinceMs = 0;
static bool wasLinkUp = false;

// Counters since the last stats line
struct Stats {
    uint32_t heartbeatsSent = 0;
    uint32_t heartbeatsReceived = 0;
    uint32_t hellosSent = 0;
    uint32_t hellosReceived = 0;
    uint32_t inputSent = 0;
    uint32_t inputReceived = 0;
    uint32_t rttCount = 0;
    uint32_t rttSumUs = 0;
    uint32_t rttMinUs = UINT32_MAX;
    uint32_t rttMaxUs = 0;
    uint32_t authFailures = 0;      // packets whose tag didn't verify
    uint32_t replaysDropped = 0;    // authentic packets with an old seq
    uint32_t sealCount = 0;
    uint32_t sealSumUs = 0;
    uint32_t openCount = 0;
    uint32_t openSumUs = 0;
};
static Stats stats;

// Counters since boot, reported to the host (totals()). The atomics are
// shared with the Wi-Fi task: each is written by only one side.
static Totals total;
static std::atomic<uint32_t> rxOverflow{0};     // written in onReceive (Wi-Fi task)
static std::atomic<uint32_t> framesAcked{0};    // written in onSent (Wi-Fi task)
static std::atomic<uint32_t> framesFailed{0};   // written in onSent (Wi-Fi task)

// ESP-NOW reports each frame as delivered (acknowledged by the peer's
// radio) or failed after its own retries. That only means the peer's
// radio received it, not that the peer processed it, so input delivery
// relies on MSG_INPUT_ACK instead; these counts are diagnostics.
static void onSent(const uint8_t *mac, esp_now_send_status_t status) {
    if (status == ESP_NOW_SEND_SUCCESS) {
        framesAcked++;
    } else {
        framesFailed++;
    }
}

static String macToString(const uint8_t *mac) {
    char s[18];
    snprintf(s, sizeof(s), "%02X:%02X:%02X:%02X:%02X:%02X",
             mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    return String(s);
}

static void onReceive(const uint8_t *mac, const uint8_t *data, int len) {
    if (len != (int)proto::PACKET_SIZE || data[0] != proto::MAGIC) return;
    Received r;
    memcpy(r.mac, mac, sizeof(r.mac));
    memcpy(r.data, data, sizeof(r.data));
    if (xQueueSend(rxQueue, &r, 0) != pdTRUE) {
        rxOverflow++;               // queue full: the packet is lost
    }
}

static void addPeer(const uint8_t *mac, bool isBroadcast = false) {
    esp_now_peer_info_t peer = {};
    memcpy(peer.peer_addr, mac, sizeof(peer.peer_addr));
    peer.channel = 0;               // 0 = the channel we are already on
    peer.ifidx = WIFI_IF_STA;
#if OMNI_RADIO_CRYPTO == OMNI_CRYPTO_ESPNOW
    // Benchmark mode: let ESP-NOW encrypt unicast frames itself (CCMP).
    // ESP-NOW cannot encrypt broadcast frames.
    peer.encrypt = !isBroadcast;
    memcpy(peer.lmk, DEV_RADIO_KEY, sizeof(peer.lmk));   // first 16 bytes
#else
    peer.encrypt = false;           // we encrypt ourselves (secure_packet), not ESP-NOW
#endif
    esp_now_add_peer(&peer);
}

// HELLOs are sealed with the long-term key (they create sessions);
// everything else with the current session key.
static const uint8_t *keyFor(uint8_t msgType) {
    return msgType == proto::MSG_SESSION_HELLO ? session::longTermKey() : session::sessionKey();
}

// Stamps our next sequence number into a copy of the packet, seals it,
// and hands it to ESP-NOW. The caller's packet stays plaintext so it can
// be sent again; every send gets a fresh sequence number and nonce.
// On success, *usedSeq (if given) receives the sequence number used.
static esp_err_t transmit(const uint8_t *mac, const uint8_t *packet, uint32_t *usedSeq = nullptr) {
    uint8_t sealed[proto::PACKET_SIZE];
    memcpy(sealed, packet, sizeof(sealed));
    uint32_t seq = txSeq + 1;
    memcpy(sealed + offsetof(proto::Header, seq), &seq, sizeof(seq));

    uint32_t start = micros();
    if (!secure_packet::seal(sealed, keyFor(sealed[offsetof(proto::Header, msg_type)]))) {
        return ESP_ERR_INVALID_ARG;
    }
    stats.sealSumUs += micros() - start;
    stats.sealCount++;

    esp_err_t result = esp_now_send(mac, sealed, sizeof(sealed));
    if (result == ESP_OK) {
        txSeq = seq;
        if (usedSeq) *usedSeq = seq;
    }
    return result;
}

static void send(const uint8_t *mac, uint8_t msgType, const void *payload, size_t len) {
    uint8_t packet[proto::PACKET_SIZE] = {};
    proto::Header header = {proto::MAGIC, proto::VERSION, msgType, 0, 0};
    memcpy(packet, &header, sizeof(header));
    memcpy(packet + proto::HEADER_SIZE, payload, len);
    transmit(mac, packet);
}

// The peer processed one of our input packets.
static void onInputAck(uint32_t seq) {
    if (!awaiting.active) return;
    for (uint8_t i = 0; i < awaiting.attempts; i++) {
        if (awaiting.seqs[i] == seq) {
            awaiting.active = false;
            return;
        }
    }
}

// Resends the packet awaiting acknowledgement if its time is up, or gives
// up after MAX_INPUT_ATTEMPTS. Returns true while it is still outstanding.
static bool serviceAwaitingAck() {
    if (!awaiting.active) return false;
    if (millis() - awaiting.sentMs < INPUT_ACK_TIMEOUT_MS) return true;
    if (awaiting.attempts >= MAX_INPUT_ATTEMPTS || !session::isUp()) {
        awaiting.active = false;
        total.inputGaveUp++;
        return false;
    }
    uint32_t seq;
    esp_err_t result = transmit(peerMac, awaiting.packet, &seq);
    if (result == ESP_OK) {
        awaiting.seqs[awaiting.attempts++] = seq;
        awaiting.sentMs = millis();
        stats.inputSent++;
        total.inputSent++;
        total.inputRetransmits++;
    }
    return true;    // on ESP_ERR_ESPNOW_NO_MEM, try again next pass
}

// Sends the next queued input packet, unless an earlier one is still
// waiting to be acknowledged.
static void flushTxQueue() {
    if (serviceAwaitingAck()) return;
    if (txCount == 0 || !session::isUp()) return;

    Outgoing &next = txQueue[txHead];
    if (next.needsAck) next.packet[offsetof(proto::Header, flags)] |= proto::FLAG_ACK_REQUESTED;
    uint32_t seq;
    esp_err_t result = transmit(peerMac, next.packet, &seq);
    if (result == ESP_ERR_ESPNOW_NO_MEM) return;    // ESP-NOW is busy; try again next pass
    if (result == ESP_OK) {
        stats.inputSent++;
        total.inputSent++;
        if (next.needsAck) {
            awaiting.active = true;
            memcpy(awaiting.packet, next.packet, sizeof(awaiting.packet));
            awaiting.attempts = 1;
            awaiting.seqs[0] = seq;
            awaiting.sentMs = millis();
        }
    }
    txHead = (txHead + 1) % TX_QUEUE_LEN;
    txCount--;
}

static void handleHello(const Received &r) {
    stats.hellosReceived++;

    // Only a board holding the long-term key can become our peer.
    if (!havePeer) {
        memcpy(peerMac, r.mac, sizeof(peerMac));
        havePeer = true;
        addPeer(peerMac);
        Serial.printf("[radio] peer found: %s\n", macToString(peerMac).c_str());
    }

    proto::SessionHello hello;
    proto::SessionHello reply;
    memcpy(&hello, r.data + proto::HEADER_SIZE, sizeof(hello));
    if (session::onHello(peerMac, hello, &reply)) {
        send(peerMac, proto::MSG_SESSION_HELLO, &reply, sizeof(reply));
        stats.hellosSent++;
    }

    if (session::generation() != sessionGeneration) {
        // A new session: the peer's sequence numbers start over, safely,
        // because packets from any earlier session no longer authenticate.
        sessionGeneration = session::generation();
        lastPeerSeq = 0;
        if (wasLinkUp) {
            Serial.printf("[radio] session %u established, replacing a running one (peer restarted)\n",
                          sessionGeneration);
        } else {
            Serial.printf("[radio] session %u established, %u ms after link loss or boot\n",
                          sessionGeneration, millis() - linkDownSinceMs);
        }
        lastHeardMs = millis();
    }
}

static void handle(Received &r) {
    // Once we have a peer, ignore everyone else before spending time on crypto.
    if (havePeer && memcmp(r.mac, peerMac, sizeof(peerMac)) != 0) return;

    // The header is plaintext, so we can see which key the packet needs.
    uint8_t msgType = r.data[offsetof(proto::Header, msg_type)];
    if (msgType != proto::MSG_SESSION_HELLO && !session::isUp()) return;   // no key to open it with

    uint32_t start = micros();
    if (!secure_packet::open(r.data, keyFor(msgType))) {
        stats.authFailures++;       // altered, corrupted, another key, or an old session
        total.authFailures++;
        return;
    }
    stats.openSumUs += micros() - start;
    stats.openCount++;

    proto::Header header;
    memcpy(&header, r.data, sizeof(header));
    if (header.version != proto::VERSION) return;

    if (msgType == proto::MSG_SESSION_HELLO) {
        handleHello(r);
        return;
    }

    // Replay protection: within a session the peer's seq only goes up, so
    // an authentic packet with an old seq is a recording being played back.
    if (header.seq <= lastPeerSeq) {
        stats.replaysDropped++;
        total.replaysDropped++;
        return;
    }
    lastPeerSeq = header.seq;
    lastHeardMs = millis();

    bool isInput = proto::isInputMessage(header.msg_type);
    if (isInput || proto::isRelayMessage(header.msg_type)) {
        stats.inputReceived++;
        total.inputReceived++;
        PacketHandler handler = isInput ? inputHandler : relayHandler;
        if (handler) handler(r.data);
        // Acknowledge after processing, so "acknowledged" means "applied"
        // (for a relayed message: handed to the host). A retransmitted
        // copy is processed again, which is harmless: input messages
        // carry state ("key A is down"), not toggles, and daemons must
        // accept a relayed message twice.
        if (header.flags & proto::FLAG_ACK_REQUESTED) {
            proto::InputAck ack = {header.seq};
            send(peerMac, proto::MSG_INPUT_ACK, &ack, sizeof(ack));
        }
        return;
    }
    if (header.msg_type == proto::MSG_INPUT_ACK) {
        proto::InputAck ack;
        memcpy(&ack, r.data + proto::HEADER_SIZE, sizeof(ack));
        onInputAck(ack.seq);
        return;
    }

    proto::Heartbeat hb;
    memcpy(&hb, r.data + proto::HEADER_SIZE, sizeof(hb));
    switch (header.msg_type) {
        case proto::MSG_HEARTBEAT:
            stats.heartbeatsReceived++;
            send(peerMac, proto::MSG_HEARTBEAT_ACK, &hb, sizeof(hb));   // echo it back
            break;
        case proto::MSG_HEARTBEAT_ACK: {
            uint32_t rtt = micros() - hb.timestamp;
#if OMNI_LOG_RTT_SAMPLES
            Serial.printf("rtt %u\n", rtt);
#endif
            stats.rttCount++;
            stats.rttSumUs += rtt;
            stats.rttMinUs = min(stats.rttMinUs, rtt);
            stats.rttMaxUs = max(stats.rttMaxUs, rtt);
            break;
        }
        default:
            break;
    }
}

static void printStats() {
    if (!havePeer) {
        Serial.printf("[radio] no peer yet | HELLOs sent %u\n", stats.hellosSent);
    } else if (!session::isUp()) {
        Serial.printf("[radio] peer %s | handshaking: HELLOs sent %u, received %u\n",
                      macToString(peerMac).c_str(), stats.hellosSent, stats.hellosReceived);
    } else if (stats.rttCount == 0) {
        Serial.printf("[radio] peer %s | session %u | sent %u, received %u | no RTT samples\n",
                      macToString(peerMac).c_str(), sessionGeneration,
                      stats.heartbeatsSent, stats.heartbeatsReceived);
    } else {
        Serial.printf("[radio] peer %s | session %u | sent %u, received %u | RTT us min %u avg %u max %u\n",
                      macToString(peerMac).c_str(), sessionGeneration,
                      stats.heartbeatsSent, stats.heartbeatsReceived,
                      stats.rttMinUs, stats.rttSumUs / stats.rttCount, stats.rttMaxUs);
    }
    if (stats.inputSent || stats.inputReceived) {
        Serial.printf("[radio] input sent %u, received %u | since boot: retransmitted %u, gave up %u\n",
                      stats.inputSent, stats.inputReceived, total.inputRetransmits, total.inputGaveUp);
    }
    Serial.printf("[radio] crypto %s: seal avg %u us, open avg %u us | auth failures %u, replays dropped %u\n",
                  secure_packet::modeName(),
                  stats.sealCount ? stats.sealSumUs / stats.sealCount : 0,
                  stats.openCount ? stats.openSumUs / stats.openCount : 0,
                  stats.authFailures, stats.replaysDropped);
    stats = Stats();
}

void begin(PacketHandler onInput, PacketHandler onRelay) {
    inputHandler = onInput;
    relayHandler = onRelay;
    rxQueue = xQueueCreate(RX_QUEUE_LEN, sizeof(Received));

    // ESP-NOW needs the Wi-Fi radio running in station mode, but we never
    // join a network: no WiFi.begin(), no auto-reconnect (which would call
    // WiFi.begin() on a disconnect event), and any network saved in flash
    // by earlier firmware is erased. See docs/threat-model.md.
    WiFi.setAutoReconnect(false);
    WiFi.mode(WIFI_STA);
    WiFi.disconnect(false, true);
    esp_wifi_set_channel(RADIO_CHANNEL, WIFI_SECOND_CHAN_NONE);
    esp_wifi_config_espnow_rate(WIFI_IF_STA, OMNI_RADIO_PHY_RATE);   // must follow esp_wifi_start()

    if (esp_now_init() != ESP_OK) {
        Serial.println("[radio] ESP-NOW init failed");
        return;
    }

    uint8_t selfMac[6];
    esp_wifi_get_mac(WIFI_IF_STA, selfMac);
    session::begin(DEV_RADIO_KEY, selfMac);   // Wi-Fi is on, so the RNG is truly random
#if OMNI_RADIO_CRYPTO == OMNI_CRYPTO_ESPNOW
    esp_now_set_pmk(DEV_RADIO_KEY);           // benchmark: ESP-NOW's own key hierarchy (first 16 bytes)
#endif
    esp_now_register_recv_cb(onReceive);
    esp_now_register_send_cb(onSent);
    addPeer(BROADCAST_MAC, true);   // HELLOs go to everyone until we know our peer

    Serial.printf("[radio] ready on channel %u, my MAC %s\n",
                  RADIO_CHANNEL, macToString(selfMac).c_str());
}

#if OMNI_TEST_REPLAY
// ── Replay attack test (build flag OMNI_TEST_REPLAY=1, test only) ──
// This board plays the attacker: it records one of its own sealed
// heartbeats and later re-sends the recording raw. The peer's log shows
// what happened ("replays dropped" or "auth failures").
//   First boot:  record at 5 s, replay in the same session at 8 s, then
//                restart itself at 10 s.
//   Second boot: replay the first boot's recording into the new session
//                at 2 s, then record/replay once more (no restart).
// The recording lives in RTC memory, which survives esp_restart() but
// not a power cycle or an EN-pin reset.
static RTC_NOINIT_ATTR uint8_t recording[proto::PACKET_SIZE];
static RTC_NOINIT_ATTR uint32_t recordingMagic;
static const uint32_t RECORDING_VALID = 0x52504C59;   // "RPLY"

static void testReplay() {
    static bool started = false;
    static bool havePrevious = false;
    static uint8_t previous[proto::PACKET_SIZE];
    static uint32_t sessionStartMs = 0;
    static bool replayedPrevious = false, recorded = false, replayedCurrent = false;

    if (!started) {                 // first call after boot
        started = true;
        havePrevious = recordingMagic == RECORDING_VALID;
        if (havePrevious) memcpy(previous, recording, sizeof(previous));
    }
    if (!isLinkUp()) {
        sessionStartMs = 0;
        return;
    }
    if (sessionStartMs == 0) sessionStartMs = millis();
    uint32_t t = millis() - sessionStartMs;

    if (havePrevious && !replayedPrevious && t > 2000) {
        replayedPrevious = true;
        esp_now_send(peerMac, previous, sizeof(previous));
        Serial.println("[test] replayed a packet recorded in the PREVIOUS session");
    }
    if (!recorded && t > 5000) {
        // Seal a heartbeat the same way transmit() does, send it, keep the bytes.
        uint8_t packet[proto::PACKET_SIZE] = {};
        proto::Heartbeat hb = {micros(), 0};
        proto::Header header = {proto::MAGIC, proto::VERSION, proto::MSG_HEARTBEAT, 0, ++txSeq};
        memcpy(packet, &header, sizeof(header));
        memcpy(packet + proto::HEADER_SIZE, &hb, sizeof(hb));
        secure_packet::seal(packet, session::sessionKey());
        esp_now_send(peerMac, packet, sizeof(packet));
        memcpy(recording, packet, sizeof(recording));
        recordingMagic = RECORDING_VALID;
        recorded = true;
        Serial.println("[test] recorded a sealed heartbeat");
    }
    if (recorded && !replayedCurrent && t > 8000) {
        replayedCurrent = true;
        esp_now_send(peerMac, recording, sizeof(recording));
        Serial.println("[test] replayed a packet recorded in THIS session");
    }
    if (!havePrevious && replayedCurrent && t > 10000) {
        Serial.println("[test] restarting to replay the recording into a new session");
        Serial.flush();
        esp_restart();
    }
}
#endif

void update() {
    Received r;
    while (xQueueReceive(rxQueue, &r, 0) == pdTRUE) {
        handle(r);
    }

#if OMNI_TEST_REPLAY
    testReplay();
#endif

    uint32_t now = millis();

    if (session::isUp() && now - lastHeardMs >= LINK_TIMEOUT_MS) {
        session::drop();    // the next session needs a fresh handshake
    }

    static uint32_t lastBeatMs = 0;
    if (now - lastBeatMs >= HEARTBEAT_INTERVAL_MS) {
        lastBeatMs = now;
        if (session::isUp()) {
            proto::Heartbeat hb = {micros(), 0};    // link quality not measured yet
            send(peerMac, proto::MSG_HEARTBEAT, &hb, sizeof(hb));
            stats.heartbeatsSent++;
        } else {
            proto::SessionHello hello;
            session::makeHello(&hello);
            send(havePeer ? peerMac : BROADCAST_MAC, proto::MSG_SESSION_HELLO, &hello, sizeof(hello));
            stats.hellosSent++;
        }
    }

    flushTxQueue();

    bool linkUp = isLinkUp();
    if (linkUp != wasLinkUp) {
        Serial.println(linkUp ? "[radio] link UP" : "[radio] link DOWN");
        if (!linkUp) linkDownSinceMs = now;
        wasLinkUp = linkUp;
    }

    static uint32_t lastStatsMs = 0;
    if (now - lastStatsMs >= STATS_INTERVAL_MS) {
        lastStatsMs = now;
        printStats();
    }
}

bool isLinkUp() {
    return session::isUp() && millis() - lastHeardMs < LINK_TIMEOUT_MS;
}

// Must this input packet be acknowledged (and resent until it is)? Key
// events and modifier syncs are: a lost one means a missing or stuck
// key. Mouse movement is not: a late delta makes the pointer jump, and
// the next one moves it anyway. But a movement that changes the buttons
// is, or a click or drag could get stuck. Repeats are harmless, because
// the peer applies them as state ("key A is down"), not as toggles.
// Relayed daemon messages (a handoff) are rare and must arrive too.
static bool needsAck(const uint8_t *packet) {
    uint8_t msgType = packet[offsetof(proto::Header, msg_type)];
    if (proto::isRelayMessage(msgType)) return true;
    switch (msgType) {
        case proto::MSG_KEY_DOWN:
        case proto::MSG_KEY_UP:
        case proto::MSG_MODIFIER_SYNC:
            return true;
        case proto::MSG_MOUSE_MOVE: {
            uint8_t buttons = packet[proto::HEADER_SIZE + offsetof(proto::MouseMove, buttons)];
            bool changed = buttons != lastQueuedButtons;
            lastQueuedButtons = buttons;
            return changed;
        }
        default:
            return false;
    }
}

bool sendToPeer(const uint8_t *packet) {
    if (!isLinkUp() || txCount == TX_QUEUE_LEN) return false;
    Outgoing &slot = txQueue[(txHead + txCount) % TX_QUEUE_LEN];
    memcpy(slot.packet, packet, proto::PACKET_SIZE);
    slot.needsAck = needsAck(packet);
    txCount++;
    return true;
}

size_t sendQueueSpace() {
    return TX_QUEUE_LEN - txCount;
}

Totals totals() {
    Totals t = total;
    t.session = sessionGeneration;
    t.rxOverflow = rxOverflow;
    t.framesAcked = framesAcked;
    t.framesFailed = framesFailed;
    t.phyRate = phyRate;
    return t;
}

bool setPhyRate(uint8_t rate) {
    static const uint8_t allowed[] = {
        WIFI_PHY_RATE_1M_L, WIFI_PHY_RATE_2M_L, WIFI_PHY_RATE_5M_L, WIFI_PHY_RATE_11M_L,
        WIFI_PHY_RATE_6M, WIFI_PHY_RATE_9M, WIFI_PHY_RATE_12M, WIFI_PHY_RATE_18M,
        WIFI_PHY_RATE_24M, WIFI_PHY_RATE_36M, WIFI_PHY_RATE_48M, WIFI_PHY_RATE_54M,
    };
    bool ok = false;
    for (uint8_t a : allowed) ok = ok || a == rate;
    if (!ok || esp_wifi_config_espnow_rate(WIFI_IF_STA, (wifi_phy_rate_t)rate) != ESP_OK) {
        Serial.printf("[radio] PHY rate 0x%02X rejected\n", rate);
        return false;
    }
    phyRate = rate;
    Serial.printf("[radio] PHY rate set to 0x%02X\n", rate);
    return true;
}

}  // namespace radio_link
