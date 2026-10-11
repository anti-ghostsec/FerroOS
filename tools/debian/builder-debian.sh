#!/bin/sh
# The Debian layer build VM's job, run by `cargo xtask debian-build` inside
# the minimal Debian that tools/vm/init.sh installs. Installs a second minimal
# Debian (the one that ships), packs it into a compressed read-only image, and
# writes that as a tar stream onto the raw disk /dev/vda for the host to read.
# @SUITE@ is filled in by xtask.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /build /out

echo "== installing build tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    debootstrap debian-archive-keyring mount squashfs-tools cryptsetup-bin >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

echo "== installing the Debian that ships"
# minbase: the C library and loader, the C++ runtime, a shell, the core
# tools and apt. Signatures are checked. On top: certificates for HTTPS; what
# FerroOS uses to keep your changes on an encrypted partition; and Wi-Fi: the
# WPA2/WPA3 program and the firmware files the cards' drivers load (the one
# part of the image that isn't free software, hence the second component).
debootstrap --variant=minbase --arch=amd64 --force-check-gpg \
    --components=main,non-free-firmware \
    --include=ca-certificates,cryptsetup-bin,e2fsprogs,wpasupplicant,iw,wireless-regdb,firmware-iwlwifi,firmware-realtek,firmware-mediatek,firmware-atheros \
    @SUITE@ /build/root \
    http://deb.debian.org/debian >/tmp/debootstrap.log 2>&1 || {
    tail -20 /tmp/debootstrap.log
    fail "debootstrap"
}
cp /build/root/etc/debian_version /out/debian.version || fail "version"

# apt's sources, with security updates, over HTTPS so the network can't see
# which packages are fetched. Names resolve through FerroOS's encrypted
# resolver; nothing may start services (there's no init to ask).
cat > /build/root/etc/apt/sources.list <<EOF
deb https://deb.debian.org/debian @SUITE@ main non-free-firmware
deb https://deb.debian.org/debian @SUITE@-updates main non-free-firmware
deb https://security.debian.org/debian-security @SUITE@-security main non-free-firmware
EOF
echo "nameserver 127.0.0.1" > /build/root/etc/resolv.conf
printf '#!/bin/sh\nexit 101\n' > /build/root/usr/sbin/policy-rc.d
chmod 755 /build/root/usr/sbin/policy-rc.d
echo ferro > /build/root/etc/hostname

echo "== packing"
# Downloaded packages and indexes are fetched again when wanted;
# documentation and translations aren't worth their space on the medium.
rm -rf /build/root/var/cache/apt/archives/*.deb /build/root/var/lib/apt/lists/* /build/root/var/log/* \
    /build/root/usr/share/doc/* /build/root/usr/share/man/* /build/root/usr/share/info/* \
    /build/root/usr/share/locale/* /build/root/usr/share/lintian/*
mksquashfs /build/root /out/debian.img -comp zstd -Xcompression-level 15 -b 256K \
    -noappend -no-progress >/tmp/mksquashfs.log 2>&1 || { tail -20 /tmp/mksquashfs.log; fail "mksquashfs"; }

echo "== adding the hash tree"
# dm-verity: a tree of hashes after the file system. FerroOS is built with
# its root, and the kernel checks every block read against it.
SIZE=$(stat -c %s /out/debian.img)
veritysetup format --hash=sha256 --data-block-size=4096 --hash-block-size=4096 \
    --data-blocks=$((SIZE / 4096)) --hash-offset="$SIZE" /out/debian.img /out/debian.img >/tmp/verity.log 2>&1 || {
    tail -5 /tmp/verity.log
    fail "veritysetup"
}
truncate -s %4096 /out/debian.img || fail "pad"
ROOT=$(sed -n 's/^Root hash:[[:space:]]*//p' /tmp/verity.log)
SALT=$(sed -n 's/^Salt:[[:space:]]*//p' /tmp/verity.log)
[ -n "$ROOT" ] && [ -n "$SALT" ] || fail "no root hash"
echo "$ROOT:$SALT:$((SIZE / 4096)):$(($(stat -c %s /out/debian.img) / 4096))" > /out/debian.verity

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/debian.img) byte debian.img (Debian $(cat /out/debian.version))"
