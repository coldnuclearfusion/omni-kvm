// ============================================================
// USB guard — see include/usb_guard.h
// ============================================================

#include "usb_guard.h"

#include <Arduino.h>
#include <string.h>
#include "tusb.h"
#include "device/usbd_pvt.h"
#include "soc/usb_reg.h"
#include "soc/usb_struct.h"
#include "soc/usb_wrap_reg.h"

namespace usb_guard {

// The Arduino core 2.0.x gives its serial port's data IN endpoint this
// fixed address (cores/esp32/USBCDC.cpp, load_cdc_descriptor), and the
// keyboard/mouse IN endpoint 0x83 (register dump, docs/platform.md).
static const uint8_t CDC_DATA_IN = 0x84;
static const uint8_t CDC_DATA_IN_NUM = CDC_DATA_IN & 0x0F;
static const uint8_t HID_IN = 0x83;

// Transmit FIFOs 1–4 get this many words each (two 64-byte packets),
// right after the receive FIFO and EP0's, which the driver lays out on
// every bus reset: 68 + 4 × 32 = 196 of the 200 words.
static const uint32_t FIFO_WORDS = 32;
static const int TX_FIFOS = 4;
// Waiting longer than this for the FIFOs to be free, the board carries on
// with the driver's layout rather than stay silent.
static const uint32_t GIVE_UP_MS = 1000;
// After a transfer completes, the USB task finishes with it (and may send
// a zero-length packet) within microseconds (see Settings::prevent).
static const uint32_t IDLE_BEFORE_WRITE_US = 1000;
// The controller confirms a flush, a NAK or an endpoint disable within
// microseconds; waiting for it gives up after this (interrupts are off).
static const uint32_t REGISTER_WAIT_US = 1000;
// A forgotten transfer (see forgotten()) is stopped once seen this long:
// long enough to rule out a transfer finishing just as it is looked at.
static const uint32_t FORGOTTEN_MS = 2;

// Watchdog. A transfer takes a few milliseconds (the host polls every
// millisecond; a keyboard or mouse report is waited for up to 100 ms),
// and the layout fix holds transfers back for at most GIVE_UP_MS:
// STUCK_MS is well past all of them.
static const uint32_t STUCK_MS = 1500;
// A hub notices a disconnect within microseconds (USB 2.0, 7.1.7.3); the
// rest is room for the computer to let go of the device.
static const uint32_t RECONNECT_OFF_MS = 500;
// The computer resets and configures the device again well within this;
// after it the watchdog carries on anyway, and logs it.
static const uint32_t WAIT_FOR_HOST_MS = 5000;
static const uint32_t RECONNECT_MIN_INTERVAL_MS = 10000;

// DIEPCTLn fields (DWC2 USB OTG core).
static const uint32_t EP_ENABLE = 1u << 31;
static uint32_t txFifoOf(uint32_t diepctl) {
    return (diepctl >> 22) & 0xF;
}
// DIEPINTn flags cleared by writing 1: all but TXFEMP, which only shows
// the FIFO's level.
static const uint32_t IN_EP_FLAGS = USB_D_XFERCOMPL0_M | USB_D_EPDISBLD0_M | USB_D_AHBERR0_M |
                                    USB_D_TIMEOUT0_M | USB_D_INTKNTXFEMP0_M | USB_D_INTKNEPMIS0_M |
                                    USB_D_INEPNAKEFF0_M | USB_D_TXFIFOUNDRN0_M | USB_D_BNAINTR0_M |
                                    USB_D_PKTDRPSTS0_M | USB_D_BBLEERR0_M | USB_D_NAKINTRPT0_M |
                                    USB_D_NYETINTRPT0_M;

// How long a condition has held, in milliseconds (0 while it does not).
struct Held {
    bool holding = false;
    uint32_t sinceMs = 0;

    uint32_t check(bool holds, uint32_t now) {
        if (!holds) {
            holding = false;
            return 0;
        }
        if (!holding) {
            holding = true;
            sinceMs = now;
        }
        return now - sinceMs;
    }
};

// Watchdog phases (see usb_guard.h).
enum class Phase { Watching, Disconnected, WaitingForHost };

static Settings current;
static uint32_t fixCount = 0;
static uint32_t forgottenStops = 0;
static Held layoutWrong;            // configured with the wrong layout
static Held forgottenFor[USB_IN_EP_NUM];
static bool gaveUpLogged = false;
static Phase phase = Phase::Watching;

Settings &settings() {
    return current;
}

// Where FIFOs 1–4 start: after EP0's transmit FIFO.
static uint32_t fifoBase() {
    uint32_t np = USB0.gnptxfsiz;
    return (np & 0xFFFF) + (np >> 16);
}

static uint32_t wanted(int f, uint32_t base) {
    return (FIFO_WORDS << 16) | (base + FIFO_WORDS * f);
}

static bool layoutRight() {
    uint32_t base = fifoBase();
    for (int f = 0; f < TX_FIFOS; f++) {
        if (USB0.dieptxf[f] != wanted(f, base)) return false;
    }
    return true;
}

// No transfer uses FIFOs 1–4: every IN endpoint mapped to one is idle.
static bool fifoUsersIdle() {
    for (int n = 1; n < USB_IN_EP_NUM; n++) {
        uint32_t ctl = USB0.in_ep_reg[n].diepctl;
        uint32_t f = txFifoOf(ctl);
        if (f < 1 || f > TX_FIFOS) continue;
        if ((ctl & EP_ENABLE) || usbd_edpt_busy(0, 0x80 | n)) return false;
    }
    return true;
}

// Waits until a flag is set (or clear), but not for ever. False if it
// gave up.
static bool waitFor(volatile uint32_t &reg, uint32_t flag, bool set) {
    uint32_t start = micros();
    while (((reg & flag) != 0) != set) {
        if (micros() - start > REGISTER_WAIT_US) return false;
    }
    return true;
}

// Empties transmit FIFO f (0x10: every one), and so resets its pointers.
static void flushTxFifo(uint32_t f) {
    waitFor(USB0.grstctl, USB_AHBIDLE_M, true);
    USB0.grstctl = (f << USB_TXFNUM_S) | USB_TXFFLSH_M;
    waitFor(USB0.grstctl, USB_TXFFLSH_M, false);
}

static void fixLayout() {
    uint32_t base = fifoBase();
    // With this core's interrupts off, the USB interrupt (which runs on
    // this core) cannot start filling a FIFO halfway through.
    portDISABLE_INTERRUPTS();
    for (int f = 0; f < TX_FIFOS; f++) USB0.dieptxf[f] = wanted(f, base);
    for (int f = 1; f <= TX_FIFOS; f++) flushTxFifo(f);    // to its new place
    portENABLE_INTERRUPTS();
}

// Stops IN endpoint n's transfer the way the controller requires (NAK it
// and wait until that takes effect, then disable it and wait until it
// is), clears its flags and transfer size, and turns its FIFO-empty
// interrupt off: the driver's interrupt fills a FIFO from the transfer
// size of any endpoint whose FIFO-empty interrupt is on. Interrupts must
// be off. False if the controller did not confirm in time.
static bool stopInEndpoint(int n) {
    volatile usb_in_endpoint_t &ep = USB0.in_ep_reg[n];
    bool confirmed = true;
    if (ep.diepctl & USB_D_EPENA1_M) {
        ep.diepctl |= USB_DI_SNAK1_M;
        confirmed = waitFor(ep.diepint, USB_D_INEPNAKEFF0_M, true);
        ep.diepctl |= USB_DI_SNAK1_M | USB_D_EPDIS1_M;
        confirmed = waitFor(ep.diepint, USB_D_EPDISBLD0_M, true) && confirmed;
    }
    ep.dieptsiz = 0;
    ep.diepint = IN_EP_FLAGS;
    USB0.dtknqr4_fifoemptymsk &= ~(1u << n);
    return confirmed;
}

// An IN transfer TinyUSB has forgotten: one left over from before a reset
// or a new configuration, which the driver leaves enabled (docs/platform.md,
// K6, K7). TinyUSB marks a transfer busy before the driver enables it, and
// clears the mark only after the controller has finished it, so an
// enabled endpoint that TinyUSB counts as idle holds a forgotten one.
static bool forgotten(int n) {
    return (USB0.in_ep_reg[n].diepctl & EP_ENABLE) && !usbd_edpt_busy(0, 0x80 | n);
}

// Log line: the IN endpoints' state, one bit per endpoint.
static void logInEndpoints(const char *when) {
    uint32_t enabled = 0, busy = 0;
    for (int n = 0; n < USB_IN_EP_NUM; n++) {
        if (USB0.in_ep_reg[n].diepctl & EP_ENABLE) enabled |= 1u << n;
        if (usbd_edpt_busy(0, 0x80 | n)) busy |= 1u << n;
    }
    Serial.printf("[usb] %s: IN endpoints enabled 0x%02X, busy for TinyUSB 0x%02X, FIFO-empty interrupts 0x%02X; "
                  "address %u, configured %u\n",
                  when, (unsigned)enabled, (unsigned)busy, (unsigned)(USB0.dtknqr4_fifoemptymsk & 0xFFFF),
                  (unsigned)((USB0.dcfg & USB_DEVADDR_M) >> USB_DEVADDR_S), (unsigned)tud_mounted());
}

// After a new configuration, stops forgotten transfers on endpoints that
// use FIFOs 1–4. Left alone, one either reaches the computer as old data
// when it next polls the endpoint, or has a new transfer programmed over
// it and blocks the endpoint for good (K7).
static void stopForgottenTransfers(uint32_t now) {
    for (int n = 1; n < USB_IN_EP_NUM; n++) {
        uint32_t f = txFifoOf(USB0.in_ep_reg[n].diepctl);
        bool candidate = f >= 1 && f <= TX_FIFOS && forgotten(n);
        if (forgottenFor[n].check(candidate, now) < FORGOTTEN_MS || !candidate) continue;
        uint32_t size = USB0.in_ep_reg[n].dieptsiz;
        bool stopped = false, confirmed = true;
        portDISABLE_INTERRUPTS();
        if (forgotten(n)) {     // still: the USB task may have moved on meanwhile
            confirmed = stopInEndpoint(n);
            flushTxFifo(f);
            stopped = true;
        }
        portENABLE_INTERRUPTS();
        forgottenFor[n].check(false, now);
        if (!stopped) continue;
        forgottenStops++;
        Serial.printf("[usb] stopped a transfer left over from before the reset on EP%d IN (%u bytes, %u packets "
                      "left)%s\n", n, (unsigned)(size & 0x7FFFF), (unsigned)((size >> 19) & 0x3FF),
                      confirmed ? "" : "; the controller did not confirm in time");
    }
}

// ── Watchdog ──────────────────────────────────────────────
static uint32_t reconnectCount = 0;
static uint32_t lastReconnectMs = 0;
static uint32_t phaseSinceMs = 0;
static uint32_t fixesAtReconnect = 0;
static bool asked = false;
static bool imitate = false;        // development: imitate the driver's own reset (K6)
static Held serialStuck, hidStuck, dataStuck;
static uint32_t lastBytesTaken = 0;

// Stops every IN endpoint (stopInEndpoint()) and empties every transmit
// FIFO. The driver's bus reset does neither: without this, an old
// transfer would outlive the reset (docs/platform.md, K7). Interrupts
// must be off, and the device disconnected (the host not polling).
// Returns the endpoints that did not confirm in time, one bit each.
static uint32_t stopInEndpoints() {
    uint32_t late = 0;
    for (int n = 0; n < USB_IN_EP_NUM; n++) {
        if (!stopInEndpoint(n)) late |= 1u << n;
    }
    flushTxFifo(0x10);
    return late;
}

static void logLate(uint32_t late) {
    if (late) Serial.printf("[usb] IN endpoints 0x%02X did not confirm stopping in time\n", (unsigned)late);
}

static void disconnect(uint32_t now) {
    // With this core's interrupts off, the USB interrupt (which runs on
    // this core) cannot fill a FIFO while the endpoints are stopped.
    logInEndpoints("disconnecting");
    portDISABLE_INTERRUPTS();
    tud_disconnect();
    uint32_t late = stopInEndpoints();
    portENABLE_INTERRUPTS();
    logLate(late);
    reconnectCount++;
    lastReconnectMs = now;
    phase = Phase::Disconnected;
    phaseSinceMs = now;
}

static void connectAgain(uint32_t now) {
    logInEndpoints("connecting again");     // as the disconnected time left them
    portDISABLE_INTERRUPTS();
    // Again, in case the USB task started a transfer meanwhile (from a
    // completion it was still handling when the board disconnected).
    uint32_t late = stopInEndpoints();
    tud_connect();
    portENABLE_INTERRUPTS();
    logLate(late);
    fixesAtReconnect = fixCount;
    phase = Phase::WaitingForHost;
    phaseSinceMs = now;
    Serial.println("[usb] connected again: waiting for the computer to configure the device");
}

// Development: does what the driver's bus-reset routine does when its
// interrupt meets an "Unknown Condition" (K6), with a serial transfer
// under way: the device forgets its address, so the computer, getting no
// answers, resets it. Copied from bus_reset() in Espressif's
// dcd_esp32sx.c (docs/platform.md, section 3).
static void imitateDriverReset() {
    logInEndpoints("imitating the driver's own reset");
    portDISABLE_INTERRUPTS();
    for (int n = 0; n < USB_OUT_EP_NUM; n++) USB0.out_ep_reg[n].doepctl |= USB_DO_SNAK0_M;
    USB0.dcfg &= ~USB_DEVADDR_M;
    USB0.daintmsk = USB_OUTEPMSK0_M | USB_INEPMSK0_M;
    USB0.doepmsk = USB_SETUPMSK_M | USB_XFERCOMPLMSK;
    USB0.diepmsk = USB_TIMEOUTMSK_M | USB_DI_XFERCOMPLMSK_M;
    USB0.grstctl |= 0x10 << USB_TXFNUM_S;
    USB0.grstctl |= USB_TXFFLSH_M;
    USB0.grxfsiz = 52;
    USB0.gnptxfsiz = (16 << USB_NPTXFDEP_S) | (USB0.grxfsiz & 0x0000ffffUL);
    USB0.out_ep_reg[0].doeptsiz |= USB_SUPCNT0_M;
    USB0.gintmsk |= USB_IEPINTMSK_M | USB_OEPINTMSK_M;
    portENABLE_INTERRUPTS();
}

bool watch(bool hostDataWaiting, uint32_t hostBytesTaken) {
    uint32_t now = millis();
    bool moved = hostBytesTaken != lastBytesTaken;
    lastBytesTaken = hostBytesTaken;

    if (phase == Phase::Disconnected) {
        if (now - phaseSinceMs >= RECONNECT_OFF_MS) connectAgain(now);
        return false;
    }
    if (phase == Phase::WaitingForHost) {
        // Every configuration gets its FIFO layout fixed (update()), so a
        // new fix means the computer has configured the device again.
        if (fixCount != fixesAtReconnect) {
            phase = Phase::Watching;
            Serial.println("[usb] the computer has configured the device again");
        } else if (now - phaseSinceMs >= WAIT_FOR_HOST_MS) {
            phase = Phase::Watching;
            Serial.println("[usb] the computer has not configured the device again yet: watching anyway");
        }
        return false;
    }

    bool awake = tud_mounted() && !tud_suspended();
    bool portOpen = tud_cdc_n_get_line_state(0) & 1;
    uint32_t serial = serialStuck.check(awake && portOpen && usbd_edpt_busy(0, CDC_DATA_IN), now);
    uint32_t hid = hidStuck.check(awake && usbd_edpt_busy(0, HID_IN), now);
    uint32_t data = dataStuck.check(awake && hostDataWaiting && !moved, now);
    if (imitate && (USB0.in_ep_reg[CDC_DATA_IN_NUM].diepctl & EP_ENABLE)) {
        imitate = false;
        imitateDriverReset();
        return false;
    }
    if (asked) {
        Serial.println("[usb] reconnecting USB, as asked");
    } else if (serial < STUCK_MS && hid < STUCK_MS && data < STUCK_MS) {
        return false;
    } else if (reconnectCount > 0 && now - lastReconnectMs < RECONNECT_MIN_INTERVAL_MS) {
        return false;
    } else {
        Serial.printf("[usb] stuck (serial busy %u ms, keyboard/mouse busy %u ms, data waiting %u ms): "
                      "reconnecting USB\n", (unsigned)serial, (unsigned)hid, (unsigned)data);
    }
    asked = false;
    serialStuck.check(false, now);
    hidStuck.check(false, now);
    dataStuck.check(false, now);
    disconnect(now);
    return true;
}

uint32_t reconnects() {
    return reconnectCount;
}

bool serialOpen() {
    return phase == Phase::Watching && (tud_cdc_n_get_line_state(0) & 1);
}

void requestReconnect() {
    asked = true;
}

void requestDriverResetImitation() {
    imitate = true;
}

// ── FIFO layout ───────────────────────────────────────────
void update() {
    uint32_t now = millis();
    if (!tud_mounted() || layoutRight()) {
        layoutWrong.check(false, now);
        gaveUpLogged = false;
        return;
    }
    if (!layoutWrong.holding) logInEndpoints("new configuration");
    uint32_t wrongFor = layoutWrong.check(true, now);
    stopForgottenTransfers(now);
    uint32_t base = fifoBase();
    if (base + FIFO_WORDS * TX_FIFOS > (USB0.ghwcfg3 >> 16)) {
        // Not the layout this fix was made for: leave it alone.
        if (!gaveUpLogged) Serial.printf("[usb] unexpected FIFO layout (EP0 FIFO ends at %u): not fixed\n", base);
        gaveUpLogged = true;
        return;
    }
    if (!fifoUsersIdle()) {
        if (!gaveUpLogged && wrongFor > GIVE_UP_MS) {
            Serial.println("[usb] FIFOs still in use: carrying on with the driver's layout");
            gaveUpLogged = true;
        }
        return;
    }
    fixLayout();
    fixCount++;
    layoutWrong.check(false, now);
    gaveUpLogged = false;
    Serial.printf("[usb] FIFO layout fixed (%u so far): FIFOs 1-4 at %u, %u words each\n",
                  (unsigned)fixCount, (unsigned)base, (unsigned)FIFO_WORDS);
}

bool fifosReady() {
    if (phase != Phase::Watching) return false;
    if (layoutRight()) return true;
    return layoutWrong.holding && millis() - layoutWrong.sinceMs > GIVE_UP_MS;
}

bool hostTxReady() {
    static bool idle = false;
    static uint32_t idleSinceUs = 0;
    if (!fifosReady()) return false;
    if (!current.prevent) return true;
    if (usbd_edpt_busy(0, CDC_DATA_IN)) {
        idle = false;
        return false;
    }
    uint32_t now = micros();
    if (!idle) {
        idle = true;
        idleSinceUs = now;
    }
    return now - idleSinceUs >= IDLE_BEFORE_WRITE_US;
}

uint32_t fixes() {
    return fixCount;
}

uint32_t forgottenTransfersStopped() {
    return forgottenStops;
}

void logFifoLayout() {
    Serial.printf("[usb] snpsid 0x%08X ghwcfg2 0x%08X ghwcfg3 0x%08X (fifo depth %u words) gdfifocfg 0x%08X "
                  "(ep info base %u, total %u)\n",
                  (unsigned)USB0.gsnpsid, (unsigned)USB0.ghwcfg2, (unsigned)USB0.ghwcfg3,
                  (unsigned)(USB0.ghwcfg3 >> 16), (unsigned)USB0.gdfifocfg, (unsigned)(USB0.gdfifocfg >> 16),
                  (unsigned)(USB0.gdfifocfg & 0xFFFF));
    uint32_t otg = REG_READ(USB_WRAP_OTG_CONF_REG);
    Serial.printf("[usb] dctl 0x%08X (soft disconnect %u) | usb_wrap otg_conf 0x%08X (pull-ups set by %s)\n",
                  (unsigned)USB0.dctl, (unsigned)((USB0.dctl >> 1) & 1), (unsigned)otg,
                  ((otg >> 12) & 1) ? "software" : "the controller");
    Serial.printf("[usb] rx fifo: start 0, depth %u | fifo 0 (EP0 in): start %u, depth %u\n",
                  (unsigned)(USB0.grxfsiz & 0xFFFF), (unsigned)(USB0.gnptxfsiz & 0xFFFF),
                  (unsigned)(USB0.gnptxfsiz >> 16));
    for (int f = 0; f < TX_FIFOS; f++) {
        uint32_t v = USB0.dieptxf[f];
        Serial.printf("[usb] fifo %d: start %u, depth %u (end %u)\n", f + 1, (unsigned)(v & 0xFFFF),
                      (unsigned)(v >> 16), (unsigned)((v & 0xFFFF) + (v >> 16)));
    }
    for (int n = 0; n < USB_IN_EP_NUM; n++) {
        uint32_t ctl = USB0.in_ep_reg[n].diepctl;
        Serial.printf("[usb] ep%d in: diepctl 0x%08X active %u type %u fifo %u mps %u | free %u words\n", n,
                      (unsigned)ctl, (unsigned)((ctl >> 15) & 1), (unsigned)((ctl >> 18) & 3),
                      (unsigned)txFifoOf(ctl), (unsigned)(n == 0 ? ctl & 3 : ctl & 0x7FF),
                      (unsigned)(USB0.in_ep_reg[n].dtxfsts & 0xFFFF));
    }
}

void snapshot(uint8_t out[14]) {
    const int n = CDC_DATA_IN_NUM;
    uint8_t flags = 0;
    if (tud_mounted()) flags |= 1 << 0;
    if (tud_suspended()) flags |= 1 << 1;
    if (tud_ready()) flags |= 1 << 2;
    if (tud_cdc_n_connected(0)) flags |= 1 << 3;
    if (current.prevent) flags |= 1 << 4;
    if (layoutRight()) flags |= 1 << 5;
    if (usbd_edpt_busy(0, CDC_DATA_IN)) flags |= 1 << 6;
    uint16_t mask = USB0.dtknqr4_fifoemptymsk;
    uint32_t ctl = USB0.in_ep_reg[n].diepctl;
    uint32_t size = USB0.in_ep_reg[n].dieptsiz;
    uint16_t interrupts = USB0.in_ep_reg[n].diepint;
    uint32_t space = USB0.in_ep_reg[n].dtxfsts & 0xFFFF;  // FIFO words free
    out[0] = flags;
    memcpy(out + 1, &mask, 2);
    memcpy(out + 3, &ctl, 4);
    memcpy(out + 7, &size, 4);
    memcpy(out + 11, &interrupts, 2);
    out[13] = space > 255 ? 255 : space;
}

}  // namespace usb_guard
