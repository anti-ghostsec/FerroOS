#!/bin/sh
# The kernel-builder VM's job, run by `cargo xtask kernel-build` inside the
# minimal Debian that tools/vm/init.sh installs. Builds the FerroOS kernel in
# RAM and writes the result as a tar stream onto the raw disk /dev/vda for the
# host to read. @VERSION@, @SHA256@ and @HW@ are filled in by xtask.

V=@VERSION@
SHA=@SHA256@
HW=@HW@

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

echo "== installing build tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    build-essential flex bison bc perl gawk libelf-dev libssl-dev openssl \
    xz-utils zstd kmod cpio wget ca-certificates >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

mkdir -p /build
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
