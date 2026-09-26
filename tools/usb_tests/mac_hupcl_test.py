"""Does the board notice when a process that had its serial port open goes
away (macOS)? The board sends LINK_CHANGED (status 0x05) each time a
daemon connects; if it does after close + reopen, it saw DTR drop.

    python mac_hupcl_test.py /dev/cu.usbmodem...

Opens the port raw (no input flush), like a daemon, several times.
"""
import fcntl
import os
import struct
import sys
import termios
import time

PORT = sys.argv[1]
TIOCMGET = 0x4004746A  # _IOR('t', 106, int) on macOS
TIOCM_DTR, TIOCM_RTS = 0x002, 0x004


def open_raw():
    return os.open(PORT, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)


def lines(fd):
    m = struct.unpack("i", fcntl.ioctl(fd, TIOCMGET, struct.pack("i", 0)))[0]
    return f"DTR={int(bool(m & TIOCM_DTR))} RTS={int(bool(m & TIOCM_RTS))}"


def hupcl(fd):
    return bool(termios.tcgetattr(fd)[2] & termios.HUPCL)


def set_hupcl(fd, on):
    attrs = termios.tcgetattr(fd)
    attrs[2] = (attrs[2] | termios.HUPCL) if on else (attrs[2] & ~termios.HUPCL)
    termios.tcsetattr(fd, termios.TCSANOW, attrs)


def link_changed_within(fd, seconds):
    buf, end = b"", time.time() + seconds
    while time.time() < end:
        try:
            buf += os.read(fd, 4096)
        except BlockingIOError:
            pass
        time.sleep(0.01)
    count, i = 0, buf.find(b"\x4b\x01\x41")
    while i >= 0:
        if len(buf) >= i + 10 and buf[i + 8] == 0x05:
            count += 1
        i = buf.find(b"\x4b\x01\x41", i + 1)
    return count


def step(name, before_close=None):
    fd = open_raw()
    info = f"HUPCL={int(hupcl(fd))} {lines(fd)}"
    n = link_changed_within(fd, 1.5)
    if before_close:
        before_close(fd)
    os.close(fd)
    print(f"{name:52s} {info:22s} LINK_CHANGED x{n}")


step("1. first open")
step("2. reopen right after close (default settings)")
step("3. reopen; then turn HUPCL off before closing", lambda fd: set_hupcl(fd, False))
step("4. reopen after a close with HUPCL off")
step("5. reopen; turn HUPCL back on before closing", lambda fd: set_hupcl(fd, True))
step("6. reopen after a close with HUPCL on")
