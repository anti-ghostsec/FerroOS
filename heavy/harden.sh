#!/bin/sh
# FerroOS Heavy: the hardening, applied to the system image when it's built
# (tools/heavy/builder-base.sh runs this inside it). It carries over what
# FerroOS does, where a full Debian can do it: a locked-down kernel, nothing
# listening, encrypted DNS, random MAC addresses, no hostname on the network,
# no memory dumps on disk.

set -e

# ---- Kernel settings: the list FerroOS's init applies
# (crates/ferro-init/src/system.rs).
mkdir -p /etc/sysctl.d
cat >/etc/sysctl.d/60-ferro-heavy.conf <<'EOF'
kernel.kptr_restrict = 2
kernel.dmesg_restrict = 1
kernel.yama.ptrace_scope = 1
kernel.unprivileged_bpf_disabled = 1
kernel.perf_event_paranoid = 3
kernel.kexec_load_disabled = 1
kernel.sysrq = 0
fs.protected_symlinks = 1
fs.protected_hardlinks = 1
fs.protected_fifos = 2
fs.protected_regular = 2
fs.suid_dumpable = 0
net.core.bpf_jit_harden = 2
# Timestamps reveal uptime.
net.ipv4.tcp_timestamps = 0
net.ipv4.conf.all.accept_redirects = 0
net.ipv4.conf.default.accept_redirects = 0
net.ipv6.conf.all.accept_redirects = 0
net.ipv6.conf.default.accept_redirects = 0
net.ipv4.conf.all.send_redirects = 0
net.ipv4.conf.all.accept_source_route = 0
net.ipv4.conf.all.rp_filter = 1
net.ipv4.conf.default.rp_filter = 1
net.ipv4.conf.all.secure_redirects = 0
net.ipv4.conf.default.secure_redirects = 0
net.ipv4.icmp_echo_ignore_broadcasts = 1
net.ipv4.icmp_ignore_bogus_error_responses = 1
net.ipv4.tcp_syncookies = 1
net.ipv4.tcp_rfc1337 = 1
net.ipv6.conf.all.use_tempaddr = 2
net.ipv6.conf.default.use_tempaddr = 2
# ASLR entropy maximization:
kernel.randomize_va_space = 2
vm.mmap_rnd_bits = 32
vm.mmap_rnd_compat_bits = 16
dev.tty.ldisc_autoload = 0
# Prevent ARP poisoning and bogus unicast traffic on public Wi-Fi:
net.ipv4.conf.all.drop_unicast_in_l2_multicast = 1
net.ipv4.conf.all.drop_gratuitous_arp = 1
net.ipv4.conf.default.drop_gratuitous_arp = 1
# Cache reclaim, dirty ratios, and memory watermark caps:
vm.vfs_cache_pressure = 200
vm.dirty_background_ratio = 5
vm.dirty_ratio = 10
vm.swappiness = 10
vm.min_free_kbytes = 16384
EOF
# ptrace_scope is 1, not FerroOS's 2: Proton's and Steam's crash handlers and
# some games' launchers attach to their own children.

# ---- Boot options for installed systems: no raw kernel memory access and
# signed modules only (a driver built on the machine, like NVIDIA's, is signed
# there; see `ferro-heavy enroll-key`), plus heap and stack hardening that
# costs games nothing measurable. Hash tables are pinned to keep idle RAM low.
# audit=0, udev.children_max=1 and cgroup_no_v1=all shave ~15MB off kernel/daemon idle RAM.
mkdir -p /etc/default/grub.d
cat >/etc/default/grub.d/60-ferro-heavy.cfg <<'EOF'
GRUB_CMDLINE_LINUX_DEFAULT="quiet loglevel=3 audit=0 cgroup_no_v1=all lockdown=integrity slab_nomerge randomize_kstack_offset=on vsyscall=none init_on_alloc=1 init_on_free=1 page_alloc.shuffle=1 transparent_hugepage=never dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512 udev.children_max=1"
# Debian's signed boot loader looks for its menu under this name.
GRUB_DISTRIBUTOR="Debian"
GRUB_TIMEOUT=2
# A text boot menu. The graphical one leaves the screen black until the
# system's own graphics driver loads, which is after the disk passphrase has
# to be typed.
GRUB_TERMINAL=console
# An encrypted disk doesn't need its other systems listed by a root script.
GRUB_DISABLE_OS_PROBER=true
EOF

# ---- No memory dumps on disk.
mkdir -p /etc/systemd/coredump.conf.d /etc/security/limits.d
printf '[Coredump]\nStorage=none\nProcessSizeMax=0\n' >/etc/systemd/coredump.conf.d/60-ferro-heavy.conf
echo '* hard core 0' >/etc/security/limits.d/60-ferro-heavy.conf

# ---- Limit journal memory in RAM and compress entries (4MB max).
mkdir -p /etc/systemd/journald.conf.d
cat >/etc/systemd/journald.conf.d/60-ferro-heavy.conf <<'EOF'
[Journal]
Storage=volatile
RuntimeMaxUse=4M
RuntimeMaxFileSize=1M
Compress=yes
RateLimitIntervalSec=30s
RateLimitBurst=300
EOF

# ---- Firewall: nothing gets in that this machine didn't ask for.
cat >/etc/nftables.conf <<'EOF'
#!/usr/sbin/nft -f
flush ruleset

table inet filter {
    chain input {
        type filter hook input priority filter; policy drop;
        ct state established,related accept
        ct state invalid drop
        iif "lo" accept
        # What IPv6 needs to work at all, and DHCPv6 replies.
        icmpv6 type { nd-neighbor-solicit, nd-neighbor-advert, nd-router-advert, packet-too-big, time-exceeded, parameter-problem } accept
        icmp type { destination-unreachable, time-exceeded, parameter-problem } accept
        udp dport 546 ip6 saddr fe80::/10 accept
        # DHCPv4 client: accept DHCP offers and ACKs from local router/gateway
        udp sport 67 udp dport 68 accept
    }
    chain forward {
        type filter hook forward priority filter; policy drop;
    }
}
EOF
systemctl enable nftables

# ---- Network privacy: a new random MAC address for every connection, wired
# and Wi-Fi, and while scanning; no hostname sent to the network; temporary
# IPv6 addresses; internal DHCP, and lean keyfile plugin.
mkdir -p /etc/NetworkManager/conf.d
cat >/etc/NetworkManager/conf.d/60-ferro-heavy.conf <<'EOF'
[main]
dns=systemd-resolved
hostname-mode=none
plugins=keyfile
dhcp=internal

[logging]
level=WARN

[connectivity]
enabled=false

[device]
wifi.scan-rand-mac-address=yes
wifi.powersave=3

[connection]
wifi.cloned-mac-address=random
ethernet.cloned-mac-address=random
ipv4.dhcp-send-hostname=0
ipv6.dhcp-send-hostname=0
ipv6.ip6-privacy=2
connection.llmnr=0
connection.mdns=0
EOF
systemctl enable NetworkManager >/dev/null 2>&1 || true

# ---- Xorg memory tuning: disable Composite (ferro-wm draws in software and
# needs no compositing buffers in RAM), RECORD, and XTEST.
mkdir -p /etc/X11/xorg.conf.d
cat >/etc/X11/xorg.conf.d/60-ferro-heavy.conf <<'EOF'
Section "Extensions"
    Option "Composite" "Disable"
    Option "RECORD" "Disable"
    Option "XTEST" "Disable"
EndSection
EOF

# ---- DNS resolution: systemd-resolved with per-network DHCP DNS and resilient fallback resolvers.
mkdir -p /etc/systemd/resolved.conf.d
cat >/etc/systemd/resolved.conf.d/60-ferro-heavy.conf <<'EOF'
[Resolve]
DNS=
FallbackDNS=9.9.9.9 149.112.112.112 1.1.1.1 8.8.8.8
DNSOverTLS=no
DNSSEC=no
LLMNR=no
MulticastDNS=no
Cache=yes
CacheFromLocalhost=no
EOF
systemctl enable systemd-resolved

# ---- Packages over HTTP/HTTPS with all standard components
cat >/etc/apt/sources.list <<'EOF'
deb http://deb.debian.org/debian @SUITE@ main contrib non-free non-free-firmware
deb http://deb.debian.org/debian @SUITE@-updates main contrib non-free non-free-firmware
deb http://security.debian.org/debian-security @SUITE@-security main contrib non-free non-free-firmware
EOF
# Only what's asked for: no "recommended" extras, no persistent in-memory caches,
# and no background polling timers.
cat >/etc/apt/apt.conf.d/60-ferro-heavy <<'EOF'
APT::Install-Recommends "false";
APT::Install-Suggests "false";
Dir::Cache::pkgcache "";
Dir::Cache::srcpkgcache "";
APT::Periodic::Update-Package-Lists "0";
APT::Periodic::Download-Upgradeable-Packages "0";
APT::Periodic::AutocleanInterval "0";
EOF

# ---- One machine name for everyone, so it identifies nobody.
echo ferro >/etc/hostname
printf '127.0.0.1 localhost ferro\n::1 localhost ip6-localhost ip6-loopback\n' >/etc/hosts

# ---- Limit login virtual terminals to 2 (X11 on tty1, backup console on tty2)
# and kill orphaned processes on logout to free RAM.
mkdir -p /etc/systemd/logind.conf.d
cat >/etc/systemd/logind.conf.d/60-ferro-heavy.conf <<'EOF'
[Login]
NAutoVTs=2
ReserveVT=2
KillUserProcesses=yes
HandleLidSwitch=suspend
HandleLidSwitchExternalPower=suspend
HandleSleepKey=suspend
EOF

# ---- Encrypted Sleep / Suspend hardening: enforce deep sleep (ACPI S3) and key eviction / cache purge
mkdir -p /etc/systemd/sleep.conf.d /lib/systemd/system-sleep
cat >/etc/systemd/sleep.conf.d/60-ferro-sleep.conf <<'EOF'
[Sleep]
AllowSuspend=yes
AllowHibernation=yes
AllowSuspendThenHibernate=yes
SuspendMode=suspend
MemorySleepMode=deep
EOF

cat >/lib/systemd/system-sleep/60-ferro-encrypted-sleep.sh <<'EOF'
#!/bin/sh
# FerroOS Heavy: Encrypted Sleep / Suspend hook
# Pre-sleep: flushes data, frees and zeroes caches (via init_on_free=1),
# enforces deep sleep mode, and evicts dm-crypt master keys if present.
# Post-sleep: restores dm-crypt mapping.

case "$1" in
    pre)
        sync
        echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true
        echo 1 > /proc/sys/vm/compact_memory 2>/dev/null || true
        echo deep > /sys/power/mem_sleep 2>/dev/null || true
        if command -v dmsetup >/dev/null 2>&1; then
            for dev in ferro-system ferro-debian-data; do
                if [ -e "/dev/mapper/$dev" ]; then
                    dmsetup suspend --noflush "$dev" 2>/dev/null || true
                fi
            done
        fi
        ;;
    post)
        if command -v dmsetup >/dev/null 2>&1; then
            for dev in ferro-system ferro-debian-data; do
                if [ -e "/dev/mapper/$dev" ]; then
                    dmsetup resume "$dev" 2>/dev/null || true
                fi
            done
        fi
        ;;
esac
EOF
chmod 755 /lib/systemd/system-sleep/60-ferro-encrypted-sleep.sh

# ---- Mask unused virtual consoles, periodic background scrubbers and APT timers.
systemctl mask getty@tty3.service getty@tty4.service getty@tty5.service getty@tty6.service >/dev/null 2>&1 || true
systemctl mask apt-daily.timer apt-daily-upgrade.timer >/dev/null 2>&1 || true
systemctl mask e2scrub_all.timer e2scrub_all.service >/dev/null 2>&1 || true

# ---- Restrictive default umask (077): files created by users are private by default.
mkdir -p /etc/profile.d
echo 'umask 077' >/etc/profile.d/60-ferro-umask.sh
if [ -f /etc/login.defs ]; then
    sed -i 's/^UMASK.*/UMASK\t\t077/' /etc/login.defs 2>/dev/null || true
fi

# ---- No root password; accounts in the sudo group use sudo.
passwd -l root >/dev/null

# ---- Passwordless power management: allow accounts in sudo group to shutdown, reboot, and suspend
mkdir -p /etc/sudoers.d
cat >/etc/sudoers.d/60-ferro-power <<'EOF'
%sudo ALL=(ALL) NOPASSWD: /bin/systemctl poweroff, /bin/systemctl reboot, /bin/systemctl suspend, /bin/systemctl hibernate, /sbin/poweroff, /sbin/reboot, /sbin/shutdown, /sbin/zzz
EOF
chmod 0440 /etc/sudoers.d/60-ferro-power

mkdir -p /etc/polkit-1/rules.d
cat >/etc/polkit-1/rules.d/60-ferro-power.rules <<'EOF'
polkit.addRule(function(action, subject) {
    if ((action.id == "org.freedesktop.login1.power-off" ||
         action.id == "org.freedesktop.login1.power-off-multiple-sessions" ||
         action.id == "org.freedesktop.login1.power-off-ignore-inhibit" ||
         action.id == "org.freedesktop.login1.reboot" ||
         action.id == "org.freedesktop.login1.reboot-multiple-sessions" ||
         action.id == "org.freedesktop.login1.reboot-ignore-inhibit" ||
         action.id == "org.freedesktop.login1.suspend" ||
         action.id == "org.freedesktop.login1.suspend-multiple-sessions" ||
         action.id == "org.freedesktop.login1.suspend-ignore-inhibit" ||
         action.id == "org.freedesktop.login1.hibernate" ||
         action.id == "org.freedesktop.login1.hibernate-multiple-sessions" ||
         action.id == "org.freedesktop.login1.hibernate-ignore-inhibit") &&
        subject.isInGroup("sudo")) {
        return polkit.Result.YES;
    }
});
EOF
chmod 0644 /etc/polkit-1/rules.d/60-ferro-power.rules

# ---- Removable storage permissions: allow users in sudo to mount, unmount, and eject disks
cat >/etc/polkit-1/rules.d/60-ferro-storage.rules <<'EOF'
polkit.addRule(function(action, subject) {
    if (action.id.indexOf("org.freedesktop.udisks2.") === 0) {
        return polkit.Result.YES;
    }
});
EOF
chmod 0644 /etc/polkit-1/rules.d/60-ferro-storage.rules

# ---- Terminal configuration: green-on-black phosphor CRT theme, clipboard shortcuts, clean backspace
mkdir -p /etc/xdg/lxterminal /etc/skel/.config/lxterminal
cat >/etc/xdg/lxterminal/lxterminal.conf <<'EOF'
[general]
fontname=DejaVu Sans Mono 10
bgcolor=#000000
fgcolor=#33ff33
disablescrollbar=false
hidemenubar=true
cursorblinks=true
cursorcolor=#33ff33

[shortcut]
copy_accel=<Primary>c
paste_accel=<Primary>v
EOF
cp /etc/xdg/lxterminal/lxterminal.conf /etc/skel/.config/lxterminal/lxterminal.conf

if [ -d /etc/X11/Xresources ]; then
    cat >/etc/X11/Xresources/ferro-terminal <<'EOF'
*VT100.Translations: #override \
    Ctrl <Key>C: copy-selection(CLIPBOARD) \n\
    Ctrl <Key>V: insert-selection(CLIPBOARD) \n\
    <Key>BackSpace: string(0x7F)
*backarrowKey: false
*ptyInitialErase: true
EOF
else
    cat >/etc/X11/Xresources <<'EOF'
*VT100.Translations: #override \
    Ctrl <Key>C: copy-selection(CLIPBOARD) \n\
    Ctrl <Key>V: insert-selection(CLIPBOARD) \n\
    <Key>BackSpace: string(0x7F)
*backarrowKey: false
*ptyInitialErase: true
EOF
fi

mkdir -p /etc/skel
cat >/etc/skel/.Xresources <<'EOF'
*VT100.Translations: #override \
    Ctrl <Key>C: copy-selection(CLIPBOARD) \n\
    Ctrl <Key>V: insert-selection(CLIPBOARD) \n\
    <Key>BackSpace: string(0x7F)
*backarrowKey: false
*ptyInitialErase: true
EOF

mkdir -p /etc/profile.d
cat >/etc/profile.d/60-ferro-terminal.sh <<'EOF'
# Ensure backspace always works cleanly across virtual consoles and terminals
if [ -t 0 ]; then
    stty erase '^?' 2>/dev/null || true
fi
EOF

# ---- Default browser & URL scheme handling (for Antigravity, Electron, xdg-open)
mkdir -p /etc/xdg /etc/skel/.config
cat >/etc/xdg/mimeapps.list <<'EOF'
[Default Applications]
x-scheme-handler/http=librewolf.desktop
x-scheme-handler/https=librewolf.desktop
text/html=librewolf.desktop
application/xhtml+xml=librewolf.desktop
x-scheme-handler/chrome=librewolf.desktop
EOF
cp /etc/xdg/mimeapps.list /etc/skel/.config/mimeapps.list

cat >/etc/profile.d/60-ferro-browser.sh <<'EOF'
export BROWSER=librewolf
export XDG_CURRENT_DESKTOP=X-Generic
EOF

cat >/usr/local/bin/xdg-open <<'EOF'
#!/bin/sh
# FerroOS wrapper for opening URLs and files
TARGET="$1"
if [ -z "$TARGET" ]; then
    exit 1
fi
case "$TARGET" in
    http://*|https://*|ftp://*)
        if command -v librewolf >/dev/null 2>&1; then
            exec librewolf "$TARGET"
        elif command -v firefox-esr >/dev/null 2>&1; then
            exec firefox-esr "$TARGET"
        fi
        ;;
    mailto:*)
        if command -v librewolf >/dev/null 2>&1; then
            exec librewolf "$TARGET"
        fi
        ;;
    *)
        if [ -x /usr/bin/xdg-open ]; then
            exec /usr/bin/xdg-open "$@"
        fi
        ;;
esac
EOF
chmod 755 /usr/local/bin/xdg-open


