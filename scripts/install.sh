#!/bin/sh
# Install a Linux release archive into the current user's ~/.local tree.
set -eu

archive_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
bin_dir=${HOME:?HOME must be set}/.local/bin
lib_dir=${HOME:?HOME must be set}/.local/lib/goatty
data_home=${XDG_DATA_HOME:-"$HOME/.local/share"}

case $data_home in
    /*) ;;
    *) echo "XDG_DATA_HOME must be an absolute path" >&2; exit 1 ;;
esac

for required in goatty goattyd goatty.svg; do
    if [ ! -f "$archive_dir/$required" ]; then
        echo "missing $required; run this from an extracted goatty Linux release" >&2
        exit 1
    fi
done

mkdir -p "$bin_dir" "$lib_dir/remote" \
    "$data_home/icons/hicolor/scalable/apps" "$data_home/applications"
install -m 755 "$archive_dir/goatty" "$bin_dir/goatty"
install -m 755 "$archive_dir/goattyd" "$bin_dir/goattyd"
install -m 644 "$archive_dir/goatty.svg" "$data_home/icons/hicolor/scalable/apps/goatty.svg"

if [ -d "$archive_dir/remote" ]; then
    for binary in "$archive_dir"/remote/goattyd-*; do
        [ -f "$binary" ] || continue
        install -m 755 "$binary" "$lib_dir/remote/$(basename "$binary")"
    done
fi

# Desktop Entry quoting: quote the executable path and escape characters that
# have special meaning inside a quoted Exec argument.
desktop_exec=$(printf '%s' "$bin_dir/goatty" | sed 's/\\/\\\\/g; s/"/\\"/g; s/`/\\`/g; s/\$/\\$/g')
cat > "$data_home/applications/goatty.desktop" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=Goatty
Comment=A GPU-rendered terminal with persistent sessions
Exec="$desktop_exec"
Icon=goatty
Terminal=false
Categories=System;TerminalEmulator;
StartupWMClass=dev.goatty.goatty
EOF

echo "Installed Goatty. The launcher is at $data_home/applications/goatty.desktop"
if ! printf '%s' ":${PATH:-}:" | grep -Fq ":$bin_dir:"; then
    echo "Add $bin_dir to PATH to launch goatty from a terminal."
fi
