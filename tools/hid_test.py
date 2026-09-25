"""Send Omni-KVM protocol packets to a board over its USB serial port.

Stands in for the host daemon. The board forwards these packets to its
peer over the radio, and the peer types / moves the mouse on its own
host. With both boards plugged into one PC, pass --port for the board
that should send; the other one does the typing.

Usage (needs pyserial; PlatformIO's Python already has it):
    python tools/hid_test.py type "Hello from Omni-KVM!"
    python tools/hid_test.py move 200 -100
    python tools/hid_test.py click right
    python tools/hid_test.py scroll -3

Typed text goes to whichever window has focus. On Windows, add
--wait-for-notepad to hold the packets until Notepad is in front.
"""

import argparse
import struct
import sys
import time

import serial
from serial.tools import list_ports

# Protocol constants — see shared/protocol.md
MAGIC = 0x4B
VERSION = 0x01
PACKET_SIZE = 64
MSG_MOUSE_MOVE = 0x01
MSG_MOUSE_SCROLL = 0x02
MSG_KEY_DOWN = 0x03
MSG_KEY_UP = 0x04

MOD_LEFT_SHIFT = 0x02
MOUSE_BUTTONS = {"left": 0x01, "right": 0x02, "middle": 0x04}
ESPRESSIF_VID = 0x303A


def build_keymap():
    """ASCII character -> (HID usage ID, needs Shift), US keyboard layout."""
    keymap = {}
    for i, c in enumerate("abcdefghijklmnopqrstuvwxyz"):
        keymap[c] = (0x04 + i, False)
        keymap[c.upper()] = (0x04 + i, True)
    for i, (plain, shifted) in enumerate(zip("1234567890", "!@#$%^&*()")):
        keymap[plain] = (0x1E + i, False)
        keymap[shifted] = (0x1E + i, True)
    for code, plain, shifted in [
        (0x28, "\n", None), (0x2B, "\t", None), (0x2C, " ", None),
        (0x2D, "-", "_"), (0x2E, "=", "+"), (0x2F, "[", "{"), (0x30, "]", "}"),
        (0x31, "\\", "|"), (0x33, ";", ":"), (0x34, "'", '"'), (0x35, "`", "~"),
        (0x36, ",", "<"), (0x37, ".", ">"), (0x38, "/", "?"),
    ]:
        keymap[plain] = (code, False)
        if shifted:
            keymap[shifted] = (code, True)
    return keymap


KEYMAP = build_keymap()


class Link:
    def __init__(self, port):
        self.serial = serial.Serial(port, timeout=1)
        self.seq = 0

    def send(self, msg_type, payload=b""):
        self.seq += 1
        # "<" = little-endian, as the protocol specifies.
        header = struct.pack("<BBBBI", MAGIC, VERSION, msg_type, 0, self.seq)
        self.serial.write((header + payload).ljust(PACKET_SIZE, b"\x00"))

    def close(self):
        self.serial.flush()
        self.serial.close()


def find_board_port():
    ports = [p for p in list_ports.comports() if p.vid == ESPRESSIF_VID]
    if len(ports) != 1:
        found = ", ".join(f"{p.device} (serial {p.serial_number})" for p in ports) or "none"
        sys.exit(f"Expected one Omni-KVM USB port, found: {found}. Use --port.")
    return ports[0].device


def wait_for_notepad(timeout_s=300):
    """Block until the foreground window is Notepad (Windows only)."""
    import ctypes

    user32 = ctypes.windll.user32
    name = ctypes.create_unicode_buffer(256)
    print("Waiting for Notepad to be the active window...")
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        user32.GetClassNameW(user32.GetForegroundWindow(), name, 256)
        if name.value == "Notepad":
            time.sleep(0.3)   # let the click that focused Notepad finish
            return
        time.sleep(0.1)
    sys.exit("Timed out waiting for Notepad.")


def type_text(link, text):
    # Sent as one burst on purpose: pacing is the firmware's job.
    for ch in text:
        if ch not in KEYMAP:
            print(f"Skipping unsupported character {ch!r}")
            continue
        code, shift = KEYMAP[ch]
        link.send(MSG_KEY_DOWN, struct.pack("<BB", code, MOD_LEFT_SHIFT if shift else 0))
        link.send(MSG_KEY_UP, struct.pack("<BB", code, 0))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", help="board's USB serial port (default: auto-detect)")
    parser.add_argument("--wait-for-notepad", action="store_true",
                        help="hold packets until Notepad is the active window (Windows)")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("type").add_argument("text")
    move = sub.add_parser("move")
    move.add_argument("dx", type=int)
    move.add_argument("dy", type=int)
    sub.add_parser("click").add_argument("button", nargs="?", default="left",
                                         choices=MOUSE_BUTTONS)
    sub.add_parser("scroll").add_argument("amount", type=int, help="positive = up")
    args = parser.parse_args()

    link = Link(args.port or find_board_port())
    if args.wait_for_notepad:
        wait_for_notepad()

    if args.command == "type":
        type_text(link, args.text)
    elif args.command == "move":
        link.send(MSG_MOUSE_MOVE, struct.pack("<hhB", args.dx, args.dy, 0))
    elif args.command == "click":
        button = MOUSE_BUTTONS[args.button]
        link.send(MSG_MOUSE_MOVE, struct.pack("<hhB", 0, 0, button))
        link.send(MSG_MOUSE_MOVE, struct.pack("<hhB", 0, 0, 0))
    elif args.command == "scroll":
        link.send(MSG_MOUSE_SCROLL, struct.pack("<hh", args.amount, 0))

    link.close()
    print(f"Sent {link.seq} packets.")


if __name__ == "__main__":
    main()
