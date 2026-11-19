#!/bin/sh
# The switch test VM's job, run by `cargo xtask switch-test` inside the
# minimal Debian that tools/vm/init.sh installs: ferro-switch-hardware keeps
# the microphone, camera and network as switched, the network over a
# firewall reload too, and ferro-update's one-time catch-up switches off
# what the taskbar showed off.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    kmod iproute2 nftables iputils-ping procps >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

# The installer's kernel has only installation drivers: veth and nftables'
# are in its full package.
KVER=$(uname -r)
mkdir -p /tmp/kernel && cd /tmp/kernel || fail "cd"
apt-get download -q "linux-image-$KVER" >/tmp/apt.log 2>&1 || {
    tail -5 /tmp/apt.log
    fail "download linux-image-$KVER"
}
dpkg-deb -x linux-image-*.deb . || fail "unpack kernel package"
MODULES=$(find . -type d -path "*/lib/modules/$KVER" | head -n1)
[ -n "$MODULES" ] || fail "no modules in the kernel package"
rm -rf "/lib/modules/$KVER" && mkdir -p /lib/modules && mv "$MODULES" /lib/modules/ || fail "install modules"
cd / && rm -rf /tmp/kernel
depmod -a "$KVER" || fail "depmod"
modprobe veth || fail "modprobe veth"
for m in nfnetlink nf_tables nft_reject nft_reject_inet nf_reject_ipv4 nf_reject_ipv6 nft_ct nft_fib nft_fib_inet; do modprobe "$m" 2>/dev/null; done
nft list tables >/dev/null || fail "nftables"
mkdir -p /etc/udev/rules.d

install -m 755 /in/heavy/ferro-switch-hardware /usr/local/sbin/ || fail "install ferro-switch-hardware"
S=/usr/local/sbin/ferro-switch-hardware
F=/var/lib/ferro
FAILS=0
check() {
    if [ "$2" = "$3" ]; then
        echo "PASS $1"
    else
        echo "FAIL $1: got '$2', wanted '$3'"
        FAILS=$((FAILS + 1))
    fi
}
has() { [ -e "$1" ] && echo yes || echo no; }
table() { nft list table inet ferro_net_off >/dev/null 2>&1 && echo yes || echo no; }
# The other end of a virtual cable, in a namespace of its own.
ip netns add peer || fail "netns"
ip link add v0 type veth peer name v1 || fail "veth"
ip link set v1 netns peer
ip addr add 192.168.73.1/24 dev v0
ip -n peer addr add 192.168.73.2/24 dev v1
ip -n peer link set v1 up
ip -n peer link set lo up
cable() { ip link set v0 up; }
reaches() { ping -c 1 -W 1 192.168.73.2 >/dev/null 2>&1 && echo yes || echo no; }
cable
check "the cable works" "$(reaches)" yes

# ---- the network
$S net off >/tmp/net.log 2>&1
check "network off: kept" "$(has $F/net-off)" yes
check "network off: the firewall table" "$(table)" yes
# A person's program brings the link back: the table still holds.
cable
check "network off, link back up: nothing goes out" "$(reaches)" no
# A firewall reload (nftables.service: flush ruleset) drops the table...
nft flush ruleset
check "the reload dropped it (so this test means something)" "$(reaches)" yes
# ...and the reload's ExecReload puts it back.
$S reapply
check "reapply: the table back" "$(table)" yes
check "reapply: nothing goes out again" "$(reaches)" no
$S net on >>/tmp/net.log 2>&1
cable
check "network on: no longer kept off" "$(has $F/net-off)" no
check "network on: no table" "$(table)" no
check "network on: it reaches" "$(reaches)" yes
$S reapply
check "reapply with the network on: no table" "$(table)" no
check "reapply only from root's own command line" "$($S reapply extra >/dev/null 2>&1 && echo ran || echo refused)" refused

# ---- the microphone and camera
$S mic off >/dev/null 2>&1
$S cam off >/dev/null 2>&1
check "microphone and camera off: kept" "$(has $F/mic-off) $(has $F/cam-off)" "yes yes"
check "and the device rule's there" "$(grep -c '^[A-Z].*TEST=="/var/lib/ferro/' /etc/udev/rules.d/72-ferro-switches.rules)" 3
$S mic on >/dev/null 2>&1
$S cam on >/dev/null 2>&1
check "back on: no longer kept off" "$(has $F/mic-off) $(has $F/cam-off)" "no no"

# ---- ferro-update's one-time catch-up, as it is in heavy/ferro-update
sed -n '/^# Older installs only switched the microphone and camera off/,/^# Older installs let the sudo group/p' /in/heavy/ferro-update |
    sed '$d' >/tmp/catch-up.sh
[ -s /tmp/catch-up.sh ] || fail "the catch-up isn't in ferro-update"
systemctl() { :; }
catch_up() { (. /tmp/catch-up.sh) >/tmp/catch-up.log 2>&1; }
reset() { rm -f $F/mic-off $F/cam-off $F/net-off $F/switches-kept /ProgramData/ferro/choices.conf /home/*/.config/ferro/switches.conf; }

# A new install from before this, never switched: the taskbar showed both off.
reset
catch_up
check "never switched: microphone and camera off, as shown" "$(has $F/mic-off) $(has $F/cam-off)" "yes yes"
check "and noted, so it's done once" "$(has $F/switches-kept)" yes
check "and the network left on" "$(has $F/net-off)" no
# Someone switched the microphone on (the Network window's file).
reset
mkdir -p /home/pat/.config/ferro
printf 'network = true\nmicrophone = true\ncamera = false\n' >/home/pat/.config/ferro/switches.conf
catch_up
check "microphone switched on: left on, camera off" "$(has $F/mic-off) $(has $F/cam-off)" "no yes"
# Someone switched the camera on (the taskbar's file).
reset
mkdir -p /ProgramData/ferro
echo 'switch network=on microphone=off camera=on vpn=off tor=off' >/ProgramData/ferro/choices.conf
catch_up
check "camera switched on in the taskbar: left on" "$(has $F/mic-off) $(has $F/cam-off)" "yes no"
# The network off right now (its table loaded) is kept off from now on.
reset
$S net off >/dev/null 2>&1
rm -f $F/net-off
catch_up
check "network off right now: kept off" "$(has $F/net-off)" yes
# Done once: switched on afterwards, an update doesn't switch it off again.
$S mic on >/dev/null 2>&1
catch_up
check "once only: switched on later, left on" "$(has $F/mic-off)" no
check "the reload rule for nftables" "$(grep -c '^Exec.*=-/usr/local/sbin/ferro-switch-hardware reapply$' /etc/systemd/system/nftables.service.d/62-ferro-switches.conf)" 3

echo "FERRO-BUILD-OK switch test: $FAILS checks failed"
[ "$FAILS" -eq 0 ] || {
    echo "== net log"
    cat /tmp/net.log
    echo "== catch-up log"
    cat /tmp/catch-up.log
}
