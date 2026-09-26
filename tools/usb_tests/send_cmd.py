"""Send one DAEMON_CMD to a board.

    python send_cmd.py PORT BYTE [BYTE ...]     e.g. send_cmd.py COM9 0x0A 4 1
"""
import struct
import sys
import time

import serial

port = serial.Serial(sys.argv[1], 115200, timeout=0, write_timeout=1)
data = bytes(int(b, 0) for b in sys.argv[2:])
port.write((struct.pack("<BBBBI", 0x4B, 0x01, 0x40, 0, 0) + data).ljust(64, b"\0"))
time.sleep(0.3)
port.close()
