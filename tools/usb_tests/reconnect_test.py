"""Measure the USB watchdog's reconnection, asked for with MSG_DAEMON_CMD
0x0A setting 8: open the board's port like the daemon (RTS, then DTR),
check that it answers, ask it to reconnect, and time when the port goes,
when it comes back, and when the board answers again after reopening.

    python reconnect_test.py PORT [RUNS]
"""
import struct
import sys
import time

import serial
from serial.tools import list_ports

MAGIC, VERSION = 0x4B, 0x01
PORT = sys.argv[1]
RUNS = int(sys.argv[2]) if len(sys.argv) > 2 else 3


def command(*data):
    return (struct.pack("<BBBBI", MAGIC, VERSION, 0x40, 0, 0) + bytes(data)).ljust(64, b"\0")


STATS = command(0x04)
RECONNECT = command(0x0A, 8, 0)


def present():
    return any(p.device == PORT for p in list_ports.comports())


def open_port():
    port = serial.Serial()
    port.port = PORT
    port.baudrate = 115200
    port.timeout = 0
    port.write_timeout = 1
    port.dtr = False
    port.rts = False
    port.open()
    port.rts = True     # RTS first, then DTR (docs/platform.md, Windows)
    port.dtr = True
    return port


def first_answer(port, limit):
    """Seconds until a link-stats reply arrives, asking every 0.2 s (None: none)."""
    start = time.time()
    buf, next_ask = b"", start
    while time.time() - start < limit:
        if time.time() >= next_ask:
            port.write(STATS)
            next_ask += 0.2
        buf += port.read(4096)
        while True:
            i = buf.find(bytes([MAGIC, VERSION]))
            if i < 0 or len(buf) - i < 64:
                break
            p, buf = buf[i:i + 64], buf[i + 64:]
            if p[2] == 0x41 and p[8] == 0x03:
                return time.time() - start
        time.sleep(0.005)
    return None


def fmt(t):
    return "never" if t is None else f"{t:5.2f} s"


def wait_back(limit):
    start = time.time()
    while time.time() - start < limit:
        if present():
            return True
        time.sleep(0.05)
    return False


spontaneous = 0
for run in range(1, RUNS + 1):
    try:
        port = open_port()
        ok = first_answer(port, 5) is not None
    except (serial.SerialException, OSError) as e:
        # The device went away on its own (the computer reset it): count
        # it, wait for it, and go on.
        spontaneous += 1
        print(f"run {run}: device went away on its own before the request ({e})", flush=True)
        try:
            port.close()
        except Exception:
            pass
        time.sleep(0.5)
        if not wait_back(15):
            print("  and did not come back")
            break
        time.sleep(1)
        continue
    if not ok:
        print(f"run {run}: no answer before asking to reconnect")
        port.close()
        break
    t0 = time.time()
    port.write(RECONNECT)
    gone = None
    last_check = 0.0
    while time.time() - t0 < 5:
        try:
            port.read(4096)
        except (serial.SerialException, OSError):
            gone = time.time() - t0
            break
        if time.time() - last_check > 0.05:
            last_check = time.time()
            if not present():
                gone = time.time() - t0
                break
        time.sleep(0.005)
    try:
        port.close()
    except Exception:
        pass
    back = answered = None
    while time.time() - t0 < 15:
        if present():
            back = time.time() - t0
            break
        time.sleep(0.02)
    if back is not None:
        port = None
        for _ in range(100):
            try:
                port = open_port()
                break
            except (serial.SerialException, OSError):
                time.sleep(0.05)
        if port is not None:
            opened = time.time() - t0
            a = first_answer(port, 10)
            answered = None if a is None else opened + a
            port.close()
    print(f"run {run}: port gone after {fmt(gone)}, back after {fmt(back)}, answering after {fmt(answered)}",
          flush=True)
    time.sleep(1)
print(f"spontaneous disconnects: {spontaneous}")
