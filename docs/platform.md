# Platform facts and limits

What the board, the chip and the software under the firmware can and
cannot do, each with its source: an official document, a file in the
toolchain as installed, or a measurement with its date. Designs and code
rely only on facts written here; one not known yet is measured before it
is relied on (requirement P1).

This document was started on 2026-09-26, after a board kept stopping under
load (experiment A5) because of defects in the USB driver underneath the
firmware, which nobody had checked. Section 4 lists them.

## 1. The board

| Fact | Source |
|---|---|
| ESP32-S3-DevKitC-1 **v1.1**, module ESP32-S3-WROOM-1 **N8R8**: 8 MB quad flash, 8 MB octal PSRAM | [DevKitC-1 v1.1 user guide](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s3/esp32-s3-devkitc-1/user_guide_v1.1.html); board label |
| Chip: ESP32-S3 (QFN56), revision **v0.2** | esptool output when flashing |
| Two Micro-USB ports: **"UART"** (USB-to-UART bridge; Windows lists it as Silicon Labs CP210x) and **"USB"** (the chip's own full-speed USB OTG, USB 1.1) | user guide; Windows device list |
| Power from either port or both at once; a 5 V → 3.3 V LDO | user guide |
| RGB LED (WS2812) on **GPIO 38** (GPIO 48 on v1.0 boards) | user guide; silkscreen "RGB@IO38" |
| Buttons: **Boot** (held while pressing Reset: download mode) and **Reset** | user guide |
| PSRAM is **not enabled** by this build (PlatformIO board `esp32-s3-devkitc-1`: flash mode qio, quad memory type; the module's PSRAM is octal). Nothing needs it. | `~/.platformio/platforms/espressif32/boards/esp32-s3-devkitc-1.json` |

## 2. The chip

| Fact | Source |
|---|---|
| Two Xtensa LX7 cores at **240 MHz** | sdkconfig `CONFIG_ESP32S3_DEFAULT_CPU_FREQ_MHZ 240` |
| No errata about USB data transfers or ESP-NOW for revision v0.2 (the only USB erratum is about the USB-OTG download function). CACHE-126 (cache write-back hit error) concerns external memory, which this build does not use. | [ESP32-S3 errata](https://docs.espressif.com/projects/esp-chip-errata/en/latest/esp32s3/03-errata-description/index.html) |

### USB OTG controller (Synopsys DWC_OTG), device mode

| Fact | Source |
|---|---|
| Full speed only (12 Mbit/s) | user guide; [DWC_OTG notes](https://docs.espressif.com/projects/esp-usb/en/latest/esp32s3/usb_host/usb_host_notes_dwc_otg.html) |
| At most **6 endpoints** besides EP0: 5 bidirectional and 1 IN only | [ESP-IDF USB device driver](https://docs.espressif.com/projects/esp-idf/en/v5.0/esp32s3/api-reference/peripherals/usb_device.html) |
| Transmit FIFOs: FIFO 0 for EP0 and **FIFOs 1–4** for other IN endpoints (`DIEPTXF1–4`) | `soc/usb_struct.h` (`dieptxf[4]`) |
| FIFO memory: **256 words** (1024 bytes), of which only the first **200** can hold FIFOs; the rest stores the controller's endpoint state | DWC_OTG notes ("Data FIFO of 1024 bytes (256 lines)"); ESP-IDF `hal/usb_dwc_hal.h` ("only 200 lines are usable due to EPINFO_CTL"); registers read 2026-09-26: GHWCFG3 depth 200, GDFIFOCFG EP info base 200 |

### Wi-Fi and ESP-NOW (ESP-IDF 4.4)

| Fact | Source |
|---|---|
| Payload at most **250 bytes** (this project sends 64-byte packets) | [ESP-NOW, ESP-IDF v4.4.7](https://docs.espressif.com/projects/esp-idf/en/v4.4.7/esp32s3/api-reference/network/esp_now.html) |
| At most 20 peers, 7 of them encrypted by default (this project encrypts itself and uses one unencrypted peer) | same |
| The send callback reports delivery to the other chip's radio only, not to its application: the project's own acknowledgement (`MSG_INPUT_ACK`) covers that | same |
| Send and receive callbacks run in the high-priority Wi-Fi task and must only hand data over (the project queues it for the main loop) | same |

## 3. The software under the firmware

| Fact | Source |
|---|---|
| PlatformIO `espressif32`, **Arduino core 2.0.17** (package 3.20017.241212), built on **ESP-IDF v4.4.7** | package.json; sdkconfig `CONFIG_ARDUINO_IDF_BRANCH` |
| USB stack: **TinyUSB 0.16.0** (device core and classes) with the ESP32-S2/S3 driver **`dcd_esp32sx.c`**, precompiled (`libarduino_tinyusb.a`). The driver is **Espressif's own copy** in the library builder, not TinyUSB's file: the same code plus error logs ("Unknown Condition", "Complete but not empty", "XFER Timeout") and a reset of its own (K6). Later TinyUSB versions replaced this driver. | `tusb_option.h`; `ar t libarduino_tinyusb.a` and the strings in it; [builder `CMakeLists.txt`](https://github.com/espressif/esp32-arduino-lib-builder/blob/release/v4.4/components/arduino_tinyusb/CMakeLists.txt) (compiles `src/dcd_esp32sx.c`, TinyUSB's own copy commented out); [driver source](https://github.com/espressif/esp32-arduino-lib-builder/blob/release/v4.4/components/arduino_tinyusb/src/dcd_esp32sx.c) |
| Endpoints the Arduino core gives this firmware: HID (keyboard + mouse) IN **EP3** → FIFO 1; serial data IN **EP4** → FIFO 2 (address fixed at 0x84); serial notification **EP5** → "FIFO 5", which does not exist (never used) | `cores/esp32/USBCDC.cpp` (`load_cdc_descriptor`); registers read 2026-09-26 |
| TinyUSB serial FIFOs: **64 bytes** each way; the Arduino receive queue in front of it is set to 4096 bytes by the firmware | sdkconfig `CONFIG_TINYUSB_CDC_RX_BUFSIZE/TX_BUFSIZE 64`; `firmware/src/main.cpp` |
| On a bus reset, `dcd_esp32sx` empties the transmit FIFOs but neither disables the IN endpoints nor turns their FIFO-empty interrupts (DIEPEMPMSK) off. Its interrupt handler fills an IN endpoint's FIFO from the endpoint's remaining transfer size whenever that endpoint's interrupt is up and its FIFO is empty, without looking at the mask. So a transfer that was under way survives a reset, and can refill the FIFO after it (K7). | [`dcd_esp32sx.c`](https://github.com/espressif/esp32-arduino-lib-builder/blob/release/v4.4/components/arduino_tinyusb/src/dcd_esp32sx.c) (`bus_reset`, `handle_epin_ints`, `transmit_packet`); TinyUSB 0.16.0's copy is the same there |
| Disconnecting from USB without unplugging: `tud_disconnect()` / `tud_connect()` set and clear the controller's soft-disconnect bit (DCTL SftDiscon). The chip's internal PHY takes its D+ pull-up from the controller unless software overrides it (`USB_WRAP` `otg_conf.pad_pull_override`), which the Arduino core does not do in device mode. Measured: both computers see the device leave and come back, and configure it again (section 5). | [`usbd.c`](https://github.com/hathach/tinyusb/blob/0.16.0/src/device/usbd.c), `dcd_esp32sx.c` (`dcd_disconnect`); `soc/usb_wrap_struct.h`, `hal/usb_phy_ll.h` |
| After a bus reset, TinyUSB's serial line state (DTR, RTS) is cleared, but the Arduino driver's "connected" flag (`USBCDC::operator bool`) is not: it goes off only when DTR does, or on an unplug event, which a bus reset does not send. The firmware counts a daemon as connected only when both are on. | [`cdc_device.c`](https://github.com/hathach/tinyusb/blob/0.16.0/src/class/cdc/cdc_device.c) (`ITF_MEM_RESET_SIZE`, `cdcd_reset`); `usbd.c` (`DCD_EVENT_BUS_RESET`); `cores/esp32/USBCDC.cpp` |
| `USBCDC::write()` waits, with no time limit, while the TinyUSB FIFO is full | `cores/esp32/USBCDC.cpp` |
| After every packet received, every transfer sent, and every line state or line coding change, the Arduino serial driver posts an event to its event loop and **waits with no time limit** while that loop's queue (**5 events**) is full. The loop runs in task `arduino_usb_events`, priority 5, on either core. | `cores/esp32/USBCDC.cpp` (`portMAX_DELAY`); `cores/esp32/USB.cpp` |
| `USBHID::SendReport()` waits until the host has taken the report: **up to 100 ms** (default timeout). HID endpoints are polled every 1 ms (`bInterval` 1): at most 1000 reports per second. | `libraries/USB/src/USBHID.cpp`, `USBHID.h` |
| Main loop (`loopTask`): core 1, priority 1, stack 8 KB | sdkconfig `CONFIG_ARDUINO_RUNNING_CORE 1`, `CONFIG_ARDUINO_LOOP_STACK_SIZE 8192` |
| USB task (`usbd`, runs `tud_task`): priority 24, **not pinned to a core**, stack 4 KB; the USB interrupt is set up on the core that calls `USB.begin()` (core 1) | `cores/esp32/esp32-hal-tinyusb.c` (`xTaskCreate`); `dcd_int_enable` |
| Watchdogs: task watchdog 5 s, panics, watches only core 0's idle task; interrupt watchdog 300 ms | sdkconfig `CONFIG_ESP_TASK_WDT_*`, `CONFIG_ESP_INT_WDT_*` |
| FreeRTOS tick 1 kHz | sdkconfig `CONFIG_FREERTOS_HZ 1000` |
| Windows (in-box `usbser.sys`): the board receives "DTR and RTS on" only when DTR is raised while RTS already is on (or both are set at once when the port opens). Raising RTS after DTR leaves the board seeing DTR off, so it never counts a daemon as connected and sends nothing. The daemon therefore raises RTS first. (Measured 2026-09-26 with every order: DTR then RTS fails; RTS then DTR, both at open, or DTR raised again afterwards all work. The cause inside the driver is inferred, not documented.) | `tools/usb_tests/line_state_variants.py` runs; `cores/esp32/USBCDC.cpp` (`_onLineState` needs both) |
| macOS: closing the port (the process ending included) lowers DTR, and opening raises DTR and RTS: the board notices every daemon that comes and goes. But the serial library discards received data right after opening (`tcflush`), so the board's report sent the moment the daemon connects can be lost; the daemon takes the link state from the answer to its first request instead. | measured 2026-09-26 (`tools/usb_tests/mac_hupcl_test.py`: HUPCL on by default, the board reports each reopen); serialport 4.10 `posix/tty.rs` |
| Task names are cut to **15 characters**, and `xTaskGetHandle()` **asserts** (the board restarts) when asked for a longer name. The core's event task `arduino_usb_events` exists as `arduino_usb_eve`. (Learned 2026-09-26 by crashing a board with the long name.) | sdkconfig `CONFIG_FREERTOS_MAX_TASK_NAME_LEN 16`; FreeRTOS `tasks.c` |

## 4. Known defects underneath, and what the firmware does about them

| # | Defect | Effect | Handling |
|---|---|---|---|
| K1 | `dcd_esp32sx` configures the transmit FIFO whose number is the endpoint's instead of the endpoint's own FIFO, and plans for 256 words. The FIFOs actually used (1 and 2) keep hardware defaults at words 512 and 768, outside the FIFO memory; they wrap around onto the receive FIFO. | Data to and from the computer overwrite each other under load; the serial IN endpoint was left holding a packet it never sent, and the board stopped sending to its computer for good (A5). | Fixed: `firmware/src/usb_guard.cpp` lays FIFOs 1–4 out inside the 200 words after each configuration and holds IN transfers until then. Verified 2026-09-26: 900 s under load without a stop (before: stops within ~10 s). |
| K2 | `dcd_esp32sx` changes shared endpoint registers (e.g. the FIFO-empty interrupt mask) with unlocked read-modify-writes, from the USB interrupt and from tasks; the USB task is not pinned, so it can run on the other core at the same moment. | A lost update would leave an endpoint waiting for ever. Not observed. | Serial writes wait until the endpoint has been idle for 1 ms (`prevent`), so only the main loop, on the interrupt's core, starts data transfers there. |
| K3 | The Arduino serial driver waits with no time limit for room in a 5-event queue (section 3). | If that task falls behind, the USB task stops. Not observed as such (see K6). | None. |
| K4 | `USBCDC::write()` waits with no time limit while the FIFO is full. | The main loop could stall if the computer stops reading. | The firmware writes only what fits (`availableForWrite`). |
| K5 | `USBHID::SendReport()` waits up to 100 ms. | The main loop stalls while the computer does not take reports (requirement F1 allows 5 ms). | Reports are sent only when the interface is ready; to be measured. |
| K7 | When the computer configures the device anew while a transfer to it is pending on the serial endpoint, TinyUSB forgets the transfer (clears its busy flag) but the endpoint stays enabled with part of the old packet in its FIFO; the next transfer is then programmed over it, and the endpoint waits for ever (seen: EPENA=1, XFERSIZE=64, PKTCNT=1, 12 words left in the FIFO, the FIFO-empty interrupt never comes). | Seen on board 2 on the Mac, 2026-09-26, after the board was unplugged and plugged back in while the focus was being switched: its serial link to the Mac stayed dead (HID kept working) until the board was replugged. Windows resets a device that stops answering (K6); macOS did not. | A watchdog in `usb_guard.cpp`: when, for 1.5 s while the computer has the device configured and awake, the serial IN endpoint stays busy with the port open, the keyboard/mouse IN endpoint stays busy, or data for the daemon stops moving, the board disconnects from USB, stops every IN endpoint as the controller requires and empties the FIFOs, and connects again 0.5 s later; the computer then resets and configures it anew. At most once per 10 s. Verified 2026-09-26 (section 5). Probable cause of the case seen: K6, which leaves the old transfer behind; the firmware now stops such forgotten transfers right after each new configuration, so the watchdog is the fallback. Later (roadmap, Phase 5): a corrected driver. |
| K6 | Now and then the board stops answering the computer while the computer opens or closes its serial port; the computer then resets the device and configures it again. Found 2026-09-26 on board 2 (UART log): at that moment Espressif's driver logs `TUSB:DCD: Unknown Condition`. Its IN endpoint interrupt handler does this when an endpoint's DIEPINT bit 15 is set (a bit this chip's register description does not list), and then runs its bus-reset routine, which among other things clears the device's address: the device stops answering until the computer resets it. The same log is reported with the same core and chip in [arduino-esp32 #11600](https://github.com/espressif/arduino-esp32/issues/11600) (unanswered). What sets the bit is not known. The reset leaves transfers under way enabled (K7). | Opening the port fails (Windows error 31), or the first write right after opening times out. Measured on one board, 2026-09-26, opening and closing the port with a stats request each time while the board sent the computer 100 packets/s: 7 failures in 2000; at 500 packets/s plus a burst from the computer: 7 in 1200. Every failure came with the computer configuring the device anew. None seen with the port kept open (25 min under heavy load in both directions). Once, right after a cable was unplugged from the board's other port, opening failed for minutes, until the board was plugged in again. On the Mac it is much more frequent: 4 in about 17 openings by a test script (2026-09-26), each at the moment the port opened. Afterwards an old transfer was left enabled: once its 64-byte packet went to the Mac when the port next opened (`Complete but not empty: 0/64`), once the FIFO layout fix waited a second for it (`FIFOs still in use`), which is the state K7 starts from. | The daemon opens the port once per board connection, and after a failed open or write it reconnects within 2 s. Tried without effect: the USB task pinned to the interrupt's core (1 in 600, against 1 in 600), and the Arduino event task (K3) raised to priority 24 (5 in 1800, against 7 in 1200). Since 2026-09-26 the firmware stops the transfers such a reset leaves behind as soon as the computer has configured the device again (`usb_guard.cpp`, "forgotten transfers"; section 5). The reset itself can only be removed in the driver (roadmap, Phase 5). |

## 5. Measured

The scripts named below are in `tools/usb_tests/`.

| Quantity | Value | Date, how |
|---|---|---|
| Radio round trip (heartbeat) | 1.7–5.5 ms, mean ~2.5 ms | 2026-09-26, board logs |
| ChaCha20-Poly1305 per packet | seal ~160 µs, open ~110 µs | 2026-09-26, board logs |
| Input packets one board forwards over the radio | ~570/s alone; ~300/s while also relaying 70 messages/s | A5, 2026-09-26 |
| Serial packets a board delivers to its computer | ≥113/s sustained with `prevent` (limit about one 64-byte transfer per ~2–3 ms) | 2026-09-26, `usb_stress.py` |
| Keyboard reports | 10 ms apart at least; faster garbles Korean IME input on Windows | Phase 1 |
| USB reconnection by the watchdog (0.5 s disconnected), asked for with setting 8 | The computer notices the disconnect after 0.02 s (Windows) / 0.13 s (macOS); the port is back after ~0.85 s / ~0.9 s, and the board answers right after. Every time the board saw the new configuration; every IN endpoint confirmed stopping in time. Windows 5 of 5, macOS 4 of 4. | 2026-09-26, `reconnect_test.py` and board logs |
| Serial port open, nobody reading, board sending 32 KB/s | Windows stops taking data once ~12 KB are waiting: the serial IN endpoint stays busy, and the watchdog reconnected after 1.5 s (then all well). macOS keeps taking data and discards what nobody reads: the endpoint never stays busy, so a daemon that stops reading cannot set the watchdog off there. | 2026-09-26, `stall_test.py` |
| Watchdog under load: 60 s of ~930 packets/s to the board and ~113/s to the computer | No reconnection | 2026-09-26, `usb_stress.py` on board 1 |
| What a soft disconnect does to a transfer under way | Disconnecting with the serial IN endpoint enabled, busy for TinyUSB and its FIFO-empty interrupt on: 0.5 s later, still disconnected and with nothing stopped by the firmware, all three were cleared (4 of 4, macOS). The watchdog stops the endpoints anyway. | 2026-09-26, board 2, UART log |
| The driver's own reset (K6), imitated (setting 9) while a serial transfer is under way | The computer resets the device ~0.6 s later. After the new configuration the serial IN endpoint is still enabled with 0 bytes and 1 packet left (the state A5 stopped in), while TinyUSB counts it idle; the firmware stopped it each time, and the board then sent ~1000 packets in 2 s. 5 of 5; 2 real K6 in the same run, one of which left a transfer behind (stopped too). No old data reached the Mac, no FIFO layout fix waited. | 2026-09-26, board 2 on the Mac, `leftover_test.py` |
| K6 on the Mac | ~1 in 40 openings with the board idle; 2 in ~13 while it sent 500 packets/s | 2026-09-26, `open_cycles_mac.py`, `leftover_test.py` |
| What each computer does when the device stops answering (its address lost, as in K6) | Windows resets and configures it again within 0.4–0.9 s and keeps the COM port; a program with the port open gets errors meanwhile ("The semaphore timeout period has expired", os error 121; "The device does not recognize the command"). macOS reset it ~0.6 s later when no program had the port open, but with the daemon holding the port open it had not reset it after 2.3 s, when the USB watchdog reconnected the device. | 2026-09-26, imitated K6 (setting 9 / UART `usb k6`), boards' UART logs and daemon logs |

## 6. Not known yet: to measure before relying on it

| Quantity | Why it matters |
|---|---|
| Time from a board's HID report to the daemon's capture seeing it (Windows low-level hook, macOS event tap), typical and worst under load | The settle time (R2, 20 ms) must outlast it, or input a board typed in can be sent back (S3). |
| How long Windows lets a low-level hook callback take, and what it does after (skip, or remove the hook) | D3, D4: a slow callback must never cost the capture. |
| How long macOS lets an event tap callback take before it disables the tap | same |
| Longest time `USBHID::SendReport()` actually blocks the main loop | F1, K5 |
| USB suspend and resume (computer asleep, display off): what the board sees, and whether the serial port and HID work after waking | B8, D6 |
