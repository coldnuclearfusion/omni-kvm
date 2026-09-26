#!/bin/zsh
# Runs the Omni-KVM daemon in a loop (restarted whenever it exits), logging to /tmp/omni-kvm-mac.log.
cd ~/projects/omni-kvm/daemon && while true; do ./target/release/omni-kvm 2>&1 | tee -a /tmp/omni-kvm-mac.log; sleep 1; done
