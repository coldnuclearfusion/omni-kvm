"""Log a board's UART port to a file and pass it commands: every line
written to CMDFILE is sent to the board, then the file is emptied. Keeps
the port open the whole time (on macOS, opening it resets the board).

    python uart_console.py PORT LOGFILE CMDFILE SECONDS

Log lines carry the wall-clock time (to line up with daemon logs) and
the seconds since start.
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
logfile, cmdfile, seconds = sys.argv[2], sys.argv[3], float(sys.argv[4])
start = time.time()
pending = b""
last_check = 0.0


def stamp():
    return f"{time.strftime('%H:%M:%S')} {time.time() - start:8.2f} "


with open(logfile, "a", encoding="utf-8") as out:
    out.write(f"--- uart_console on {sys.argv[1]} started {time.strftime('%Y-%m-%d %H:%M:%S')}\n")
    out.flush()
    while time.time() - start < seconds:
        pending += port.read(4096)
        while b"\n" in pending:
            line, pending = pending.split(b"\n", 1)
            out.write(f"{stamp()} {line.decode('utf-8', 'replace').rstrip()}\n")
            out.flush()
        if time.time() - last_check > 0.2:
            last_check = time.time()
            try:
                with open(cmdfile, "r+", encoding="utf-8") as f:
                    commands = f.read()
                    f.seek(0)
                    f.truncate()
            except FileNotFoundError:
                commands = ""
            for command in commands.splitlines():
                command = command.strip()
                if command:
                    port.write((command + "\n").encode())
                    out.write(f"{stamp()} >>> {command}\n")
                    out.flush()
port.close()
