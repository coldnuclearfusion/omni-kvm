"""Load a board's USB serial link both ways and watch it.

    python usb_stress.py PORT SECONDS PREVENT STATS_PER_S

Sets the board's "prevent" setting (PREVENT: 0 or 1; see
firmware/include/usb_guard.h) and its diagnostic reports on, then sends
zero-length mouse moves as fast as it can (load towards the board) and
asks for link stats STATS_PER_S times a second (each answered by a
64-byte packet: load towards this computer). A write the board does not
take within 0.5 s is counted, not fatal. Prints per 5 s.
"""
import struct
import sys
import time

import serial

MAGIC, VERSION = 0x4B, 0x01


def packet(msg_type, payload):
    return (struct.pack("<BBBBI", MAGIC, VERSION, msg_type, 0, 0) + payload).ljust(64, b"\0")


def guard(setting, on):
    return packet(0x40, bytes([0x0A, setting, 1 if on else 0]))


port = serial.Serial(sys.argv[1], 115200, timeout=0, write_timeout=0.5)
seconds = float(sys.argv[2])
prevent = sys.argv[3] == "1"
stats_interval = 1.0 / float(sys.argv[4])
timeouts = 0


def write(data):
    global timeouts
    try:
        port.write(data)
        return True
    except serial.SerialTimeoutException:
        timeouts += 1
        return False


write(guard(1, prevent) + guard(3, True))
print(f"prevent {'on' if prevent else 'off'}, {1 / stats_interval:.0f} stats requests/s", flush=True)

move = packet(0x01, struct.pack("<hhB", 0, 0, 0))
stats_request = packet(0x40, bytes([0x04]))
start = last_reply = last_ask = time.perf_counter()
replies = moves = 0
window = {"replies": 0, "timeouts": 0}
next_report = 5.0
longest_gap = 0.0
buf = b""
while True:
    now = time.perf_counter()
    t = now - start
    if t >= seconds:
        break
    before = timeouts
    if write(move):
        moves += 1
    if now - last_ask >= stats_interval:
        write(stats_request)
        last_ask = now
    window["timeouts"] += timeouts - before
    buf += port.read(8192)
    while True:
        i = buf.find(bytes([MAGIC, VERSION]))
        if i < 0 or len(buf) - i < 64:
            buf = buf[i:] if i >= 0 else b""
            break
        p, buf = buf[i:i + 64], buf[i + 64:]
        if p[2] == 0x41 and p[8] == 0x03:
            replies += 1
            window["replies"] += 1
            longest_gap = max(longest_gap, now - last_reply)
            last_reply = now
    if t >= next_report:
        print(f"t={next_report:5.1f} s  replies {window['replies']:4d}  write timeouts {window['timeouts']:3d}",
              flush=True)
        window = {"replies": 0, "timeouts": 0}
        next_report += 5.0
    time.sleep(0.0005)

write(guard(3, False))
gap_now = time.perf_counter() - last_reply
print(f"{seconds:.0f} s: {moves} moves sent, {replies} stats replies, {timeouts} write timeouts")
print(f"longest gap between replies {longest_gap * 1000:.0f} ms; last reply {gap_now * 1000:.0f} ms ago")
print("RESULT:", "board kept answering" if gap_now < 1 else "BOARD STOPPED ANSWERING")
