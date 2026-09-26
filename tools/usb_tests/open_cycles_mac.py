"""Open the board's port like the daemon N times (RTS, then DTR), ask for
link stats until answered (up to 3 s), close, and wait a second. Counts
openings that failed (the device went away, or no answer), waiting for
the device to come back after one.

    python open_cycles_mac.py PORT N
"""
import struct
import sys
import time

import serial
from serial.tools import list_ports

MAGIC, VERSION = 0x4B, 0x01
PORT, N = sys.argv[1], int(sys.argv[2])
STATS = (struct.pack("<BBBBI", MAGIC, VERSION, 0x40, 0, 0) + bytes([0x04])).ljust(64, b"\0")


def present():
    return any(p.device == PORT for p in list_ports.comports())


def cycle():
    port = serial.Serial()
    port.port = PORT
    port.baudrate = 115200
    port.timeout = 0
    port.write_timeout = 1
    port.dtr = False
    port.rts = False
    port.open()
    try:
        port.rts = True
        port.dtr = True
        start, next_ask, buf = time.time(), time.time(), b""
        while time.time() - start < 3:
            if time.time() >= next_ask:
                port.write(STATS)
                next_ask += 0.2
            buf += port.read(4096)
            i = buf.find(bytes([MAGIC, VERSION, 0x41, 0]))
            while i >= 0 and len(buf) - i >= 64:
                if buf[i + 8] == 0x03:
                    return time.time() - start
                buf = buf[i + 64:]
                i = buf.find(bytes([MAGIC, VERSION, 0x41, 0]))
            time.sleep(0.005)
        return None
    finally:
        try:
            port.close()
        except Exception:
            pass


ok = failed = 0
worst = 0.0
for k in range(1, N + 1):
    try:
        t = cycle()
    except (serial.SerialException, OSError) as e:
        t = None
        print(f"open {k}: device went away ({e})", flush=True)
    if t is None:
        failed += 1
        time.sleep(0.5)
        waited = time.time()
        while not present() and time.time() - waited < 15:
            time.sleep(0.05)
        print(f"open {k}: failed; device back after {time.time() - waited:.2f} s", flush=True)
    else:
        ok += 1
        worst = max(worst, t)
    time.sleep(1)
print(f"{ok} answered (slowest {worst:.2f} s), {failed} failed, of {N}")
