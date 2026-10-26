#!/bin/sh
# Installs Quotty for the current user: no root needed.
#   ./install.sh            installs to ~/.local
#   ./install.sh --no-autostart   skips the autostart entry
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin"
autostart=yes
[ "${1:-}" = "--no-autostart" ] && autostart=no

mkdir -p "$bin" "$prefix/applications" "$prefix/icons/hicolor/256x256/apps"
install -m 755 "$here/quotty" "$bin/quotty"
install -m 644 "$here/quotty.png" "$prefix/icons/hicolor/256x256/apps/quotty.png"

cat > "$prefix/applications/quotty.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Quotty
Comment=AI quota strip
Exec="$bin/quotty"
Icon=quotty
Terminal=false
Categories=Utility;
StartupNotify=false
EOF

if [ "$autostart" = yes ]; then
    mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/autostart"
    cp "$prefix/applications/quotty.desktop" "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/Quotty.desktop"
fi

echo "Installed to $bin/quotty"
echo "Make sure $bin is on your PATH, or run it by its full path."
