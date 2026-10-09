#!/bin/sh
# /init of the WireGuard test server VM, run by `cargo xtask wg-test-server`.
# A throwaway VPN server for testing FerroOS's VPN: it forwards its client's
# traffic to the internet (NAT), and prints a client profile between
# FERRO-WG-CONFIG-BEGIN/END for xtask to save. Its keys exist only in this
# VM's RAM and in that profile. Not for real use.

export PATH=/usr/sbin:/usr/bin:/sbin:/bin

fail() { echo "FERRO-WG-FAILED: $*"; sync; poweroff -f; }

mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
for m in failover net_failover virtio_net virtio_blk; do
    insmod /builder/modules/$m.ko || fail "insmod $m"
done

ip link set lo up
ip link set eth0 up
ip addr add 10.0.2.15/24 dev eth0
ip route add default via 10.0.2.2
echo "nameserver 10.0.2.3" > /etc/resolv.conf

apk add --no-cache -q kmod wireguard-tools-wg iproute2 nftables >/dev/null || fail "apk add"
KVER=$(ls /lib/modules | head -n1)
depmod -a "$KVER" || fail "depmod"
modprobe wireguard || fail "modprobe wireguard"

umask 077
wg genkey > /tmp/server.key
wg pubkey < /tmp/server.key > /tmp/server.pub
wg genkey > /tmp/client.key
wg pubkey < /tmp/client.key > /tmp/client.pub

ip link add wg0 type wireguard || fail "wg0"
wg set wg0 listen-port 51820 private-key /tmp/server.key peer "$(cat /tmp/client.pub)" allowed-ips 10.66.0.2/32 || fail "wg set"
ip addr add 10.66.0.1/24 dev wg0
ip link set wg0 up
echo 1 > /proc/sys/net/ipv4/ip_forward
nft add table ip nat || fail "nft"
nft add chain ip nat post '{ type nat hook postrouting priority 100 ; }'
nft add rule ip nat post oifname eth0 masquerade

echo "FERRO-WG-CONFIG-BEGIN"
cat <<EOF
# WireGuard test profile from 'cargo xtask wg-test-server' (QEMU only)
[Interface]
PrivateKey = $(cat /tmp/client.key)
Address = 10.66.0.2/32

[Peer]
PublicKey = $(cat /tmp/server.pub)
Endpoint = 10.0.2.2:51820
AllowedIPs = 0.0.0.0/0
PersistentKeepalive = 25
EOF
echo "FERRO-WG-CONFIG-END"
rm /tmp/client.key

echo "wg-test-server: listening on UDP 51820"
while true; do
    sleep 20
    wg show wg0 latest-handshakes | awk '{ if ($2 > 0) print "wg-test-server: client handshake at", $2; else print "wg-test-server: no handshake yet" }'
    wg show wg0 transfer | awk '{ print "wg-test-server: received", $2, "bytes, sent", $3 }'
done
