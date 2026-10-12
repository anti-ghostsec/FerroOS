#!/bin/sh
# The FerroOS Heavy image build VM's job, run by `cargo xtask heavy-iso`
# inside the minimal Debian that tools/vm/init.sh installs. Builds the system
# FerroOS Heavy ships: Debian's smallest base, the packages in
# heavy/packages.txt, our desktop (ferro-wm) and the hardening
# (heavy/harden.sh). Packs it into an image that runs straight from the
# installation medium and that `ferro-install` copies to a disk, with no
# network. The boot loader and kernel are Debian's signed ones, so it starts
# with Secure Boot on. Writes the ISO as a tar stream onto /dev/vda.
# @SUITE@, @PACKAGES@ and @BOOT_OPTIONS@ are filled in by xtask.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

R=/build/root
mkdir -p /build /out /iso/live /iso/boot/grub /iso/.disk

echo "== installing build tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    debootstrap debian-archive-keyring mount squashfs-tools xorriso mtools grub-common grub-pc-bin >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

echo "== installing the base system"
debootstrap --variant=minbase --arch=amd64 --force-check-gpg \
    --components=main,contrib,non-free,non-free-firmware @SUITE@ $R \
    http://deb.debian.org/debian >/tmp/debootstrap.log 2>&1 || {
    tail -20 /tmp/debootstrap.log
    fail "debootstrap"
}

echo "== installing FerroOS Heavy's packages"
mount -t proc proc $R/proc && mount -t sysfs sys $R/sys && mount -t devtmpfs dev $R/dev || fail "mount"
mkdir -p $R/dev/pts && mount -t devpts devpts $R/dev/pts
# Nothing may start services inside the image while it's being built.
printf '#!/bin/sh\nexit 101\n' >$R/usr/sbin/policy-rc.d && chmod 755 $R/usr/sbin/policy-rc.d
cp /etc/resolv.conf $R/etc/resolv.conf
chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin DEBIAN_FRONTEND=noninteractive \
    apt-get install -y -q --no-install-recommends @PACKAGES@ >/tmp/packages.log 2>&1 || {
    tail -30 /tmp/packages.log
    fail "install packages"
}

echo "== adding our desktop and the hardening"
# Our own programs: the window manager and taskbar, the file manager, the
# Command Prompt, the Task Manager, the logon screen, the installer window.
# Static binaries; they need nothing from the system.
install -m 755 /in/bin/ferro-wm /in/bin/ferro-files /in/bin/ferro-cmd /in/bin/ferro-logon /in/bin/ferro-setup \
    /in/bin/ferro-taskmgr $R/usr/local/bin/ || fail "our programs"
install -m 755 /in/heavy/ferro-install /in/heavy/ferro-heavy $R/usr/local/bin/ || fail "tools"
install -m 755 /in/heavy/harden.sh $R/tmp/harden.sh
chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/sh /tmp/harden.sh >/tmp/harden.log 2>&1 || {
    tail -20 /tmp/harden.log
    fail "harden"
}
rm -f $R/tmp/harden.sh

echo "== bringing it up to date"
# harden.sh pointed apt at the security updates too: ship today's kernel
# and fixes, not the last point release's.
chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin DEBIAN_FRONTEND=noninteractive \
    sh -c 'apt-get update -q && apt-get full-upgrade -y -q && apt-get autoremove --purge -y -q' >/tmp/upgrade.log 2>&1 || {
    tail -20 /tmp/upgrade.log
    fail "upgrade"
}
# One kernel on the medium: the newest. (apt keeps the one before, too.)
NEWEST=$(chroot $R dpkg-query -W -f='${Depends}' linux-image-amd64 | grep -o 'linux-image-[^ ,]*' | head -n1)
for old in $(chroot $R dpkg-query -W -f='${Package}\n' 'linux-image-[0-9]*' 2>/dev/null); do
    [ "$old" = "$NEWEST" ] || chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin DEBIAN_FRONTEND=noninteractive \
        apt-get purge -y -q "$old" >>/tmp/upgrade.log 2>&1 || fail "remove $old"
done

# The desktop: what the logon screen starts for whoever logs on.
cat >$R/etc/X11/xinit/xinitrc <<'EOF'
#!/bin/sh
# FerroOS Heavy's desktop: our window manager, taskbar and Start menu.
xsetroot -solid '#008080' -cursor_name left_ptr
exec /usr/local/bin/ferro-wm
EOF
chmod 755 $R/etc/X11/xinit/xinitrc

# The logon screen, on the first console instead of a text logon. (The
# other consoles, Ctrl+Alt+F2 and on, still have the text one.)
mkdir -p $R/usr/local/lib/ferro-heavy
install -m 755 /in/heavy/logon /in/heavy/logon-session /in/heavy/check-password $R/usr/local/lib/ferro-heavy/ || fail "logon scripts"
cat >$R/etc/systemd/system/ferro-logon.service <<'EOF'
[Unit]
Description=FerroOS Heavy's logon screen
After=systemd-user-sessions.service systemd-logind.service ferro-live.service getty@tty1.service
Conflicts=getty@tty1.service

[Service]
ExecStart=/usr/local/lib/ferro-heavy/logon
Restart=always
RestartSec=1

[Install]
WantedBy=multi-user.target
EOF
chroot $R systemctl enable ferro-logon.service >/dev/null 2>&1 || fail "enable ferro-logon"

# Running from the installation medium: an account that needs no password
# (there's nothing to protect yet). The logon screen lets it straight in.
mkdir -p $R/usr/local/lib/ferro-heavy
cat >$R/usr/local/lib/ferro-heavy/live-setup <<'EOF'
#!/bin/sh
# FerroOS Heavy started from its installation medium, not from a disk.
useradd -m -s /bin/bash -G sudo,video,render,audio,input live 2>/dev/null
passwd -d live >/dev/null
echo 'live ALL=(ALL) NOPASSWD: ALL' >/etc/sudoers.d/live
chmod 440 /etc/sudoers.d/live
autologin() { # autologin <unit> <agetty arguments>
    mkdir -p "/run/systemd/system/$1.d"
    printf '[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin live %s\n' "$2" >"/run/systemd/system/$1.d/live.conf"
}
# Test images are driven over the serial line.
if grep -qw ferro.test /proc/cmdline; then
    autologin serial-getty@ttyS0.service '--keep-baud 115200,57600,38400,9600 %I vt220'
fi
systemctl daemon-reload
EOF
chmod 755 $R/usr/local/lib/ferro-heavy/live-setup
cat >$R/etc/systemd/system/ferro-live.service <<'EOF'
[Unit]
Description=FerroOS Heavy running from its installation medium
ConditionKernelCommandLine=boot=live
Before=ferro-logon.service serial-getty@ttyS0.service systemd-user-sessions.service

[Service]
Type=oneshot
ExecStart=/usr/local/lib/ferro-heavy/live-setup

[Install]
WantedBy=multi-user.target
EOF
chroot $R systemctl enable ferro-live.service >/dev/null 2>&1 || fail "enable ferro-live"

# The folders every new account starts with.
mkdir -p $R/etc/skel/Desktop $R/etc/skel/Documents $R/etc/skel/Downloads $R/etc/skel/Pictures

mkdir -p $R/usr/local/share/applications
# The Start menu: our programs, under the names Windows gave them.
cat >$R/usr/local/share/applications/ferro-computer.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=My Computer
Exec=ferro-files /
EOF
cat >$R/usr/local/share/applications/ferro-documents.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=My Documents
Exec=ferro-files
EOF
cat >$R/usr/local/share/applications/ferro-cmd.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Command Prompt
Exec=xterm -T "Command Prompt" -bg black -fg gray80 -e /usr/local/bin/ferro-cmd
EOF
cat >$R/usr/local/share/applications/ferro-taskmgr.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Task Manager
Exec=ferro-taskmgr
EOF
cat >$R/usr/local/share/applications/ferro-add.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Add Programs
Exec=xterm -T "Add Programs" -bg black -fg gray80 -e ferro-heavy menu
EOF
# The terminal's own two entries would only repeat that one.
for entry in debian-xterm debian-uxterm; do
    cat >$R/usr/local/share/applications/$entry.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Terminal
Exec=xterm
NoDisplay=true
EOF
done
cat >$R/usr/local/share/applications/ferro-install.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Install FerroOS Heavy
Exec=ferro-setup
EOF

cat >$R/etc/os-release <<'EOF'
PRETTY_NAME="FerroOS Heavy"
NAME="FerroOS Heavy"
ID=ferroos-heavy
ID_LIKE=debian
HOME_URL="https://github.com/anti-ghostsec/FerroOS"
EOF
echo "FerroOS Heavy \n \l" >$R/etc/issue

echo "== packing"
rm -f $R/usr/sbin/policy-rc.d $R/etc/resolv.conf
chroot $R apt-get clean
rm -rf $R/var/lib/apt/lists/* $R/var/log/*.log $R/var/cache/debconf/*-old \
    $R/usr/share/doc/* $R/usr/share/man/* $R/usr/share/info/* $R/usr/share/lintian/*
find $R/usr/share/locale -mindepth 1 -maxdepth 1 ! -name 'en*' -exec rm -rf {} + 2>/dev/null
# Every installation makes its own.
: >$R/etc/machine-id
ln -sf /run/systemd/resolve/stub-resolv.conf $R/etc/resolv.conf
umount $R/dev/pts $R/dev $R/sys $R/proc || fail "unmount"

KERNEL=$(ls $R/boot/vmlinuz-* | sort | tail -n1)
INITRD=$(ls $R/boot/initrd.img-* | sort | tail -n1)
[ -f "$KERNEL" ] && [ -f "$INITRD" ] || fail "no kernel in the image"
cp "$KERNEL" /iso/live/vmlinuz && cp "$INITRD" /iso/live/initrd.img || fail "copy kernel"
mksquashfs $R /iso/live/filesystem.squashfs -comp zstd -Xcompression-level 15 -b 256K \
    -noappend -no-progress >/tmp/mksquashfs.log 2>&1 || { tail -20 /tmp/mksquashfs.log; fail "mksquashfs"; }

echo "== making the ISO"
echo "FerroOS Heavy $(cat $R/etc/debian_version)" >/iso/.disk/info
cat >/iso/boot/grub/grub.cfg <<'EOF'
set timeout=5
set default=0
menuentry "FerroOS Heavy (try it, or install from its Start menu)" {
    linux /live/vmlinuz boot=live quiet lockdown=integrity @BOOT_OPTIONS@
    initrd /live/initrd.img
}
EOF
# What UEFI firmware starts: Debian's shim (signed by Microsoft) and its
# signed boot loader for installation media, which reads the menu above.
SHIM=$R/usr/lib/shim/shimx64.efi.signed
GRUB=$R/usr/lib/grub/x86_64-efi-signed/gcdx64.efi.signed
MM=$R/usr/lib/shim/mmx64.efi.signed
[ -f "$SHIM" ] && [ -f "$GRUB" ] && [ -f "$MM" ] || fail "signed boot files not found"
dd if=/dev/zero of=/build/efi.img bs=1M count=8 2>/dev/null || fail "efi image"
mformat -i /build/efi.img -v FERROHEAVY :: || fail "format efi image"
mmd -i /build/efi.img ::/EFI ::/EFI/BOOT || fail "efi folders"
mcopy -i /build/efi.img "$SHIM" ::/EFI/BOOT/BOOTX64.EFI &&
    mcopy -i /build/efi.img "$GRUB" ::/EFI/BOOT/grubx64.efi &&
    mcopy -i /build/efi.img "$MM" ::/EFI/BOOT/mmx64.efi || fail "fill efi image"
cp /build/efi.img /iso/boot/efi.img
# The system is on the ISO now; its unpacked copy isn't needed any more.
rm -rf $R
grub-mkrescue -d /usr/lib/grub/i386-pc -o /out/ferroos-heavy.iso /iso -- -volid FERROHEAVY \
    -append_partition 2 0xef /build/efi.img \
    -boot_image any next -boot_image any efi_path=/boot/efi.img \
    >/tmp/mkrescue.log 2>&1 || { tail -20 /tmp/mkrescue.log; fail "grub-mkrescue"; }

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/ferroos-heavy.iso) byte ferroos-heavy.iso"
