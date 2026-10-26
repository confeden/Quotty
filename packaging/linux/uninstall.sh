#!/bin/sh
# Removes what install.sh put in place. Settings in ~/.config/Quotty stay.
set -eu

prefix="${XDG_DATA_HOME:-$HOME/.local/share}"
config="${XDG_CONFIG_HOME:-$HOME/.config}"

pkill -x quotty 2>/dev/null || true
rm -f "$HOME/.local/bin/quotty" \
      "$prefix/applications/quotty.desktop" \
      "$prefix/icons/hicolor/256x256/apps/quotty.png" \
      "$config/autostart/Quotty.desktop"
echo "Quotty removed. Settings kept in $config/Quotty."
