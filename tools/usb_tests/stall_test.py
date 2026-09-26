"""Make the board's serial IN endpoint stay busy and see whether the USB
watchdog notices by itself: open the port like the daemon, check that the
board answers, switch the filler on (a status packet every 2 ms, setting
7), then stop reading. Times when the port goes and comes back, reopens
it, checks the answer and switches the filler off.

    python stall_test.py PORT [SECONDS_WITHOUT_READING]
"""
import struct
import sys
import time

import serial
from serial.tools import list_ports

MAGIC, VERSION = 0x4B, 0x01
PORT = sys.argv[1]
QUIET = float(sys.argv[2]) if len(sys.argv) > 2 else 8


def command(*data):
    return (struct.pack("<BBBBI", MAGIC, VERSION, 0x40, 0, 0) + bytes(data)).ljust(64, b"\0")


STATS = command(0x04)


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
    port.rts = True
    port.dtr = True
    return port


def first_answer(port, limit):
    start = time.time()
    buf, next_ask = b"", start
    while time.time() - start < limit:
        if time.time() >= next_ask:
            port.write(STATS)
            next_ask += 0.2
        buf += port.read(65536)
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


for attempt in range(5):
    try:
        port = open_port()
        if first_answer(port, 5) is None:
            sys.exit("no answer at the start")
        break
    except (serial.SerialException, OSError) as e:
        print(f"device went away on its own while opening ({e}); trying again", flush=True)
        time.sleep(0.5)
        while not present():
            time.sleep(0.05)
        time.sleep(1)
else:
    sys.exit("could not start")
port.write(command(0x0A, 7, 1))
t0 = time.time()
gone = None
while time.time() - t0 < QUIET:
    if not present():
        gone = time.time() - t0
        break
    time.sleep(0.05)
try:
    port.close()
except Exception:
    pass
print(f"filler on, not reading: port gone after {fmt(gone)}", flush=True)
if gone is None:
    port = open_port()
    port.write(command(0x0A, 7, 0))
    print("answer:", fmt(first_answer(port, 5)))
    port.close()
    sys.exit()
back = None
while time.time() - t0 < gone + 15:
    if present():
        back = time.time() - t0
        break
    time.sleep(0.02)
print(f"port back after {fmt(back)}", flush=True)
port = None
for _ in range(100):
    try:
        port = open_port()
        break
    except (serial.SerialException, OSError):
        time.sleep(0.05)
opened = time.time() - t0
a = first_answer(port, 10)
print(f"answering after {fmt(None if a is None else opened + a)} (the filler is still on)", flush=True)
port.write(command(0x0A, 7, 0))
time.sleep(0.3)
port.read(65536)
print("filler off; answer:", fmt(first_answer(port, 5)))
port.close()
