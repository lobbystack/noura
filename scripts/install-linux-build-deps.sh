#!/usr/bin/env bash
# Installs the system libraries the desktop app links on Linux: ALSA and
# PulseAudio for speech capture, and GTK for the tray icon and menus. Any
# job that builds app-core, speech or desktop needs them. With --runtime it
# also installs the tray library the app loads when it starts.
set -euo pipefail

packages=(pkg-config libasound2-dev libpulse-dev libgtk-3-dev)
if [ "${1:-}" = "--runtime" ]; then
  packages+=(libayatana-appindicator3-1)
fi

sudo apt-get update
sudo apt-get install -y "${packages[@]}"
