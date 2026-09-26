#!/bin/zsh
# Test input driver (posts HID-level events on request; see input_driver.swift).
# Runs the input_driver built next to this file.
exec "$(cd "$(dirname "$0")" && pwd)/input_driver"
