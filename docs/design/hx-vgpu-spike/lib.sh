#!/bin/bash
# helpers run inside the container
G="ssh -q -i /w/guestkey -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -p 22722 root@127.0.0.1"
waitssh() { for i in $(seq 1 ${1:-90}); do $G true 2>/dev/null && return 0; sleep 2; done; return 1; }
