// ============================================================
// USB guard — fixes the Arduino core's USB FIFO layout
// ============================================================
// The Arduino core 2.0.x ships TinyUSB 0.16 with Espressif's own copy of
// its ESP32-S2/S3 device driver (dcd_esp32sx.c, from the library builder;
// docs/platform.md, section 3). When the host configures the device, the driver
// gives each IN endpoint (data going to the host) a transmit FIFO, but it
// writes that FIFO's place and size into the register of the FIFO whose
// number is the endpoint's, not the FIFO's own. With the core's endpoints
// (keyboard/mouse on EP3, serial port on EP4 and EP5), the two FIFOs in
// use keep their hardware defaults: words 512–767 and 768–1023. The FIFO
// memory has 256 words, of which only the first 200 are for FIFOs (the
// rest holds the core's endpoint state: GHWCFG3, GDFIFOCFG). The
// addresses wrap around onto the receive FIFO, so data going to the board
// and data going to the host overwrite each other. Under load (experiment
// A5) the serial port's IN endpoint was left holding a packet it never
// sent (EPENA=1, XFERSIZE=0, PKTCNT=1): the board stopped sending to its
// host for good, within about 10 s. (Register dump, 2026-09-26: rx
// 0–51, EP0 52–67, fifo 1 at 512 and fifo 2 at 768, 256 words each;
// fifos 3/4 at 68 and 115, used by no endpoint.)
//
// After each (re)configuration, once no transfer is using them, transmit
// FIFOs 1–4 get their own places inside the 200 words and are flushed.
// Until then fifosReady() is false and the board starts no keyboard,
// mouse or serial transfer, for at most GIVE_UP_MS.
//
// Before that, any IN transfer left over from before the reset is stopped
// (docs/platform.md, K6, K7). The driver leaves such transfers enabled
// while TinyUSB forgets them; one would reach the computer as old data, or
// have a new transfer programmed over it and block its endpoint for good.
// TinyUSB marks every transfer it starts busy before the driver enables
// it, so an enabled endpoint TinyUSB counts as idle holds a forgotten one.
//
// A second hazard, found by reading the driver rather than seen: starting
// a transfer sets the endpoint's bit in the "IN FIFO empty" interrupt mask
// with an unlocked read-modify-write, and the interrupt clears bits of
// that register the same way. The driver's USB task, which starts the
// next serial transfer when one completes with data still waiting, is not
// pinned to a core, so it can run on the other core at the very moment the
// interrupt runs; a bit it sets can then be lost, and that endpoint would
// wait for ever. With `prevent` on, data for the host is written only once
// the serial endpoint has been idle for a moment: no data is ever waiting
// when a transfer completes, so every data transfer is started by this
// loop, on the interrupt's own core, where the interrupt cannot interleave
// with it that way.
//
// A watchdog covers what is left (docs/platform.md, K7). While the host
// has the device configured and awake, if for STUCK_MS the serial IN
// endpoint stays busy with the port open, or the keyboard/mouse IN
// endpoint stays busy, or data for the daemon waits without the driver
// taking any, the board disconnects from USB (DCTL soft disconnect,
// tud_disconnect()). Disconnected, it stops every IN endpoint the way the
// controller requires, turns their FIFO-empty interrupts off and empties
// the transmit FIFOs, which the driver's own bus reset does not do: that
// is how an old transfer outlives a reset. (Measured: the disconnect by
// itself dropped the transfer under way; the endpoints are stopped
// anyway.) After RECONNECT_OFF_MS it connects again, and the host resets
// and configures the device anew.
// From the disconnect until that new configuration, fifosReady() and
// serialOpen() are false: no IN transfer starts, and the daemon counts as
// gone. At most once per RECONNECT_MIN_INTERVAL_MS.
// ============================================================

#pragma once

#include <stdint.h>

namespace usb_guard {

struct Settings {
    // Write to the host only once the serial IN endpoint has been idle for
    // a moment (see above). Costs throughput (one 64-byte transfer per
    // ~2 ms), which the daemons do not need.
    bool prevent = true;
};

Settings &settings();

// Checks the FIFO layout, and fixes it when it is wrong and no transfer
// uses the FIFOs. Call at the start of every loop pass.
void update();

// May an IN transfer (keyboard, mouse, serial) start? False while the
// layout waits to be fixed.
bool fifosReady();

// May data for the host be written now? Call before each write.
bool hostTxReady();

// Times the layout was fixed since boot (once per configuration).
uint32_t fixes();

// Forgotten IN transfers stopped since boot (see above).
uint32_t forgottenTransfersStopped();

// Watchdog. Call on every loop pass with whether the daemon is connected
// and has data waiting, and the bytes the USB driver has taken for the
// host so far. Returns true on the pass that disconnects: the host then
// lets go of every key and button the board held down.
bool watch(bool hostDataWaiting, uint32_t hostBytesTaken);

// USB reconnections since boot.
uint32_t reconnects();

// The host has the serial port open (DTR on), as TinyUSB last saw it.
// Unlike the Arduino driver's flag, this is cleared when the host resets
// the device. False while the watchdog reconnects.
bool serialOpen();

// Development: reconnect on the next watch(), stuck or not.
void requestReconnect();

// Development: once a serial transfer is under way, do what the driver's
// own reset does (K6): the device forgets its address, and the computer
// resets it and configures it anew, which leaves the transfer behind. To
// test that forgotten transfers are stopped after the new configuration.
void requestDriverResetImitation();

// Development: prints the USB core's FIFO layout to the log (UART).
void logFifoLayout();

// Development: 14 bytes on the state of the serial port's IN endpoint,
// bytes 1–14 of MSG_BOARD_REPORT (shared/protocol.md).
void snapshot(uint8_t out[14]);

}  // namespace usb_guard
