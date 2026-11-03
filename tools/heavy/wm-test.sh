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
    xvfb xterm x11-apps x11-utils x11-xserver-utils xauth xclip xdotool wmctrl netpbm psmisc sudo argon2 >>/tmp/apt.log 2>&1 || {
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
as_user() { su gatetest -s /bin/sh -c "cd; export DISPLAY=:7; $*"; }
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
# Its title (often the file that's open) is for the display, not other programs.
pw=$(real xdotool search --name '^Private$' | head -1)
gcheck "the display has its title" "$(real xprop -id "$pw" WM_NAME | grep -c '"Private"')" 1
gcheck "another program reads no title" "$(as_user "xprop -id $pw WM_NAME _NET_WM_NAME" | grep -c Private)" 0
gcheck "nor finds the window by it" "$(as_user "xdotool search --name Private" 2>/dev/null | wc -l)" 0
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

# The desktop's start (xinitrc) sets the background and the mouse pointer:
# a cursor made from a picture of its own, which mustn't ask.
as_user "timeout 20 xsetroot -solid '#008080' -cursor_name left_ptr"
gcheck "setting the pointer doesn't ask" "$? $(real xdotool search --name '^Screen access$' 2>/dev/null | wc -l)" "0 0"

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

# What's where the question shows, without it (root's programs aren't asked).
DISPLAY=:7 timeout 10 /usr/local/bin/ferro-xgate probe >/tmp/p0.txt 2>&1
as_user /usr/local/bin/ferro-xgate probe >/tmp/p2.txt 2>&1 &
w=$(question)
sleep 1.5
press "$w" 62
sleep 2
gcheck "Allow once: it sees the screen" "$(probe getimage /tmp/p2.txt)" seen
# The question's grey (c0c0c0) where it was would mean the picture has it in.
gcheck "without the question in the picture" "$(probe middle /tmp/p2.txt)" "$(probe middle /tmp/p0.txt)"
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
# What's copied is for the program being used (xclip -o never is).
as_user "printf secret | xclip -selection clipboard" >/dev/null 2>&1 &
sleep 1
gcheck "the display has what's copied" "$(real timeout 5 xclip -o -selection clipboard)" secret
start=$(date +%s)
gcheck "a program in the background gets nothing" "$(as_user "timeout 5 xclip -o -selection clipboard" 2>/dev/null)" ""
gcheck "and isn't kept waiting" "$([ $(($(date +%s) - start)) -lt 4 ] && echo yes)" yes
as_user "printf picked | xclip -selection primary" >/dev/null 2>&1 &
sleep 1
gcheck "the display has the selected text" "$(real timeout 5 xclip -o -selection primary)" picked
gcheck "a program in the background doesn't" "$(as_user "timeout 5 xclip -o -selection primary" 2>/dev/null)" ""
# Which folder a File Manager window shows: the File Manager's only.
real xprop -id "$pw" -f _FERRO_CURRENT_DIR 8u -set _FERRO_CURRENT_DIR /home/gatetest/Taxes
gcheck "a window's folder: the display has it" "$(real xprop -id "$pw" _FERRO_CURRENT_DIR | grep -c Taxes)" 1
gcheck "another program doesn't" "$(as_user "xprop -id $pw _FERRO_CURRENT_DIR" | grep -c Taxes)" 0
shot5 28-through-the-gate
cat /out/xgate.txt

echo "== root's scripts: what a program running as an administrator can't do"
rcheck() {
    if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: got '$2', wanted '$3'"; fi >> /out/root.txt
}
: > /out/root.txt
mkdir -p /usr/local/sbin /usr/local/lib/ferro-heavy /etc/sudoers.d
install -m 755 /in/heavy/ferro-drive /in/heavy/ferro-vpn /usr/local/sbin/
install -m 755 /in/heavy/check-password /usr/local/lib/ferro-heavy/check-password
# Importing a VPN config leaves /etc/ferro root's alone.
/usr/local/sbin/ferro-vpn import </dev/null >/dev/null 2>&1
rcheck "importing a VPN config: /etc/ferro stays root's" "$(stat -c '%a %U' /etc/ferro)" "755 root"
# Saved by a Windows program (a byte-order mark, CRLF lines): taken, its
# commands left out.
printf '\357\273\277[Interface]\r\nPrivateKey = abc=\r\nPostUp = touch /tmp/pwned\r\n[Peer]\r\nEndpoint = 192.0.2.1:51820\r\nAllowedIPs = 0.0.0.0/0\r\n' >/tmp/win.conf
/usr/local/sbin/ferro-vpn import </tmp/win.conf >/dev/null 2>&1
rcheck "a config saved on Windows is taken, without its commands" "$? $(head -1 /etc/ferro/vpn.conf) $(grep -c PostUp /etc/ferro/vpn.conf) $(grep -c 'state=ready' /run/ferro/vpn-status)" "0 [Interface] 0 1"
rcheck "what isn't a config: refused, saying why" "$(echo hello | /usr/local/sbin/ferro-vpn import 2>&1 >/dev/null | tail -1)" "ferro-vpn: what was given isn't a WireGuard config (no [Interface] section)"
# The VPN on: a new config is used at once (here, with no WireGuard tools,
# switching on says so; it's still imported).
mkdir -p /var/lib/ferro && printf '192.0.2.1 51820\n' >/var/lib/ferro/vpn-on
/usr/local/sbin/ferro-vpn import </tmp/win.conf >/dev/null 2>&1
rcheck "importing with the VPN on: switched on again with it" "$? $(grep -c 'tools aren' /run/ferro/vpn-status)" "0 1"
rm -f /var/lib/ferro/vpn-on

# The drive list: only entries as ferro-drive writes them.
u=12345678-1234-1234-1234-123456789abc
mkdir -p -m 700 /etc/ferro/drive-keys
(umask 277 && head -c 64 /dev/urandom >/etc/ferro/drive-keys/$u.key)
drive() {
    printf '%s\n' "$1" >/etc/ferro/drives && chmod 644 /etc/ferro/drives
    /usr/local/sbin/ferro-drive open $u 2>&1 | sed 's/^ferro-drive: //'
}
rcheck "a drive as it's remembered: let through" "$(drive "$u|ferro-12345678|/Drives/Backup 2")" "that drive isn't plugged in"
rcheck "a made-up name for it: refused" "$(drive "$u|../../dev/sda|/Drives/Backup")" "$u's entry in /etc/ferro/drives has an unexpected name"
rcheck "mounted over a system folder: refused" "$(drive "$u|ferro-12345678|/usr/bin")" "$u's entry in /etc/ferro/drives has an unexpected place"
rcheck "nor through /Drives/..: refused" "$(drive "$u|ferro-12345678|/Drives/../usr")" "$u's entry in /etc/ferro/drives has an unexpected place"
chmod 775 /etc/ferro
rcheck "a list others could change: not trusted" "$(drive "$u|ferro-12345678|/Drives/Backup")" "/etc/ferro isn't root's alone: not trusted"
chmod 755 /etc/ferro

# A duress PIN someone else put there isn't one. (If it were taken for
# one, this machine would switch off here and the test would fail.)
salt=abcdefghijklmnop
h=$(printf %s 1234567890 | argon2 $salt -id -t 1 -m 10 -p 1 -l 32 -r)
echo "argon2id 1 10 $salt $h" >/etc/ferro/duress.hash
chown gatetest /etc/ferro/duress.hash && chmod 600 /etc/ferro/duress.hash
printf 'gatetest\n1234567890\n' | /usr/local/lib/ferro-heavy/check-password >/dev/null 2>&1
rcheck "a duress PIN that isn't root's: nothing erased, still running" "$?" 1
rm -f /etc/ferro/duress.hash

# The sudo rules as harden.sh writes them, for an administrator (sudo group).
for f in ferro-tor ferro-switch-hardware; do
    printf '#!/bin/sh\necho "$*" >>/tmp/ran\n' >/usr/local/sbin/$f && chmod 755 /usr/local/sbin/$f
done
sed -n '/^cat >\/etc\/sudoers.d\/60-ferro-tor/,/^chmod 0440 \/etc\/sudoers.d\/60-ferro-vpn/p' /in/heavy/harden.sh >/tmp/rules.sh
sh /tmp/rules.sh >/tmp/rules.log 2>&1
rcheck "harden.sh's sudo rules are valid" "$(visudo -c >/dev/null 2>&1 && echo yes)" yes
usermod -aG sudo gatetest
cp /usr/local/sbin/ferro-vpn /tmp/ferro-vpn.real
printf '#!/bin/sh\necho "$*" >>/tmp/ran\n' >/usr/local/sbin/ferro-vpn
ran() {
    rm -f /tmp/ran
    su gatetest -s /bin/sh -c "sudo -n $* </dev/null" >/dev/null 2>&1
    [ -s /tmp/ran ] && echo yes || echo no
}
rcheck "Tor on: no password" "$(ran /usr/local/sbin/ferro-tor on) $(ran /usr/local/sbin/ferro-tor on bridges)" "yes yes"
rcheck "Tor off: asks" "$(ran /usr/local/sbin/ferro-tor off)" no
rcheck "VPN and its kill switch on: no password" "$(ran /usr/local/sbin/ferro-vpn on) $(ran /usr/local/sbin/ferro-vpn killswitch on)" "yes yes"
rcheck "VPN off, kill switch off, another config: ask" "$(ran /usr/local/sbin/ferro-vpn off) $(ran /usr/local/sbin/ferro-vpn killswitch off) $(ran /usr/local/sbin/ferro-vpn import)" "no no no"
rcheck "microphone, camera, network off: no password" "$(ran /usr/local/sbin/ferro-switch-hardware mic off) $(ran /usr/local/sbin/ferro-switch-hardware cam off) $(ran /usr/local/sbin/ferro-switch-hardware net off)" "yes yes yes"
rcheck "back on: asks" "$(ran /usr/local/sbin/ferro-switch-hardware mic on) $(ran /usr/local/sbin/ferro-switch-hardware cam on) $(ran /usr/local/sbin/ferro-switch-hardware net on) $(ran /usr/local/sbin/ferro-switch-hardware apply-all on on on)" "no no no no"

# The password box gets what's typed: on its own, and when the network
# menu (which holds the keyboard and mouse while open) asks for it.
install -m 755 /in/ferro-askpass /in/ferro-flyout /usr/local/bin/
echo 'gatetest:Pass-1234' | chpasswd
askpass_up() {
    for _ in $(seq 50); do
        real xdotool search --name '^Run as administrator$' >/dev/null 2>&1 && return 0
        sleep 0.1
    done
    return 1
}
type_password() {
    askpass_up || { echo none; return; }
    sleep 0.5
    real xdotool type --delay 40 'Pass-1234'
    real xdotool key Return
    sleep 2
    pkill -x ferro-askpass 2>/dev/null
    echo up
}
rm -f /tmp/ran
as_user "SUDO_ASKPASS=/usr/local/bin/ferro-askpass FERRO_ADMIN_FOR='Switching VPN off' sudo -k -A /usr/local/sbin/ferro-vpn off </dev/null" >/dev/null 2>&1 &
rcheck "the password box takes typing" "$(type_password) $(cat /tmp/ran 2>/dev/null)" "up off"
rm -f /tmp/ran
as_user "mkdir -p .config/ferro && printf 'vpn = true\n' >.config/ferro/switches.conf"
as_user "/usr/local/bin/ferro-flyout network 500" >/out/flyout.log 2>&1 &
sleep 2
shot5 29-network-menu
real xdotool mousemove 498 591 click 1
rcheck "switching the VPN off from the network menu: the box takes typing" "$(type_password) $(cat /tmp/ran 2>/dev/null)" "up off"
rcheck "and the menu says it's off" "$(as_user "grep -c 'vpn = false' .config/ferro/switches.conf")" 1
pkill -x ferro-flyout 2>/dev/null
cp /tmp/ferro-vpn.real /usr/local/sbin/ferro-vpn

# An update puts right a PC where /etc/ferro was opened to the sudo group.
awk '/^# \/etc\/ferro: root.s alone again/ { on = 1 } on { print } on && /^fi$/ { exit }' /in/heavy/ferro-update >/tmp/repair.sh
chgrp sudo /etc/ferro && chmod 775 /etc/ferro
printf 'desktop_color = 0x123456\n' >/etc/ferro/appearance.conf && chown gatetest /etc/ferro/appearance.conf
printf 'x\n' >/etc/ferro/planted && chown gatetest /etc/ferro/planted && chmod 666 /etc/ferro/planted
ln -sf /etc/passwd /etc/ferro/link
sh /tmp/repair.sh
rcheck "the update: /etc/ferro root's again" "$(stat -c '%a %U:%G' /etc/ferro)" "755 root:root"
rcheck "the logon screen's look kept, in its own folder" "$(cat /etc/ferro/logon/appearance.conf) $(stat -c '%a %G' /etc/ferro/logon)" "desktop_color = 0x123456 2775 sudo"
rcheck "and gone from /etc/ferro" "$([ -e /etc/ferro/appearance.conf ] && echo there || echo gone)" gone
rcheck "what someone else put there: root's, unwritable" "$(stat -c '%a %U' /etc/ferro/planted)" "644 root"
rcheck "a link in it isn't followed" "$(stat -c '%a %U' /etc/passwd)" "644 root"
rm -f /etc/ferro/logon/appearance.conf
ln -sf /etc/shadow /etc/ferro/appearance.conf
sh /tmp/repair.sh
rcheck "a logon look that's a link to root's: not copied" "$(cat /etc/ferro/logon/appearance.conf 2>/dev/null | grep -c root)" 0
cat /out/root.txt

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(ls /out/*.png | wc -l) screenshots, $(grep -c '^FAIL' /out/workspaces.txt) desktop checks failed, $(grep -c '^FAIL' /out/xgate.txt) gate checks failed, $(grep -c '^FAIL' /out/root.txt) root script checks failed"
