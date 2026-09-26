"""Record a board's UART log without resetting it.

    python uart_log.py PORT SECONDS OUTFILE

Opens the port with DTR and RTS off (the board keeps running) and writes
each line with the time since start.
"""
import sys
import time

import serial

port = serial.Serial()
port.port = sys.argv[1]
port.baudrate = 115200
port.timeout = 0.05
port.dtr = False
port.rts = False
port.open()
start = time.time()
end = start + float(sys.argv[2])
with open(sys.argv[3], "w", encoding="utf-8") as out:
    pending = b""
    while time.time() < end:
        pending += port.read(4096)
        while b"\n" in pending:
            line, pending = pending.split(b"\n", 1)
            out.write(f"{time.time() - start:7.2f}  {line.decode('utf-8', 'replace').rstrip()}\n")
            out.flush()
port.close()
