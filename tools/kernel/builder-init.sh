#!/bin/sh
# /init of the kernel-builder VM (a minimal Linux userland and kernel), run
# by `cargo xtask kernel-build`. Builds the FerroOS kernel in RAM and writes
# the result as a tar stream onto the raw disk /dev/vda for the host to read.
# @VERSION@, @SHA256@ and @HW@ are filled in by xtask.

export PATH=/usr/sbin:/usr/bin:/sbin:/bin
V=@VERSION@
SHA=@SHA256@
HW=@HW@

fail() { echo "FERRO-BUILD-FAILED: $*"; sync; poweroff -f; }

mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
mkdir -p /dev/pts /build && mount -t devpts devpts /dev/pts
mount -t tmpfs -o size=95% tmpfs /build || fail "tmpfs"
for m in failover net_failover virtio_net virtio_blk; do
    insmod /builder/modules/$m.ko || fail "insmod $m"
done

# QEMU user networking is fixed: guest 10.0.2.15, gateway .2, DNS .3.
ip link set lo up
ip link set eth0 up
ip addr add 10.0.2.15/24 dev eth0
ip route add default via 10.0.2.2
echo "nameserver 10.0.2.3" > /etc/resolv.conf

echo "== installing build tools"
apk add --no-cache -q build-base flex bison bc perl elfutils-dev openssl openssl-dev \
    linux-headers xz zstd tar kmod diffutils findutils >/dev/null || fail "apk add"

cd /build || fail "cd"
echo "== downloading linux-$V"
wget -q "https://cdn.kernel.org/pub/linux/kernel/v${V%%.*}.x/linux-$V.tar.xz" || fail "download"
echo "$SHA  linux-$V.tar.xz" | sha256sum -c - || fail "checksum mismatch"
echo "== unpacking"
tar -xJf "linux-$V.tar.xz" --exclude="linux-$V/tools/testing" || fail "unpack"
# Free RAM: other architectures and Documentation are only needed for their
# Kconfig and Makefile fragments (crypto/Kconfig sources every arch's;
# fs/hostfs includes arch/um's make rules), so keep just those.
for d in "linux-$V"/arch/* "linux-$V/Documentation"; do
    [ "$d" = "linux-$V/arch/x86" ] && continue
    [ -d "$d" ] && find "$d" -type f ! -name 'Kconfig*' ! -name 'Makefile*' -delete
done
rm "linux-$V.tar.xz"
cd "linux-$V" || fail "cd src"

# FerroOS doesn't enable AMD memory encryption, so the (empty) decrypted-BSS
# section needs no 2 MB alignment. Left in, it pads the kernel's BSS up to the
# next 2 MB boundary: up to 2 MB of RAM wasted, depending on where BSS ends.
sed -i '/#define BSS_DECRYPTED/,/__pi___end_bss_decrypted/ s/ALIGN(PMD_SIZE)/ALIGN(PAGE_SIZE)/' arch/x86/kernel/vmlinux.lds.S || fail "patch"

echo "== configuring"
make ARCH=x86_64 tinyconfig >/dev/null || fail "tinyconfig"
FRAGS=/builder/ferro.config
[ "$HW" = 1 ] && FRAGS="$FRAGS /builder/ferro-hw.config"
./scripts/kconfig/merge_config.sh -m .config $FRAGS >/dev/null || fail "merge_config"
make ARCH=x86_64 olddefconfig >/dev/null || fail "olddefconfig"
for f in $FRAGS; do
    grep -E '^CONFIG_[A-Z0-9_]+=' "$f" | while read -r line; do
        grep -qx "$line" .config || echo "NOT APPLIED: $line (dependency missing or renamed)"
    done
done

echo "== building with $(nproc) CPUs"
START=$(date +%s)
make ARCH=x86_64 -j"$(nproc)" bzImage modules >/build/make.log 2>&1 || {
    # Parallel builds bury the cause; show the actual error lines.
    grep -nE "error:|Error [0-9]|No rule|No such file|undefined reference" /build/make.log | head -30
    fail "make"
}
echo "== built in $(( $(date +%s) - START ))s"

mkdir -p /out
cp arch/x86/boot/bzImage .config System.map /out/ || fail "copy"
make ARCH=x86_64 modules_install INSTALL_MOD_PATH=/out INSTALL_MOD_STRIP=1 >/dev/null || fail "modules_install"
echo "$V" > /out/VERSION
tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/bzImage) byte bzImage"
poweroff -f
