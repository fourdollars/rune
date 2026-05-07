#!/bin/sh
# Docker entrypoint: drops privileges to rune user (uid 1000) for normal usage.
# Concourse invokes /opt/resource/{check,in,out} directly (hard links to rune binary),
# bypassing this entrypoint entirely, and runs as root for volume write access.
if [ "$(id -u)" = "0" ]; then
    # Running as root in docker run — drop to rune user
    # Use su since it's available on all distros (Alpine, Debian, Ubuntu)
    exec su -s /bin/sh rune -c 'exec /usr/local/bin/rune "$@"' -- rune "$@"
else
    # Already non-root (e.g. --user flag)
    exec /usr/local/bin/rune "$@"
fi
