#!/usr/bin/env bash
# Fetch Taoyuan weather from Open-Meteo (no API key)
# Coordinates for Taoyuan City, Taiwan: 24.993628, 121.296968

set -euo pipefail

LAT=24.993628
LON=121.296968

# Request current weather (temperature, windspeed, weathercode) from Open-Meteo
URL="https://api.open-meteo.com/v1/forecast?latitude=${LAT}&longitude=${LON}&current_weather=true&timezone=Asia%2FTaipei"

curl -sS "$URL" | jq '.'
