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
wmctrl -l > /out/windows-after.txt 2>&1

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(ls /out/*.png | wc -l) screenshots"
