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

# show_errors <log>: what went wrong, not just how the log ends: apt and
# dpkg give the reason (a missing package, a full disk) well before their
# closing summary.
show_errors() {
    grep -E -i -B2 -A4 '^E:|dpkg: error|error processing|No space left|unable to|Unable to locate|failed' "$1" | tail -60
    echo "-- end of $1:"
    tail -20 "$1"
    df -h / 2>/dev/null
}

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
    show_errors /tmp/packages.log
    fail "install packages"
}

echo "== adding our desktop and the hardening"
# Our own programs: the window manager and taskbar, the file manager, the
# Command Prompt, the Task Manager, the logon screen, the installer window.
# Static binaries; they need nothing from the system.
install -m 755 /in/bin/ferro-wm /in/bin/ferro-files /in/bin/ferro-properties /in/bin/ferro-rename /in/bin/ferro-cmd /in/bin/ferro-logon /in/bin/ferro-setup \
    /in/bin/ferro-taskmgr /in/bin/ferro-appearance /in/bin/ferro-network \
    /in/bin/ferro-volume /in/bin/ferro-datetime /in/bin/ferro-associations /in/bin/ferro-screensaver /in/bin/ferro-viewer \
    /in/bin/ferro-jail /in/bin/ferro-program-props $R/usr/local/bin/ || fail "our programs"
install -m 755 /in/heavy/ferro-install /in/heavy/ferro-heavy /in/heavy/ferro-terminal /in/heavy/ferro-torbrowser /in/heavy/ferro-inhibit /in/heavy/ferro-vm /in/heavy/ferro-uninstall \
    /in/heavy/ferro-installer /in/heavy/ferro-video /in/heavy/ferro-update $R/usr/local/bin/ || fail "tools"
mkdir -p $R/usr/local/sbin
install -m 755 /in/heavy/ferro-tor /in/heavy/ferro-switch-hardware /in/heavy/ferro-vpn /in/heavy/ferro-vm-vpn-killswitch $R/usr/local/sbin/ || fail "ferro-tor"
# Start > Help.
mkdir -p $R/usr/local/share/ferro
install -m 644 /in/heavy/help.txt $R/usr/local/share/ferro/help.txt || fail "help"
# Tor's built-in bridges (Network > Pluggable Transports).
install -m 644 /in/heavy/tor-bridges.conf $R/usr/local/share/ferro/tor-bridges.conf || fail "tor bridges"
# The Video Player (ferro-video): mpv's settings, keys and scripts. Debian's
# own mpv entry in the Start menu is replaced by one that starts it.
mkdir -p $R/usr/local/share/ferro/mpv $R/usr/local/share/applications
install -m 644 /in/heavy/mpv/* $R/usr/local/share/ferro/mpv/ || fail "video player"
printf '[Desktop Entry]\nType=Application\nName=Video Player\nComment=Plays videos, silent at first, looping\nExec=ferro-video %%F\nCategories=AudioVideo;Video;Player;\n' \
    >$R/usr/local/share/applications/mpv.desktop

mkdir -p $R/usr/share/backgrounds
cp -f /in/heavy/wallpapers/*.bmp $R/usr/share/backgrounds/ 2>/dev/null || true

echo "== installing preinstalled privacy & productivity applications"
chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin DEBIAN_FRONTEND=noninteractive \
    sh -c '
    mkdir -p /etc/apt/keyrings
    # LibreWolf left deb.librewolf.net for repo.librewolf.net (one suite for
    # every Debian release).
    if curl -fsSL https://repo.librewolf.net/pubkey.gpg -o /etc/apt/keyrings/librewolf.asc 2>/dev/null && [ -s /etc/apt/keyrings/librewolf.asc ]; then
        chmod 644 /etc/apt/keyrings/librewolf.asc
        printf "Types: deb\nURIs: https://repo.librewolf.net\nSuites: librewolf\nComponents: main\nArchitectures: amd64\nSigned-By: /etc/apt/keyrings/librewolf.asc\n" > /etc/apt/sources.list.d/librewolf.sources
    fi
    apt-get update -q &&
    apt-get install -y -q --no-install-recommends librewolf &&
    command -v librewolf
' >/tmp/apps.log 2>&1 || {
    # Never Firefox standing in under LibreWolf's name: people would get
    # Mozilla's defaults believing they had LibreWolf's.
    show_errors /tmp/apps.log
    fail "LibreWolf"
}

# Skeleton desktop shortcut for LibreWolf and Command Prompt
mkdir -p $R/etc/skel/Desktop
cat >$R/etc/skel/Desktop/LibreWolf.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=LibreWolf
Comment=LibreWolf Privacy Web Browser
Exec=librewolf %u
Icon=librewolf
Terminal=false
Categories=Network;WebBrowser;
EOF
chmod 755 $R/etc/skel/Desktop/LibreWolf.desktop

cat >$R/etc/skel/Desktop/Terminal.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Command Prompt
Comment=Command Prompt
Exec=ferro-terminal
Icon=utilities-terminal
Terminal=false
Categories=System;TerminalEmulator;
EOF
chmod 755 $R/etc/skel/Desktop/Terminal.desktop

install -m 755 /in/heavy/harden.sh $R/tmp/harden.sh
chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin /bin/sh /tmp/harden.sh >/tmp/harden.log 2>&1 || {
    tail -20 /tmp/harden.log
    fail "harden"
}
rm -f $R/tmp/harden.sh

echo "== bringing it up to date"
# harden.sh pointed apt at the security updates too: ship today's kernel
# and fixes, not the last point release's. The packages already installed
# needn't stay downloaded meanwhile: the whole build is in memory.
chroot $R /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin DEBIAN_FRONTEND=noninteractive \
    sh -c 'apt-get clean && apt-get update -q && apt-get full-upgrade -y -q && apt-get autoremove --purge -y -q && apt-get clean' >/tmp/upgrade.log 2>&1 || {
    show_errors /tmp/upgrade.log
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
if [ -f "$HOME/.Xresources" ]; then
    xrdb -merge "$HOME/.Xresources" 2>/dev/null || true
fi
xsetroot -solid '#008080' -cursor_name left_ptr
if command -v udiskie >/dev/null 2>&1; then
    udiskie-mount -a 2>/dev/null || true
    udiskie -N -T &
fi
exec /usr/local/bin/ferro-wm
EOF
chmod 755 $R/etc/X11/xinit/xinitrc

# The logon screen, on the first console instead of a text logon. (The
# other consoles, Ctrl+Alt+F2 and on, still have the text one.)
mkdir -p $R/usr/local/lib/ferro-heavy
install -m 755 /in/heavy/logon /in/heavy/logon-session /in/heavy/check-password /in/heavy/unlock-disk $R/usr/local/lib/ferro-heavy/ || fail "logon scripts"
# The duress PIN at the disk passphrase prompt (see unlock-disk).
mkdir -p $R/etc/initramfs-tools/hooks
install -m 755 /in/heavy/initramfs-duress $R/etc/initramfs-tools/hooks/ferro-duress || fail "duress hook"
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

# Tor, if it was left switched on: its firewall rules go in before the
# network comes up, so nothing gets out around it at startup either.
cat >$R/etc/systemd/system/ferro-tor.service <<'EOF'
[Unit]
Description=FerroOS Heavy's Tor switch
After=nftables.service
Before=network-pre.target
Wants=network-pre.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/ferro-tor restore

[Install]
WantedBy=multi-user.target
EOF
chroot $R systemctl enable ferro-tor.service >/dev/null 2>&1 || fail "enable ferro-tor"

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
# Mask unneeded virtual consoles to keep idle RAM minimal (tty1 has the desktop, tty2 has text console).
chroot $R systemctl mask getty@tty3.service getty@tty4.service getty@tty5.service getty@tty6.service >/dev/null 2>&1 || true

# The folders every new account starts with.
mkdir -p $R/etc/skel/Desktop $R/etc/skel/Documents $R/etc/skel/Downloads $R/etc/skel/Pictures
mkdir -p $R/etc/skel/.librewolf $R/etc/skel/.config/gtk-3.0 $R/etc/gtk-3.0
cat >$R/etc/skel/.librewolf/librewolf.overrides.cfg <<'EOF'
// FerroOS: sites behind Cloudflare's human check work with these exempt.
defaultPref("privacy.resistFingerprinting.exemptedDomains", "*.claude.ai,claude.ai,*.anthropic.com,anthropic.com,challenges.cloudflare.com");
defaultPref("browser.tabs.inTitlebar", 0);
defaultPref("browser.tabs.drawInTitlebar", false);
defaultPref("widget.gtk.titlebar-decoration", 0);

// FerroOS Memory Optimizations for LibreWolf
defaultPref("browser.tabs.unloadOnLowMemory", true);
defaultPref("browser.cache.memory.capacity", 65536);
defaultPref("browser.cache.memory.max_entry_size", 4096);
defaultPref("browser.sessionhistory.max_entries", 10);
defaultPref("browser.sessionhistory.max_total_viewers", 2);
defaultPref("dom.ipc.processCount.webIsolated", 3);
defaultPref("javascript.options.mem.gc_compacting", true);
defaultPref("media.memory_cache_max_size", 16384);
defaultPref("image.mem.surfacecache.max_size_kb", 32768);
EOF
cat >$R/etc/skel/.config/gtk-3.0/settings.ini <<'EOF'
[Settings]
gtk-decoration-layout = :
EOF
cp $R/etc/skel/.config/gtk-3.0/settings.ini $R/etc/gtk-3.0/settings.ini

mkdir -p $R/usr/local/share/applications
# The Start menu: our programs, under the names Windows gave them.
cat >$R/usr/local/share/applications/ferro-computer.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=My Computer
Exec=ferro-files "My Computer"
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
Exec=ferro-terminal
EOF
cat >$R/usr/local/share/applications/ferro-taskmgr.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Task Manager
Exec=ferro-taskmgr
EOF
cat >$R/usr/local/share/applications/ferro-appearance.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Appearance
Exec=ferro-appearance
EOF
cat >$R/usr/local/share/applications/ferro-add.desktop <<'EOF'
[Desktop Entry]
Type=Application
Name=Add Programs
Exec=xterm -T "Add Programs" -bg "#000000" -fg "#33FF33" -cr "#33FF33" -fa "DejaVu Sans Mono" -fs 10 -e ferro-heavy menu
EOF
mkdir -p $R/etc/X11/app-defaults
cat >$R/etc/X11/app-defaults/XTerm <<'EOF'
*background: #000000
*foreground: #33FF33
*cursorColor: #33FF33
*faceName: DejaVu Sans Mono
*faceSize: 10
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
    linux /live/vmlinuz boot=live quiet lockdown=integrity intel_iommu=on amd_iommu=force_isolation iommu=force iommu.passthrough=0 iommu.strict=1 efi=disable_early_pci_dma init_on_alloc=1 init_on_free=1 page_alloc.shuffle=1 transparent_hugepage=never dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512 udev.children_max=2 i915.enable_psr=0 i915.enable_fbc=0 i915.enable_dc=0 @BOOT_OPTIONS@
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
# FerroOS's own programs, scripts and settings files from this image, for
# ferro-update to bring an installed system up to date: exactly what's in
# /usr/local here, from the folders ferro-update accepts.
tar -czf /out/ferroos-heavy-update.tar.gz --owner=0 --group=0 -C $R \
    usr/local/bin usr/local/sbin usr/local/share/ferro usr/local/share/applications usr/local/lib/ferro-heavy ||
    fail "update bundle"
# The system is on the ISO now; its unpacked copy isn't needed any more.
rm -rf $R
grub-mkrescue -d /usr/lib/grub/i386-pc -o /out/ferroos-heavy.iso /iso -- -volid FERROHEAVY \
    -append_partition 2 0xef /build/efi.img \
    -boot_image any next -boot_image any efi_path=/boot/efi.img \
    >/tmp/mkrescue.log 2>&1 || { tail -20 /tmp/mkrescue.log; fail "grub-mkrescue"; }

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/ferroos-heavy.iso) byte ferroos-heavy.iso"
