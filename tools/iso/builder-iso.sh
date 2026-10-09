#!/bin/sh
# /init of the ISO build VM, run by `cargo xtask iso`. Makes a hybrid ISO
# (BIOS and UEFI; DVD or USB stick) with a GRUB boot menu, plus the
# installation payload as an extra partition, and writes it as a tar stream
# onto /dev/vda for the host to read.

export PATH=/usr/sbin:/usr/bin:/sbin:/bin

fail() { echo "FERRO-BUILD-FAILED: $*"; sync; poweroff -f; }

mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
mkdir -p /out /iso/boot/grub
for m in failover net_failover virtio_net virtio_blk; do
    insmod /builder/modules/$m.ko || fail "insmod $m"
done

ip link set lo up
ip link set eth0 up
ip addr add 10.0.2.15/24 dev eth0
ip route add default via 10.0.2.2
echo "nameserver 10.0.2.3" > /etc/resolv.conf

echo "== installing ISO tools"
apk add --no-cache -q grub grub-bios grub-efi xorriso mtools >/dev/null || fail "apk add"

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
# Partition 3: the installation payload (kernel, initramfs, boot loader,
# Tor), which FerroOS finds by its signature.
grub-mkrescue -o /out/ferroos.iso /iso -- -volid FERROOS -append_partition 3 0x83 /in/payload.img \
    >/tmp/mkrescue.log 2>&1 || { tail -20 /tmp/mkrescue.log; fail "grub-mkrescue"; }
tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/ferroos.iso) byte ferroos.iso"
poweroff -f
