"""Unattended soak: switch the focus with the Mac hotkey (synthetic,
through the Mac input driver) twice per cycle, and every FAULT_EVERY
cycles first cause a USB event on a board (reconnect or imitated driver
reset, taking turns) and wait FAULT_WAIT seconds. Each switch must reach
both daemons with the same view and follow the hotkey rule (focus on the
PC -> Mac; Mac or split -> PC).

    python soak.py MINUTES [CYCLE_SECONDS=60] [FAULT_EVERY=10] [FAULT_WAIT=15]

Writes soak.log next to this script, and a summary at the end.
"""
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PC_LOG = os.path.join(HERE, "pc_daemon_t3.log")
PC_UART_CMD = os.path.join(HERE, "uart_pc.cmd")
OUT = os.path.join(HERE, "soak.log")
MINUTES = float(sys.argv[1])
CYCLE = float(sys.argv[2]) if len(sys.argv) > 2 else 60
FAULT_EVERY = int(sys.argv[3]) if len(sys.argv) > 3 else 10
FAULT_WAIT = float(sys.argv[4]) if len(sys.argv) > 4 else 15
FOCUS = re.compile(r"\[focus\] (split mode|focus on the PC|focus on the Mac) \(view (\d+) by the (PC|Mac)\)")
FAULTS = [("board 2", "usb reconnect"), ("board 2", "usb k6"), ("board 1", "usb reconnect"), ("board 1", "usb k6")]


def log(text):
    line = f"{time.strftime('%H:%M:%S')} {text}"
    print(line, flush=True)
    with open(OUT, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def ssh(command, tries=3):
    for _ in range(tries):
        r = subprocess.run(["ssh", "-o", "ConnectTimeout=15", "m4-mba", command], capture_output=True, text=True)
        if r.returncode == 0:
            return r.stdout
        time.sleep(3)
    raise RuntimeError(f"ssh failed: {r.stderr.strip()}")


def pc_lines():
    with open(PC_LOG, encoding="utf-8", errors="replace") as f:
        return f.read().splitlines()


def mac_lines():
    return ssh("cat /tmp/omni-kvm-mac.log").splitlines()


def last_focus(lines):
    for line in reversed(lines):
        m = FOCUS.search(line)
        if m:
            return m.group(1), int(m.group(2))
    return None, None


def switch(expected_views_after):
    """Press the hotkey; return (ok, text)."""
    pc_before, mac_before = len(pc_lines()), len(mac_lines())
    mode_before, view_before = last_focus(pc_lines())
    want = "focus on the Mac" if mode_before == "focus on the PC" else "focus on the PC"
    ssh("echo hotkey > /tmp/mac_input.cmd")
    deadline = time.time() + 5
    while time.time() < deadline:
        time.sleep(0.5)
        pc_new, mac_new = pc_lines()[pc_before:], mac_lines()[mac_before:]
        pc_mode, pc_view = last_focus(pc_new)
        mac_mode, mac_view = last_focus(mac_new)
        if pc_mode and mac_mode and pc_view == mac_view and pc_mode == mac_mode:
            ok = pc_mode == want and pc_view > (view_before or 0)
            return ok, f"{mode_before} (view {view_before}) -> {pc_mode} (view {pc_view})" + ("" if ok else f"; expected {want}")
    return False, f"{mode_before} (view {view_before}) -> no matching change on both daemons within 5 s " \
                  f"(PC: {last_focus(pc_lines()[pc_before:])}, Mac: {last_focus(mac_lines()[mac_before:])})"


def fault(board, command):
    if board == "board 1":
        with open(PC_UART_CMD, "w", encoding="utf-8") as f:
            f.write(command + "\n")
    else:
        ssh(f"echo '{command}' > /tmp/uart_mac.cmd")


def count_events():
    pc = open(os.path.join(HERE, "uart_pc.log"), encoding="utf-8", errors="replace").read()
    mac = ssh("cat /tmp/uart_mac.log")
    keys = {"Unknown Condition": "driver resets (K6)", "stuck (": "watchdog reconnects",
            "left over from before": "forgotten transfers stopped", "Complete but not empty": "old data sent",
            "still in use": "layout fix gave up", "did not confirm": "late endpoint stops"}
    return {name: (pc.count(k), mac.count(k)) for k, name in keys.items()}


start = time.time()
log(f"soak started for {MINUTES} minutes, a cycle every {CYCLE} s, a USB event every {FAULT_EVERY} cycles; "
    f"events so far (board 1, board 2): {count_events()}")
switches = passed = faults = 0
failures = []
cycle = 0
while time.time() - start < MINUTES * 60:
    cycle += 1
    cycle_start = time.time()
    if cycle % FAULT_EVERY == 0:
        board, command = FAULTS[faults % len(FAULTS)]
        faults += 1
        log(f"cycle {cycle}: {board}: {command}")
        try:
            fault(board, command)
        except RuntimeError as e:
            log(f"  could not send: {e}")
        time.sleep(FAULT_WAIT)
    for _ in range(2):
        try:
            ok, text = switch(None)
        except RuntimeError as e:
            ok, text = False, f"ssh error: {e}"
        switches += 1
        passed += ok
        log(f"cycle {cycle}: {'PASS' if ok else 'FAIL'} {text}")
        if not ok:
            failures.append(f"cycle {cycle}: {text}")
            time.sleep(10)
        time.sleep(1)
    time.sleep(max(0, CYCLE - (time.time() - cycle_start)))
log(f"soak done: {passed} of {switches} switches passed, {faults} USB events")
for f in failures:
    log(f"  failed: {f}")
log(f"events (board 1, board 2): {count_events()}")
