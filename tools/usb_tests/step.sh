#!/bin/bash
# One test step: send the given commands, wait, then show what every log
# got meanwhile (PC daemon, board 1 UART, Mac daemon, board 2 UART, Mac
# input driver). An empty string sends nothing.
#   step.sh SECONDS "MAC_INPUT_CMD" "BOARD2_UART_CMD" "BOARD1_UART_CMD"
S=$(cd "$(dirname "$0")" && pwd)   # logs and command files next to this script
WAIT=$1; MACIN=$2; B2=$3; B1=$4
PCD=$(wc -l < "$S/pc_daemon_t3.log"); PCU=$(wc -l < "$S/uart_pc.log")
read MD MU MI < <(ssh -o ConnectTimeout=15 m4-mba 'echo $(wc -l < /tmp/omni-kvm-mac.log) $(wc -l < /tmp/uart_mac.log) $(cat /tmp/mac_input.log 2>/dev/null | wc -l)')
date "+step at %H:%M:%S"
[ -n "$B1" ] && echo "$B1" > "$S/uart_pc.cmd"
ssh -o ConnectTimeout=15 m4-mba "[ -n \"$MACIN\" ] && echo \"$MACIN\" > /tmp/mac_input.cmd; [ -n \"$B2\" ] && echo \"$B2\" > /tmp/uart_mac.cmd; true"
sleep "$WAIT"
echo "=== PC daemon ==="; tail -n +$((PCD+1)) "$S/pc_daemon_t3.log" | grep -v "^\[link\] UP"
echo "=== board 1 UART ==="; tail -n +$((PCU+1)) "$S/uart_pc.log" | grep -v "\[radio\]\|packets so far"
ssh -o ConnectTimeout=15 m4-mba "echo '=== Mac daemon ==='; tail -n +$((MD+1)) /tmp/omni-kvm-mac.log | grep -v '^\[link\] UP'; echo '=== board 2 UART ==='; tail -n +$((MU+1)) /tmp/uart_mac.log | grep -v '\[radio\]\|packets so far'; echo '=== Mac input ==='; tail -n +$((MI+1)) /tmp/mac_input.log"
