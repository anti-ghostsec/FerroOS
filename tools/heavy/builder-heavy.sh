#!/bin/sh
# The FerroOS Heavy image build VM's job, run by `cargo xtask heavy-iso`
# inside the minimal Debian that tools/vm/init.sh installs. Takes Debian's
# own small network-installer image and adds FerroOS Heavy's answers and
# files (heavy/) to its initrd, as a second archive the kernel unpacks over
# the first. Nothing else changes, so it still boots with Secure Boot on:
# the boot loader and kernel are Debian's signed ones. Writes the result as a
# tar stream onto the raw disk /dev/vda for the host to read.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /build /out

echo "== installing ISO tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    xorriso >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

echo "== adding FerroOS Heavy to the installer"
xorriso -osirrox on -indev /in/mini.iso -extract /initrd.gz /build/initrd.gz >/tmp/xorriso.log 2>&1 || {
    tail -5 /tmp/xorriso.log
    fail "read the installer's initrd"
}
chmod u+w /build/initrd.gz
# The kernel only looks for the next archive at a multiple of 4 bytes.
truncate -s %4 /build/initrd.gz && cat /in/heavy.cpio >> /build/initrd.gz || fail "append"
xorriso -indev /in/mini.iso -outdev /out/ferroos-heavy.iso -boot_image any replay \
    -volid FERROHEAVY -update /build/initrd.gz /initrd.gz >/tmp/xorriso.log 2>&1 || {
    tail -20 /tmp/xorriso.log
    fail "write the image"
}

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/ferroos-heavy.iso) byte ferroos-heavy.iso"
