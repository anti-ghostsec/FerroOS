#!/bin/sh
# The window-manager test VM's job, run by `cargo xtask wm-test` inside the
# minimal Debian that tools/vm/init.sh installs. Starts a virtual screen,
# ferro-wm and two ordinary programs, works the mouse like a person would
# (drag a title bar, press Close, go full screen), and takes a screenshot
# after each step. The pictures and logs go onto /dev/vda as a tar stream.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /out

echo "== installing a virtual screen and test programs"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    xvfb xterm x11-apps xdotool wmctrl netpbm >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

shot() {
    xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng > "/out/$1.png" || echo "screenshot $1 failed"
}

cp /in/ferro-wm /tmp/ferro-wm && chmod +x /tmp/ferro-wm || fail "copy ferro-wm"
Xvfb :1 -screen 0 1024x768x24 >/out/xvfb.log 2>&1 &
sleep 3
export DISPLAY=:1
/tmp/ferro-wm >/out/ferro-wm.log 2>&1 &
sleep 2

echo "== two programs"
xterm -geometry 64x12 -T "Command Prompt" -e sh -c 'echo "An ordinary Linux program in a FerroOS frame."; sleep 900' &
sleep 3
xclock -geometry 180x180 &
sleep 5
shot 1-two-windows

echo "== drag the clock by its title bar"
# The second window opens at 56,56; its title bar is a few pixels in.
xdotool mousemove 140 68 mousedown 1 mousemove 400 250 mousemove 600 400 mouseup 1
sleep 2
shot 2-clock-moved

echo "== click the terminal: it comes forward"
xdotool mousemove 200 150 click 1
sleep 2
shot 3-terminal-in-front

echo "== full screen, as a game would ask"
wmctrl -r :ACTIVE: -b add,fullscreen
sleep 2
shot 4-fullscreen
wmctrl -r :ACTIVE: -b remove,fullscreen
sleep 2

echo "== close the clock with its Close button"
wmctrl -lG > /out/windows.txt 2>&1
# The clock's frame was dragged so its title bar's grab point is at 600,400:
# the frame starts 84,12 before that and is 188 wide; Close is 14 in from its right.
xdotool mousemove 690 398 click 1
sleep 3
shot 5-clock-closed

echo "== the terminal's taskbar button puts it away, then brings it back"
# The taskbar is the bottom 28 pixels; the first window's button starts at 62.
xdotool mousemove 120 754 click 1
sleep 2
shot 6-minimized
xdotool mousemove 120 754 click 1
sleep 2

echo "== maximize with the title bar button"
# The terminal's frame starts at 32,32; Maximize is the middle caption button.
W=$(wmctrl -lG | awk '/Command Prompt/ { print $5 }')
xdotool mousemove $((32 + W + 8 - 4 - 2 - 16 - 2 - 8)) 45 click 1
sleep 2
shot 7-maximized

echo "== Start menu: open it, point at an entry, start a program from it"
xdotool mousemove 28 754 click 1
sleep 2
xdotool mousemove 100 600
sleep 1
shot 8-start-menu
ls /usr/share/applications > /out/applications.txt 2>&1
# A click anywhere else closes it.
xdotool mousemove 600 300 click 1
sleep 1
cat > /usr/share/applications/zz-test-clock.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Zz Test Clock
Exec=xclock -geometry 160x160 %U
EOF
xdotool mousemove 28 754 click 1
sleep 2
shot 9-start-menu-with-test-clock
N=$(ls /usr/share/applications/*.desktop | wc -l)
echo "$N entries" >> /out/applications.txt
wmctrl -l > /out/windows-before-launch.txt 2>&1
# "Zz Test Clock" sorts last among programs: one row above the separator,
# which sits above Restart and Shut Down (20 pixels each, 8 for the line).
xdotool mousemove 100 $((740 - 3 - 20 - 20 - 8 - 10)) click 1
sleep 4
shot 10-launched-from-start
wmctrl -l > /out/windows-after.txt 2>&1

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(ls /out/*.png | wc -l) screenshots"
