#!/bin/sh
# Adds TeddyBench to the application menu of the current user, pointing to the teddybench
# executable next to this script. Run it again after moving the folder.
#   ./install-desktop-entry.sh              install
#   ./install-desktop-entry.sh --uninstall  remove
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
entry="$data/applications/teddybench.desktop"
icon="$data/icons/hicolor/512x512/apps/teddybench.png"

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$entry" "$icon"
    echo "Removed the TeddyBench menu entry."
else
    if [ ! -x "$here/teddybench" ]; then
        echo "teddybench not found next to this script" >&2
        exit 1
    fi
    mkdir -p "$(dirname "$entry")" "$(dirname "$icon")"
    cp "$here/teddybench.png" "$icon"
    # quote the path for the Exec key, it may contain spaces
    exec_path="$(printf '%s' "$here/teddybench" | sed 's/\\/\\\\\\\\/g; s/"/\\\\"/g')"
    sed -e "s|^Exec=.*|Exec=\"$exec_path\" %F|" "$here/teddybench.desktop" > "$entry"
    chmod 644 "$entry"
    echo "Installed $entry"
fi

# refresh the menus where the tools are available
command -v update-desktop-database > /dev/null && update-desktop-database "$data/applications" 2> /dev/null || true
command -v gtk-update-icon-cache > /dev/null && gtk-update-icon-cache -q "$data/icons/hicolor" 2> /dev/null || true
