#!/bin/sh
# The VPN test VM's job, run by `cargo xtask vpn-test` inside the minimal
# Debian that tools/vm/init.sh installs. Two WireGuard servers, each in a
# network namespace of its own behind a virtual cable, and ferro-vpn with a
# config for each: one server stops answering, and its watcher must switch
# to the other (and back), with the kill switch on throughout.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    kmod wireguard-tools iproute2 nftables iputils-ping procps >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

# The installer's kernel has only installation drivers: WireGuard, veth and
# nftables' are in its full package (as tools/wg-test/server.sh does).
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
modprobe wireguard || fail "modprobe wireguard"
modprobe veth || fail "modprobe veth"
for m in nfnetlink nf_tables nft_reject nft_reject_inet nf_reject_ipv4 nf_reject_ipv6 nft_ct nft_fib nft_fib_inet; do modprobe "$m" 2>/dev/null; done
nft list tables >/dev/null || fail "nftables"
# wg-quick hands wg its settings through /dev/fd, which this system lacks.
[ -e /dev/fd ] || ln -s /proc/self/fd /dev/fd

install -m 755 /in/heavy/ferro-vpn /usr/local/sbin/ || fail "install ferro-vpn"
V=/usr/local/sbin/ferro-vpn
FAILS=0
check() {
    if [ "$2" = "$3" ]; then
        echo "PASS $1"
    else
        echo "FAIL $1: got '$2', wanted '$3'"
        FAILS=$((FAILS + 1))
    fi
}
using() { cat /var/lib/ferro/vpn-using 2>/dev/null; }
state() { sed -n 's/^state=//p' /run/ferro/vpn-status; }
endpoint() { wg show wg0 endpoints 2>/dev/null | awk '{ print $2 }'; }
# Waits up to $2 seconds for the config in use to be $1.
await() {
    n=0
    while [ "$(using)" != "$1" ] && [ "$n" -lt "$2" ]; do
        sleep 1
        n=$((n + 1))
    done
}

# Server N: 192.168.7N.2 (namespace sN), its tunnel 10.7N.0.1; the client's
# config for it in /tmp/cN.conf.
server() {
    ns=s$1
    ip netns add "$ns" || fail "netns $ns"
    ip link add "v$1" type veth peer name "v$1p" || fail "veth $1"
    ip link set "v$1p" netns "$ns"
    ip addr add "192.168.7$1.1/24" dev "v$1"
    ip link set "v$1" up
    ip netns exec "$ns" ip addr add "192.168.7$1.2/24" dev "v$1p"
    ip netns exec "$ns" ip link set "v$1p" up
    ip netns exec "$ns" ip link set lo up
    umask 077
    wg genkey >"/tmp/s$1.key" && wg pubkey <"/tmp/s$1.key" >"/tmp/s$1.pub"
    wg genkey >"/tmp/c$1.key" && wg pubkey <"/tmp/c$1.key" >"/tmp/c$1.pub"
    umask 022
    ip netns exec "$ns" ip link add wg0 type wireguard || fail "server $1 wg0"
    ip netns exec "$ns" wg set wg0 listen-port 51820 private-key "/tmp/s$1.key" peer "$(cat "/tmp/c$1.pub")" allowed-ips "10.7$1.0.2/32"
    ip netns exec "$ns" ip addr add "10.7$1.0.1/24" dev wg0
    ip netns exec "$ns" ip link set wg0 up
    printf '[Interface]\nPrivateKey = %s\nAddress = 10.7%s.0.2/32\n\n[Peer]\nPublicKey = %s\nEndpoint = 192.168.7%s.2:51820\nAllowedIPs = 0.0.0.0/0\n' \
        "$(cat "/tmp/c$1.key")" "$1" "$(cat "/tmp/s$1.pub")" "$1" >"/tmp/c$1.conf"
}
server 1
server 2

$V import first.conf </tmp/c1.conf >/dev/null || fail "import first"
check "a second config: added to the list" "$($V import second.conf </tmp/c2.conf)" "ferro-vpn: added second to the list (2 configs)"
check "the list: the first in use" "$($V list | tr '\n' ' ')" "01-first in-use 02-second "
check "the same config again: not added twice" "$($V import again </tmp/c1.conf)" "ferro-vpn: that config is already on the list (first)"
check "the list's names can be read by anyone (the Network window)" "$(stat -c %a /etc/ferro/vpn.d /var/lib/ferro/vpn-using | tr '\n' ' ')" "755 644 "
$V on >/tmp/on.log 2>&1
[ "$(state)" = connected ] || { echo "== on said:"; cat /tmp/on.log; cat /run/ferro/vpn-status; }
check "on: connected to the first server" "$(state) $(endpoint)" "connected 192.168.71.2:51820"
check "the kill switch lets both servers through" "$(nft list set inet ferro_vpn servers4 | grep -o '192\.168\.7[12]\.2' | sort -u | tr '\n' ' ')" "192.168.71.2 192.168.72.2 "

FERRO_VPN_EVERY=3 $V watch 2>/tmp/watch.log &
W=$!
sleep 25
check "the server answering: kept" "$(using) $(endpoint)" "01-first 192.168.71.2:51820"

# The first server stops answering.
ip netns exec s1 ip link set wg0 down
await 02-second 90
sleep 3
check "the first server gone: switched to the second" "$(using) $(state) $(endpoint)" "02-second connected 192.168.72.2:51820"
check "and the status says why" "$(grep -c 'first stopped answering: switched to second' /run/ferro/vpn-status)" 1
check "the kill switch still on" "$(nft list table inet ferro_vpn >/dev/null 2>&1 && echo on)" on
check "nothing goes out around the tunnel meanwhile" "$(ping -c 1 -W 2 -I v1 192.168.71.9 >/dev/null 2>&1 && echo out || echo blocked)" blocked

# And the second stops while the first is back: round to the first.
ip netns exec s1 ip link set wg0 up
ip netns exec s2 ip link set wg0 down
await 01-first 90
sleep 3
check "the second gone, the first back: switched to the first" "$(using) $(state) $(endpoint)" "01-first connected 192.168.71.2:51820"
cp /etc/ferro/vpn.conf /tmp/inuse.conf
check "vpn.conf is the config in use" "$(grep -c 'Endpoint = 192.168.71.2' /tmp/inuse.conf)" 1

# Chosen by hand.
ip netns exec s2 ip link set wg0 up
$V use 02-second >/dev/null 2>&1
check "use: that config, at once" "$(using) $(state) $(endpoint)" "02-second connected 192.168.72.2:51820"
check "use: only the list's" "$($V use 03-../../etc >/dev/null 2>&1; echo $?) $($V use 07-none >/dev/null 2>&1; echo $?)" "2 1"

# Off: left alone, however long nothing answers.
$V off >/dev/null
ip netns exec s2 ip link set wg0 down
sleep 40
check "off: the watcher leaves it" "$(using) $(ip link show wg0 >/dev/null 2>&1 && echo up || echo down)" "02-second down"

# Removed while in use: the next is in use.
$V remove 02-second >/dev/null
check "removing the one in use: the next is" "$($V list | tr '\n' ' ')" "01-first in-use "
$V remove 01-first >/dev/null
check "removing the last: no config" "$($V list | wc -l) $([ -e /etc/ferro/vpn.conf ] && echo there || echo none) $(state)" "0 none no_config"

# A config from before the list: its first.
cp /tmp/c1.conf /etc/ferro/vpn.conf
check "a config from before the list: becomes its first" "$($V list | tr '\n' ' ')" "01-main in-use "

kill "$W" 2>/dev/null
echo "== watcher's log"
cat /tmp/watch.log
echo "FERRO-BUILD-OK VPN test: $FAILS checks failed"
