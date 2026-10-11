#!/bin/sh
# The ISO build VM's job, run by `cargo xtask iso` inside the minimal Debian
# that tools/vm/init.sh installs. Makes a hybrid ISO (BIOS and UEFI; DVD or
# USB stick), plus the installation payload as an extra partition, and writes
# it as a tar stream onto /dev/vda for the host to read. BIOS machines get a
# GRUB menu; UEFI ones start the signed FerroOS through shim (tools/boot), so
# Secure Boot can stay on.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /out /iso/boot/grub

echo "== installing ISO tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    grub-common grub-pc-bin xorriso mtools >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

cp /in/bzImage /in/initramfs.cpio /iso/boot/ || fail "copy"
cat > /iso/boot/grub/grub.cfg <<'EOF'
# FerroOS installation media. The *hash_entries keep the kernel's lookup
# tables at desktop sizes instead of growing them with RAM.
set timeout=10
set default=0
serial --unit=0 --speed=115200
terminal_input console serial
terminal_output console serial
set color_normal=white/blue
set menu_color_normal=white/blue
set menu_color_highlight=blue/white
insmod all_video
set gfxpayload=keep

menuentry "Try FerroOS (nothing is saved)" {
    linux /boot/bzImage ferro.live quiet loglevel=3 console=ttyS0 dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512
    initrd /boot/initramfs.cpio
}
menuentry "Install FerroOS" {
    linux /boot/bzImage ferro.live ferro.install quiet loglevel=3 console=ttyS0 dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512
    initrd /boot/initramfs.cpio
}
EOF

echo "== making the ISO"
# Partition 2: what UEFI firmware starts. shim looks for its next step under
# the name grubx64.efi; the key manager finds the key to approve beside it.
SIZE=$(( ($(cat /in/boot/*.efi | wc -c) >> 20) + 4 ))
dd if=/dev/zero of=/build-efi.img bs=1M count="$SIZE" 2>/dev/null || fail "efi image"
mformat -i /build-efi.img -v FERROOS :: || fail "format efi image"
mmd -i /build-efi.img ::/EFI ::/EFI/BOOT || fail "efi folders"
mcopy -i /build-efi.img /in/boot/shimx64.efi ::/EFI/BOOT/BOOTX64.EFI &&
    mcopy -i /build-efi.img /in/boot/grubx64.efi /in/boot/mmx64.efi ::/EFI/BOOT/ &&
    mcopy -i /build-efi.img /in/boot/ferro-mok.der ::/ || fail "fill efi image"
# The same image twice: as a partition for USB sticks, as a file for discs.
cp /build-efi.img /iso/boot/efi.img || fail "copy efi image"
# Partition 3: the installation payload (kernel, initramfs, boot loader,
# Tor), which FerroOS finds by its signature. Partition 4: the Debian layer,
# when there is one. With both on the medium, installing needs no network.
LAYER=
[ -f /in/debian.img ] && LAYER="-append_partition 4 0x83 /in/debian.img"
grub-mkrescue -d /usr/lib/grub/i386-pc -o /out/ferroos.iso /iso -- -volid FERROOS \
    -append_partition 2 0xef /build-efi.img \
    -boot_image any next -boot_image any efi_path=/boot/efi.img \
    -append_partition 3 0x83 /in/payload.img $LAYER \
    >/tmp/mkrescue.log 2>&1 || { tail -20 /tmp/mkrescue.log; fail "grub-mkrescue"; }
tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/ferroos.iso) byte ferroos.iso"
