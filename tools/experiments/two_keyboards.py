"""Experiment A2 (docs/requirements.md): two keyboards typing into one
computer. Does Hangul compose when its letters come from two keyboards,
and does a modifier held on one keyboard apply to keys on the other?

Run it on one computer; each time you press Enter here, this computer's
board types a key into the OTHER computer (through the radio), where you
also type on that computer's own keyboard.

    python two_keyboards.py            (finds the board by itself)
    python two_keyboards.py --port COM9

Needs pyserial. The board must be on its "USB" port, and both boards
linked. No Omni-KVM daemon may be using the port.
"""
import argparse
import struct
import time

import serial
from serial.tools import list_ports

MAGIC, VERSION = 0x4B, 0x01
KEY_DOWN, KEY_UP, MODIFIER_SYNC = 0x03, 0x04, 0x05
USAGE = {"r": 0x15, "k": 0x0E}  # 2-set Korean: r = ㄱ, k = ㅏ
LEFT_SHIFT = 0x02


def packet(msg_type, payload, seq):
    return (struct.pack("<BBBBI", MAGIC, VERSION, msg_type, 0, seq) + payload).ljust(64, b"\0")


class Board:
    def __init__(self, port_name):
        self.port = serial.Serial()
        self.port.port, self.port.baudrate, self.port.timeout = port_name, 115200, 0.1
        self.port.dtr, self.port.rts = True, False
        self.port.open()
        self.seq = 0

    def send(self, msg_type, payload):
        self.seq += 1
        self.port.write(packet(msg_type, payload, self.seq))

    def tap(self, letter):
        self.send(KEY_DOWN, bytes([USAGE[letter], 0]))
        time.sleep(0.05)
        self.send(KEY_UP, bytes([USAGE[letter], 0]))

    def hold_shift(self, seconds):
        self.send(MODIFIER_SYNC, bytes([LEFT_SHIFT]))
        time.sleep(seconds)
        self.send(MODIFIER_SYNC, bytes([0]))


def find_board():
    ports = [p.device for p in list_ports.comports() if p.vid == 0x303A and not p.device.startswith("/dev/tty.")]
    if len(ports) != 1:
        raise SystemExit(f"Found boards on {ports or 'no port'}; pass --port")
    return ports[0]


def step(text):
    input(f"\n{text}\n  → 준비되면 이 창에서 Enter ")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port")
    board = Board(parser.parse_args().port or find_board())

    print("실험 A2: 두 키보드로 한 컴퓨터에 입력하기")
    print("여기서 Enter를 누를 때마다, 이 컴퓨터의 보드가 '상대 컴퓨터'에 키를 입력해요.")

    step("1) 상대 컴퓨터에서 메모장(맥은 텍스트 편집기)을 열고, 한글 입력 상태로 두세요.\n"
         "   Enter를 누르면 상대 컴퓨터에 'ㄱ'이 입력돼요.\n"
         "   그다음 상대 컴퓨터의 자기 키보드로 'ㅏ'(k)를 치세요. '가'가 되는지 보세요.")
    board.tap("r")

    step("2) 이번엔 반대로: 상대 컴퓨터의 자기 키보드로 'ㄱ'(r)만 쳐 두세요.\n"
         "   Enter를 누르면 제가 'ㅏ'를 보내요. '가'가 되는지 보세요.")
    board.tap("k")

    step("3) Enter를 누르면 보드가 10초 동안 Shift를 누르고 있어요.\n"
         "   그 10초 안에 상대 컴퓨터의 자기 키보드로 'r'을 치세요. 'ㄲ'이 나오는지 보세요.")
    print("   (Shift 누르는 중... 10초)")
    board.hold_shift(10)

    print("\n끝났어요. 1), 2), 3)에서 각각 무엇이 입력됐는지 알려 주세요.")


if __name__ == "__main__":
    main()
