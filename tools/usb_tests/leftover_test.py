"""Test that transfers left behind by a reset are stopped: with the filler
on (a status packet every 2 ms), ask the board to reconnect the way the
driver's own reset leaves things (setting 9), then reopen the port and
check that packets keep coming for 2 s (the serial endpoint not blocked).

    python leftover_test.py PORT RUNS
"""
import struct
import sys
import time

import serial
from serial.tools import list_ports

MAGIC, VERSION = 0x4B, 0x01
PORT, RUNS = sys.argv[1], int(sys.argv[2])


def command(*data):
    return (struct.pack("<BBBBI", MAGIC, VERSION, 0x40, 0, 0) + bytes(data)).ljust(64, b"\0")


def present():
    return any(p.device == PORT for p in list_ports.comports())


def open_port():
    for _ in range(100):
        try:
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
        except (serial.SerialException, OSError):
            time.sleep(0.05)
    raise SystemExit("could not open the port")


def packets_for(port, seconds):
    end, count, buf = time.time() + seconds, 0, b""
    while time.time() < end:
        buf += port.read(65536)
        time.sleep(0.01)
    return buf.count(bytes([MAGIC, VERSION]))


def wait_gone_and_back(limit=15):
    start = time.time()
    while present() and time.time() - start < 10:
        time.sleep(0.02)
    gone = time.time() - start
    while not present() and time.time() - start < limit:
        time.sleep(0.02)
    return gone, time.time() - start


good = 0
for run in range(1, RUNS + 1):
    try:
        port = open_port()
        port.write(command(0x0A, 7, 1))
        before = packets_for(port, 0.5)
        port.write(command(0x0A, 9, 0))
        time.sleep(0.2)
        try:
            port.close()
        except Exception:
            pass
    except (serial.SerialException, OSError) as e:
        print(f"run {run}: device went away early ({e})", flush=True)
        wait_gone_and_back()
        time.sleep(1)
        continue
    gone, back = wait_gone_and_back()
    time.sleep(0.3)
    try:
        port = open_port()
        after = packets_for(port, 2)
        port.write(command(0x0A, 7, 0))
        time.sleep(0.2)
        port.close()
    except (serial.SerialException, OSError) as e:
        after = None
        print(f"run {run}: device went away after reopening ({e})", flush=True)
        wait_gone_and_back()
    ok = after is not None and after > 200
    good += ok
    print(f"run {run}: {before} packets in 0.5 s before; gone after {gone:.2f} s, back after {back:.2f} s; "
          f"{after} packets in 2 s after reopening -> {'ok' if ok else 'BLOCKED?'}", flush=True)
    time.sleep(1)
print(f"{good} of {RUNS} runs kept sending after the reconnect")
