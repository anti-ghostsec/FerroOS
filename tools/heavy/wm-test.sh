#!/bin/sh
# The window-manager test VM's job, run by `cargo xtask wm-test` inside the
# minimal Debian that tools/vm/init.sh installs. Starts a virtual screen,
# ferro-wm and two ordinary programs, works the mouse like a person would
# (drag a title bar, press Close, go full screen, go from desktop to
# desktop), and takes a screenshot after each step. The pictures and logs go onto /dev/vda as a tar stream.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /out

echo "== installing a virtual screen and test programs"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    xvfb xterm x11-apps x11-utils xauth xdotool wmctrl netpbm psmisc >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}
apt-get clean
rm -rf /var/lib/apt/lists/* /tmp/apt.log /var/cache/* 2>/dev/null || true
sync

shot() {
    xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng > "/out/$1.png" || echo "screenshot $1 failed"
}

cp /in/ferro-wm /in/ferro-files /in/ferro-taskmgr /in/ferro-appearance /tmp/ && chmod +x /tmp/ferro-wm /tmp/ferro-files /tmp/ferro-taskmgr /tmp/ferro-appearance || fail "copy our programs"
mkdir -p /usr/local/bin /usr/bin
cp /tmp/ferro-appearance /tmp/ferro-taskmgr /tmp/ferro-files /tmp/ferro-wm /usr/local/bin/ 2>/dev/null || true
cp /tmp/ferro-appearance /tmp/ferro-taskmgr /tmp/ferro-files /tmp/ferro-wm /usr/bin/ 2>/dev/null || true
Xvfb :1 -screen 0 1024x768x24 >/out/xvfb.log 2>&1 &
sleep 3
export DISPLAY=:1
/tmp/ferro-wm >/out/ferro-wm.log 2>&1 &
sleep 2

echo "== two programs"
xterm -geometry 64x12 -T "Command Prompt" -bg "#000000" -fg "#33FF33" -cr "#33FF33" -fa "DejaVu Sans Mono" -fs 10 -e sh -c 'echo "FerroOS Command Prompt (C:\> ferro-cmd)"; sleep 900' &
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

echo "== Start menu: open it, point at Programs, see flyout submenu"
xdotool mousemove 28 754 click 1
sleep 2
xdotool mousemove 100 474
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
xdotool mousemove 100 474
sleep 1
shot 9-start-menu-with-test-clock
N=$(ls /usr/share/applications/*.desktop | wc -l)
echo "$N entries" >> /out/applications.txt
wmctrl -l > /out/windows-before-launch.txt 2>&1
# "Zz Test Clock" sits in the Programs flyout at (268, 545).
xdotool mousemove 268 545 click 1
sleep 4
shot 10-launched-from-start
wmctrl -l > /out/windows-after.txt 2>&1

echo "== the file manager: a folder, then into one of its folders"
mkdir -p /home/ferro/Documents /home/ferro/Pictures
echo hello > /home/ferro/notes.txt
echo hi > /home/ferro/Documents/letter.txt
/tmp/ferro-files /home/ferro >/out/ferro-files.log 2>&1 &
sleep 3
shot 11-file-manager
# Double-click its first icon (Documents): 30,50 inside its window frame (4 left, 20 top).
set -- $(wmctrl -lG | awk '/ferro/ { print $3, $4 }')
xdotool mousemove $(($1 + 4 + 30)) $(($2 + 20 + 50)) click --repeat 2 --delay 120 1
sleep 2
shot 12-file-manager-in-documents
wmctrl -l > /out/windows-file-manager.txt 2>&1

echo "== right click in file manager to show its context menu"
xdotool mousemove $(($1 + 4 + 180)) $(($2 + 20 + 180)) click 3
sleep 2
shot 13-files-context-menu

echo "== close all windows to show clean desktop, and open desktop context menu"
killall -9 ferro-files xterm xclock 2>/dev/null || true
pkill -9 -f ferro-files 2>/dev/null || true
pkill -9 -f xterm 2>/dev/null || true
pkill -9 -f xclock 2>/dev/null || true
sleep 3
xdotool mousemove 400 350 click 3
sleep 2
shot 14-desktop-context-menu

echo "== dismiss menu, then click desktop icon (Files) to select it"
xdotool mousemove 600 300 click 1
sleep 1
xdotool mousemove 50 45 click 1
sleep 2
shot 15-desktop-files-selected

echo "== click taskbar tray to launch Task Manager"
xdotool mousemove 960 754 click 1
sleep 3
shot 16-task-manager-from-tray

echo "== launch ferro-appearance dialog"
killall -9 ferro-taskmgr 2>/dev/null || true
/tmp/ferro-appearance >/out/appearance.log 2>&1 &
sleep 3
shot 17-display-properties

echo "== switch to dark slate theme and dark UI mode"
killall -9 ferro-appearance 2>/dev/null || true
/tmp/ferro-appearance --set-color 0x182026 --set-theme dark
sleep 2
xterm -geometry 60x12 -T "Command Prompt" -bg "#000000" -fg "#33FF33" -cr "#33FF33" -fa "DejaVu Sans Mono" -fs 10 -e sh -c 'echo "Dark mode active. Desktop: #182026"; sleep 900' &
sleep 2
shot 18-dark-mode-desktop

echo "== desktops (workspaces): the keys, the taskbar's buttons, and pagers (wmctrl)"
# Each check says PASS or FAIL in workspaces.txt.
check() {
    if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: got '$2', wanted '$3'"; fi >> /out/workspaces.txt
}
current() { wmctrl -d | awk '$2 == "*" { print $1 }'; }
desk_of() { wmctrl -l | awk -v t="$1" 'index($0, t) { print $2 }'; }
shown() { xdotool search --onlyvisible --name "$1" >/dev/null 2>&1 && echo yes || echo no; }
: > /out/workspaces.txt
check "four desktops to start with" "$(wmctrl -d | wc -l)" 4
check "on the first" "$(current)" 0
xterm -geometry 50x8+100+100 -T "Desk One" -e sh -c 'sleep 900' &
sleep 3
check "a new window goes on the desktop shown" "$(desk_of 'Desk One')" 0
xdotool key ctrl+super+Right
sleep 2
check "Ctrl+Win+Right: the next desktop" "$(current)" 1
check "the first desktop's window is put away" "$(shown 'Desk One')" no
shot 19-desktop-2-empty
xterm -geometry 50x8+300+300 -T "Desk Two" -e sh -c 'sleep 900' &
sleep 3
check "opened here: on desktop 2" "$(desk_of 'Desk Two')" 1
shot 20-desktop-2-window
wmctrl -s 0
sleep 2
check "a pager shows desktop 1 again" "$(current)" 0
check "its window is back" "$(shown 'Desk One')" yes
check "desktop 2's window is away" "$(shown 'Desk Two')" no
shot 21-back-on-desktop-1
wmctrl -r "Desk One" -t 2
sleep 2
check "a pager moves a window to desktop 3" "$(desk_of 'Desk One')" 2
check "moved away, it's off this desktop" "$(shown 'Desk One')" no
xdotool key ctrl+super+d
sleep 2
check "Ctrl+Win+D adds a fifth desktop" "$(wmctrl -d | wc -l)" 5
check "and shows it" "$(current)" 4
shot 22-new-desktop
xdotool key ctrl+super+F4
sleep 2
check "Ctrl+Win+F4 closes it" "$(wmctrl -d | wc -l)" 4
check "to the one on its left" "$(current)" 3
# The desktops' buttons sit left of the clock's box (1024 - 215 - 4 = 805):
# 4 x 18 wide plus 4 of border, from 725; desktop 3's is the third.
xdotool mousemove $((727 + 2 * 18 + 9)) 748 click 1
sleep 2
check "clicking desktop 3's button shows it" "$(current)" 2
check "with its window" "$(shown 'Desk One')" yes
shot 23-taskbar-desktop-button
xdotool key ctrl+super+shift+Left
sleep 2
check "Ctrl+Win+Shift+Left takes the window along" "$(current) $(desk_of 'Desk One')" "1 1"
check "still shown, with desktop 2's" "$(shown 'Desk One') $(shown 'Desk Two')" "yes yes"
shot 24-window-taken-along
# (wmctrl's -t -1 means the desktop shown; sticky is EWMH's "on all of them".)
wmctrl -r "Desk One" -b add,sticky
sleep 1
check "sticky: on all desktops" "$(desk_of 'Desk One')" -1
wmctrl -s 0
sleep 2
check "on all desktops: shown on desktop 1 too" "$(shown 'Desk One')" yes
shot 25-on-all-desktops
wmctrl -r "Desk One" -b remove,sticky
sleep 1
check "not sticky: on the desktop shown only" "$(desk_of 'Desk One')" 0
cat /out/workspaces.txt

echo "== the gate (ferro-xgate): a program asks before it sees the screen"
# The display itself is :5, its cookie root's; the gate is :7, for the
# test user's programs. This script (root) answers the question on :5.
gcheck() {
    if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: got '$2', wanted '$3'"; fi >> /out/xgate.txt
}
: > /out/xgate.txt
install -m 755 /in/ferro-xgate /usr/local/bin/ferro-xgate || fail "copy ferro-xgate"
useradd -m gatetest || fail "a test user"
xauth -q -f /tmp/x5.auth add :5 . "$(mcookie)" || fail "a cookie"
Xvfb :5 -screen 0 1024x768x24 -auth /tmp/x5.auth -nolisten tcp >/out/xvfb5.log 2>&1 &
sleep 3
real() { DISPLAY=:5 XAUTHORITY=/tmp/x5.auth "$@"; }
shot5() { real xwd -root -silent | xwdtopnm 2>/dev/null | pnmtopng > "/out/$1.png" || echo "screenshot $1 failed"; }
as_user() { su gatetest -s /bin/sh -c "cd; DISPLAY=:7 $*"; }
/usr/local/bin/ferro-xgate serve --display 7 --upstream 5 --auth /tmp/x5.auth --uid "$(id -u gatetest)" 2>/out/xgate.log &
sleep 1
install -m 755 /in/ferro-wm /usr/local/bin/ferro-wm || fail "install ferro-wm"
as_user /usr/local/bin/ferro-wm >/out/ferro-wm-gated.log 2>&1 &
sleep 3
# (Each of its connections says so; it has more than one.)
gcheck "the window manager is let through as it is" "$(grep -q 'ferro-wm (process [0-9]*) connected, trusted' /out/xgate.log && ! grep -q 'ferro-wm .*looked at' /out/xgate.log && echo yes)" yes
gcheck "and runs" "$(real xprop -root _NET_NUMBER_OF_DESKTOPS | awk '{ print $3 }')" 4
as_user "xterm -geometry 60x10+80+80 -T Private -e sh -c 'echo my bank details; sleep 900'" &
sleep 2
# The question's window, once it's up; its buttons (inside it): Allow once
# at 62,104, Allow until it quits at 206,104, Don't allow at 408,104.
question() {
    for _ in $(seq 40); do
        w=$(real xdotool search --name '^Screen access$' 2>/dev/null | head -1)
        [ -n "$w" ] && break
        sleep 0.1
    done
    echo "$w"
}
press() { real xdotool mousemove --window "$1" "$2" 104 click 1; }
probe() { grep "^$1:" "$2" | cut -d' ' -f2; }

as_user /usr/local/bin/ferro-xgate probe >/tmp/p1.txt 2>&1 &
w=$(question)
gcheck "a screenshot puts up the question" "$([ -n "$w" ] && echo up)" up
gcheck "on every desktop" "$(real xprop -id "$w" _NET_WM_DESKTOP | awk '{ print $3 }')" 4294967295
# Pressed at once, Allow once does nothing: a click meant for something else mustn't allow it.
press "$w" 62
sleep 1.5
shot5 26-screen-access-question
gcheck "a click in its first second doesn't allow" "$(real xdotool search --name '^Screen access$' | wc -l)" 1
press "$w" 408
sleep 2
gcheck "Don't allow: a black picture" "$(probe getimage /tmp/p1.txt)" black
gcheck "copying the screen: nothing copied" "$(probe copyarea /tmp/p1.txt)" black
gcheck "MIT-SHM: nothing" "$(probe shmgetimage /tmp/p1.txt)" black
gcheck "XTEST (pretend input) hidden" "$(probe xtest /tmp/p1.txt)" hidden
gcheck "RECORD (keylogging) hidden" "$(probe record /tmp/p1.txt)" hidden
gcheck "the question is gone" "$(real xdotool search --name '^Screen access$' | wc -l)" 0

as_user /usr/local/bin/ferro-xgate probe >/tmp/p2.txt 2>&1 &
w=$(question)
sleep 1.5
press "$w" 62
sleep 2
gcheck "Allow once: it sees the screen" "$(probe getimage /tmp/p2.txt)" seen
gcheck "and may copy it" "$(probe copyarea /tmp/p2.txt)" seen
gcheck "and through MIT-SHM" "$(probe shmgetimage /tmp/p2.txt)" seen

as_user /usr/local/bin/ferro-xgate probe hold >/tmp/p3.txt 2>&1 &
w=$(question)
sleep 1.5
press "$w" 206
sleep 2
gcheck "Allow until it quits: it sees the screen" "$(probe getimage /tmp/p3.txt)" seen
gcheck "the taskbar is told who" "$(real xprop -root _FERRO_SCREEN | sed -n 's/.*= "\(.*\)"/\1/p')" ferro-xgate
shot5 27-screen-in-use-light
sleep 6
gcheck "once it's quit, nobody" "$(real xprop -root _FERRO_SCREEN | sed -n 's/.*= "\(.*\)"/\1/p')" ""

DISPLAY=:7 timeout 10 /usr/local/bin/ferro-xgate probe >/tmp/p4.txt 2>&1
gcheck "root's programs aren't asked" "$(probe getimage /tmp/p4.txt)" seen
as_user "xterm -geometry 40x6+500+400 -T Still-works -e sh -c 'sleep 900'" &
sleep 2
gcheck "ordinary programs work through it" "$(real xdotool search --name '^Still-works$' | wc -l)" 1
shot5 28-through-the-gate
cat /out/xgate.txt

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(ls /out/*.png | wc -l) screenshots, $(grep -c '^FAIL' /out/workspaces.txt) desktop checks failed, $(grep -c '^FAIL' /out/xgate.txt) gate checks failed"
