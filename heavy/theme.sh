#!/bin/sh
# FerroOS Heavy: the Windows 95 look. Fetches Chicago95 (a fixed release,
# checked before use) and makes it the default for the logon screen and for
# every account's desktop. Run by postinstall.sh, and by `ferro-heavy theme`
# if the download failed during installation.

HERE=/usr/local/share/ferro-heavy
C95_URL=https://github.com/grassmunk/Chicago95/archive/refs/tags/v3.0.1.tar.gz
C95_SHA256=ddf0e35c00899ce0cea752db09f71d1e28ec3caf15e0653838c0d7b3bc5e043a

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

curl -fsSL --proto '=https' -o "$T/c95.tar.gz" "$C95_URL" || { echo "download failed"; exit 1; }
echo "$C95_SHA256  $T/c95.tar.gz" | sha256sum -c - >/dev/null || { echo "checksum mismatch"; exit 1; }
tar -xzf "$T/c95.tar.gz" -C "$T" || { echo "unpack failed"; exit 1; }
SRC="$T/Chicago95-3.0.1"

mkdir -p /usr/share/themes /usr/share/icons /usr/share/fonts/truetype/chicago95
cp -r "$SRC/Theme/Chicago95" /usr/share/themes/ || exit 1
cp -r "$SRC/Icons/Chicago95" "$SRC/Icons/Chicago95-tux" /usr/share/icons/ || exit 1
cp -r "$SRC/Cursors/Chicago95_Cursor_Black" "$SRC/Cursors/Chicago95_Cursor_White" /usr/share/icons/ || exit 1
cp -r "$SRC/Fonts/vga_font" /usr/share/fonts/truetype/chicago95/ 2>/dev/null
fc-cache -f >/dev/null 2>&1

# What every new account's desktop starts with.
mkdir -p /etc/xdg/xfce4/xfconf/xfce-perchannel-xml
cp "$HERE/xfce/"*.xml /etc/xdg/xfce4/xfconf/xfce-perchannel-xml/ || exit 1

# The logon screen.
mkdir -p /etc/lightdm/lightdm-gtk-greeter.conf.d
cat >/etc/lightdm/lightdm-gtk-greeter.conf.d/60-ferro-heavy.conf <<'EOF'
[greeter]
theme-name = Chicago95
icon-theme-name = Chicago95
cursor-theme-name = Chicago95_Cursor_Black
background = #008080
EOF
echo "Chicago95 installed"
