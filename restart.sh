#!/bin/bash
pkill -f "rune serve" 2>/dev/null
sleep 2
cd /home/u/rune
setsid ./target/debug/rune serve >> /tmp/rune-serve.log 2>&1 < /dev/null &
sleep 2
pgrep -a rune
ss -tlnp | grep 9527
