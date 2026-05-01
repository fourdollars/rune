#!/usr/bin/env bash
# Fetch Taoyuan weather using wttr.in
# Use location: Taoyuan, Taiwan
set -euo pipefail

LOCATION="Taoyuan"
# request terminal-friendly output
curl -sS "https://wttr.in/${LOCATION}?format=3&lang=zh"
