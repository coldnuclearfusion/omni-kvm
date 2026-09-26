# USB tests

Tools used on 2026-09-26 to find and verify the USB defects and their
workarounds (docs/platform.md, K1–K7) and to run acceptance tests with
nobody at the computers (docs/acceptance-tests.md). They talk to the
boards directly, so stop the daemon first unless a tool says otherwise.
They need Python 3 with pyserial (on the Mac: `~/.venvs/esptool/bin/python`).

## One board, through its USB serial port

| Tool | What it does |
|---|---|
| `send_cmd.py PORT BYTE...` | Sends one `MSG_DAEMON_CMD`, e.g. `0x0A 4 1` (log the FIFO layout). |
| `reconnect_test.py PORT [RUNS]` | Asks the board to reconnect its USB (setting 8) and times: port gone, port back, first answer. |
| `stall_test.py PORT [SECONDS]` | Switches the filler on (a status packet every 2 ms) and stops reading, to see whether the watchdog notices a stuck serial endpoint. |
| `leftover_test.py PORT RUNS` | With the filler on, imitates the driver's own reset (setting 9), reopens the port and checks that packets keep coming. |
| `open_cycles_mac.py PORT N` | Opens and closes the port N times like the daemon, asking for stats each time; counts failed openings (K6). |
| `usb_stress.py PORT SECONDS PREVENT REPAIR STATS_PER_S` | Loads the serial link both ways (experiment A5). `REPAIR` is ignored by current firmware. |

## A board's "UART" port

| Tool | What it does |
|---|---|
| `uart_log.py PORT SECONDS OUTFILE` | Records the log. On macOS opening the port resets the board. |
| `uart_console.py PORT LOGFILE CMDFILE SECONDS` | Records the log and sends every line written to CMDFILE to the board: `usb reconnect`, `usb k6`, `usb layout`, `usb report` (firmware `main.cpp`, "commands on the UART port"). Works with the daemon running. |

## Both computers, nobody at them

Used for the remote acceptance tests: both daemons running, each board's
UART port on its computer, the Mac reachable over SSH as `m4-mba`.

| Tool | What it does |
|---|---|
| `run-daemon.command` | Mac: runs the daemon in a loop, logging to `/tmp/omni-kvm-mac.log`. Start it with `open run-daemon.command` so that it runs in Terminal, which holds the Accessibility permission the daemon needs. |
| `input_driver.swift`, `run-input-driver.command` | Mac: posts HID-level keyboard and pointer events on request (`hotkey` = Command+Esc, `push-left N`, `push-right N`, `where`), read from `/tmp/mac_input.cmd`. Never clicks or types characters. Build with `swiftc -O input_driver.swift -o input_driver`; start with `open run-input-driver.command` (Terminal's permission lets it post). The Windows daemon ignores injected input by design, so there is no PC counterpart. |
| `step.sh SECONDS MAC_INPUT BOARD2_UART BOARD1_UART` | One test step: sends the commands, waits, prints what every log got meanwhile. Expects the PC daemon's log in `pc_daemon_t3.log` and board 1's UART console files (`uart_pc.log`, `uart_pc.cmd`) next to it. |
| `soak.py MINUTES [CYCLE_SECONDS] [FAULT_EVERY] [FAULT_WAIT]` | Switches the focus with the Mac hotkey twice per cycle and causes a USB event on a board every FAULT_EVERY cycles; checks each switch reached both daemons with the same view and followed the hotkey rule. Writes `soak.log`. |
