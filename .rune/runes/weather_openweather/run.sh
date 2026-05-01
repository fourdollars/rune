#!/usr/bin/env bash
# Fetch Taoyuan weather from OpenWeatherMap (requires OPENWEATHER_API_KEY env var)
# Coordinates for Taoyuan City, Taiwan: 24.993628, 121.296968

set -euo pipefail

if [ -z "${OPENWEATHER_API_KEY:-}" ]; then
  echo "OPENWEATHER_API_KEY environment variable not set" >&2
  exit 1
fi

LAT=24.993628
LON=121.296968

URL="https://api.openweathermap.org/data/2.5/weather?lat=${LAT}&lon=${LON}&appid=${OPENWEATHER_API_KEY}&units=metric&lang=zh_tw"

curl -sS "$URL" | jq '.'
