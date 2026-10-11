#!/bin/sh
# /init of every build VM. The VM boots Debian's network installer (its
# kernel, and its initrd: a small busybox system). This installs a minimal
# Debian into RAM with debootstrap, which checks the archive's signatures,
# and runs the VM's job (/builder/run.sh) inside it. @SUITE@ and @TMPFS@ are
# filled in by xtask.

export PATH=/usr/sbin:/usr/bin:/sbin:/bin
MIRROR=http://deb.debian.org/debian

fail() { echo "FERRO-BUILD-FAILED: $*"; sync; poweroff -f; }

mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
while read -r m; do
    insmod "/builder/modules/$m.ko" || fail "insmod $m"
done < /builder/modules/order

# QEMU user networking is fixed: guest 10.0.2.15, gateway .2, DNS .3.
ip link set lo up
ip link set eth0 up
ip addr add 10.0.2.15/24 dev eth0
ip route add default via 10.0.2.2
echo "nameserver 10.0.2.3" > /etc/resolv.conf

# A second disk is scratch space: as swap, it lets what's in RAM (Debian and
# the build) grow past the VM's memory.
if [ -b /dev/vdb ]; then
    mkswap /dev/vdb >/dev/null && swapon /dev/vdb || fail "swap"
fi
mkdir -p /target
mount -t tmpfs -o size=@TMPFS@ tmpfs /target || fail "tmpfs"

echo "== installing Debian (@SUITE@)"
debootstrap --variant=minbase --arch=amd64 --force-check-gpg \
    --keyring=/usr/share/keyrings/debian-archive-keyring.gpg \
    @SUITE@ /target "$MIRROR" >/tmp/debootstrap.log 2>&1 || {
    tail -20 /tmp/debootstrap.log
    fail "debootstrap"
}
rm -f /target/var/cache/apt/archives/*.deb
cp /etc/resolv.conf /target/etc/resolv.conf

mount -t proc proc /target/proc
mount -t sysfs sys /target/sys
mount -t devtmpfs dev /target/dev
mkdir -p /target/dev/pts && mount -t devpts devpts /target/dev/pts
mv /builder /target/builder || fail "move job"
if [ -d /in ]; then
    mv /in /target/in || fail "move inputs"
fi

chroot /target /usr/bin/env -i PATH="$PATH" HOME=/root DEBIAN_FRONTEND=noninteractive \
    /bin/sh /builder/run.sh
sync
poweroff -f
