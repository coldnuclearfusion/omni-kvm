// ============================================================
// Radio link — see include/radio_link.h
// ============================================================

#include "radio_link.h"

#include <Arduino.h>
#include <WiFi.h>
#include <esp_now.h>
#include <esp_wifi.h>
#include "protocol.h"

namespace radio_link {

static const uint8_t RADIO_CHANNEL = 1;             // both boards must match
static const uint32_t HEARTBEAT_INTERVAL_MS = 100;
static const uint32_t LINK_TIMEOUT_MS = 3000;       // 30 missed heartbeats
static const uint32_t STATS_INTERVAL_MS = 2000;
static const uint8_t BROADCAST_MAC[6] = {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF};

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
static bool wasLinkUp = false;

// Counters since the last stats line
struct Stats {
    uint32_t heartbeatsSent = 0;
    uint32_t heartbeatsReceived = 0;
    uint32_t rttCount = 0;
    uint32_t rttSumUs = 0;
    uint32_t rttMinUs = UINT32_MAX;
    uint32_t rttMaxUs = 0;
};
static Stats stats;

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
    xQueueSend(rxQueue, &r, 0);     // if the queue is full, drop it
}

static void addPeer(const uint8_t *mac) {
    esp_now_peer_info_t peer = {};
    memcpy(peer.peer_addr, mac, sizeof(peer.peer_addr));
    peer.channel = 0;               // 0 = the channel we are already on
    peer.ifidx = WIFI_IF_STA;
    peer.encrypt = false;           // encryption comes in a later step
    esp_now_add_peer(&peer);
}

static void send(const uint8_t *mac, uint8_t msgType, const void *payload, size_t len) {
    uint8_t packet[proto::PACKET_SIZE] = {};
    proto::Header header = {proto::MAGIC, proto::VERSION, msgType, 0, ++txSeq};
    memcpy(packet, &header, sizeof(header));
    memcpy(packet + proto::HEADER_SIZE, payload, len);
    esp_now_send(mac, packet, sizeof(packet));
}

static void handle(const Received &r) {
    proto::Header header;
    memcpy(&header, r.data, sizeof(header));
    if (header.version != proto::VERSION) return;

    if (!havePeer) {
        if (header.msg_type != proto::MSG_HEARTBEAT) return;
        memcpy(peerMac, r.mac, sizeof(peerMac));
        havePeer = true;
        addPeer(peerMac);
        Serial.printf("[radio] peer found: %s\n", macToString(peerMac).c_str());
    }
    if (memcmp(r.mac, peerMac, sizeof(peerMac)) != 0) return;   // not our peer
    lastHeardMs = millis();

    proto::Heartbeat hb;
    memcpy(&hb, r.data + proto::HEADER_SIZE, sizeof(hb));
    switch (header.msg_type) {
        case proto::MSG_HEARTBEAT:
            stats.heartbeatsReceived++;
            send(peerMac, proto::MSG_HEARTBEAT_ACK, &hb, sizeof(hb));   // echo it back
            break;
        case proto::MSG_HEARTBEAT_ACK: {
            uint32_t rtt = micros() - hb.timestamp;
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
        Serial.printf("[radio] no peer yet | heartbeats sent %u\n", stats.heartbeatsSent);
    } else if (stats.rttCount == 0) {
        Serial.printf("[radio] peer %s | sent %u, received %u | no RTT samples\n",
                      macToString(peerMac).c_str(), stats.heartbeatsSent, stats.heartbeatsReceived);
    } else {
        Serial.printf("[radio] peer %s | sent %u, received %u | RTT us min %u avg %u max %u\n",
                      macToString(peerMac).c_str(), stats.heartbeatsSent, stats.heartbeatsReceived,
                      stats.rttMinUs, stats.rttSumUs / stats.rttCount, stats.rttMaxUs);
    }
    stats = Stats();
}

void begin() {
    rxQueue = xQueueCreate(16, sizeof(Received));

    // ESP-NOW needs the Wi-Fi radio running in station mode, but we never
    // join a network: no WiFi.begin(), no auto-reconnect (which would call
    // WiFi.begin() on a disconnect event), and any network saved in flash
    // by earlier firmware is erased. See docs/threat-model.md.
    WiFi.setAutoReconnect(false);
    WiFi.mode(WIFI_STA);
    WiFi.disconnect(false, true);
    esp_wifi_set_channel(RADIO_CHANNEL, WIFI_SECOND_CHAN_NONE);

    if (esp_now_init() != ESP_OK) {
        Serial.println("[radio] ESP-NOW init failed");
        return;
    }
    esp_now_register_recv_cb(onReceive);
    addPeer(BROADCAST_MAC);   // heartbeats go to everyone until we know our peer

    Serial.printf("[radio] ready on channel %u, my MAC %s\n",
                  RADIO_CHANNEL, WiFi.macAddress().c_str());
}

void update() {
    Received r;
    while (xQueueReceive(rxQueue, &r, 0) == pdTRUE) {
        handle(r);
    }

    uint32_t now = millis();

    static uint32_t lastHeartbeatMs = 0;
    if (now - lastHeartbeatMs >= HEARTBEAT_INTERVAL_MS) {
        lastHeartbeatMs = now;
        proto::Heartbeat hb = {micros(), 0};    // link quality not measured yet
        send(havePeer ? peerMac : BROADCAST_MAC, proto::MSG_HEARTBEAT, &hb, sizeof(hb));
        stats.heartbeatsSent++;
    }

    bool linkUp = isLinkUp();
    if (linkUp != wasLinkUp) {
        Serial.println(linkUp ? "[radio] link UP" : "[radio] link DOWN");
        wasLinkUp = linkUp;
    }

    static uint32_t lastStatsMs = 0;
    if (now - lastStatsMs >= STATS_INTERVAL_MS) {
        lastStatsMs = now;
        printStats();
    }
}

bool isLinkUp() {
    return havePeer && millis() - lastHeardMs < LINK_TIMEOUT_MS;
}

}  // namespace radio_link
