"""Which DTR/RTS sequences make the board talk? For each variant: open the
port with the given DTR/RTS, change them as listed, ask for stats, and
count what comes back within 1.5 s.

    python line_state_variants.py PORT
"""
import struct
import sys
import time

import serial

MAGIC, VERSION = 0x4B, 0x01
REQUEST = (struct.pack("<BBBBI", MAGIC, VERSION, 0x40, 0, 0) + bytes([0x04])).ljust(64, b"\0")


def run(name, dtr_at_open, rts_at_open, steps):
    port = serial.Serial()
    port.port = sys.argv[1]
    port.baudrate = 115200
    port.timeout = 0
    port.dtr = dtr_at_open
    port.rts = rts_at_open
    port.open()
    for what, value in steps:
        time.sleep(0.15)
        setattr(port, what, value)
    time.sleep(0.15)
    port.write(REQUEST)
    buf, end = b"", time.time() + 1.5
    while time.time() < end:
        buf += port.read(4096)
        time.sleep(0.01)
    port.close()
    statuses = sum(1 for i in range(0, len(buf) - 2) if buf[i:i + 3] == bytes([MAGIC, VERSION, 0x41]))
    print(f"{name:45s} -> {statuses} status packets, {len(buf)} bytes")
    time.sleep(0.5)


run("both on at open (pyserial default)", True, True, [])
run("both off at open, DTR on, then RTS on", False, False, [("dtr", True), ("rts", True)])
run("both on at open (again)", True, True, [])
run("both off at open, RTS on, then DTR on", False, False, [("rts", True), ("dtr", True)])
run("DTR on at open, then RTS on", True, False, [("rts", True)])
run("RTS on at open, then DTR on", False, True, [("dtr", True)])
run("both on at open (again)", True, True, [])
