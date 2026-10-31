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
# User namespaces stay available to unprivileged programs: Chromium's and
# Electron's own sandboxes are built on them, and without them those apps
# run unsandboxed (--no-sandbox), which is worse. The cost is kernel attack
# surface; our seccomp sandbox (ferro-run) refuses new namespaces to the
# programs it starts.
kernel.unprivileged_userns_clone = 1
EOF
# ptrace_scope is 1, not FerroOS's 2: Proton's and Steam's crash handlers and
# some games' launchers attach to their own children.

# amdgpu.sg_display=0: Ryzen graphics (Raphael, Phoenix...) show the screen
# from one block of memory, not pages scattered about: scattered, under the
# strict IOMMU above, the display reads pages it isn't given (IO_PAGE_FAULT)
# and the screen goes white or flickers, at once with smooth drawing on.
# ---- Boot options for installed systems: no raw kernel memory access and
# signed modules only (a driver built on the machine, like NVIDIA's, is signed
# there; see `ferro-heavy enroll-key`), plus heap and stack hardening that
# costs games nothing measurable. Hash tables are pinned to keep idle RAM low.
# audit=0, udev.children_max=1 and cgroup_no_v1=all shave ~15MB off kernel/daemon idle RAM.
mkdir -p /etc/default/grub.d
cat >/etc/default/grub.d/60-ferro-heavy.cfg <<'EOF'
GRUB_CMDLINE_LINUX_DEFAULT="quiet loglevel=3 audit=0 cgroup_no_v1=all lockdown=integrity intel_iommu=on intel_iommu=igfx_off amd_iommu=force_isolation iommu=force iommu.passthrough=0 iommu.strict=1 efi=disable_early_pci_dma slab_nomerge randomize_kstack_offset=on vsyscall=none init_on_alloc=1 init_on_free=1 page_alloc.shuffle=1 transparent_hugepage=never dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512 udev.children_max=1 amdgpu.sg_display=0 i915.enable_psr=0 i915.enable_fbc=0 i915.enable_dc=0"
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

# ---- Intel graphics stability: disable PSR, FBC, and DC states to prevent FIFO underrun & black screen flickers
mkdir -p /etc/modprobe.d
cat >/etc/modprobe.d/i915.conf <<'EOF'
options i915 enable_psr=0 enable_fbc=0 enable_dc=0 fastboot=1
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
# Starting, reloading or stopping the firewall clears every rule, including the ones
# that keep traffic inside Tor while it's switched on: ferro-tor puts them
# straight back.
mkdir -p /etc/systemd/system/nftables.service.d
printf '[Service]\nExecStartPost=-/usr/local/sbin/ferro-tor reapply\nExecReload=\nExecReload=/usr/sbin/nft -f /etc/nftables.conf\nExecReload=-/usr/local/sbin/ferro-tor reapply\nExecStopPost=-/usr/local/sbin/ferro-tor reapply\n' \
    >/etc/systemd/system/nftables.service.d/60-ferro-tor.conf
# The same for the VPN's kill switch (ferro-vpn).
printf '[Service]\nExecStartPost=-/usr/local/sbin/ferro-vpn reapply\nExecReload=-/usr/local/sbin/ferro-vpn reapply\nExecStopPost=-/usr/local/sbin/ferro-vpn reapply\n' \
    >/etc/systemd/system/nftables.service.d/61-ferro-vpn.conf

# ---- Network privacy: a new random MAC address for every connection, wired
# and Wi-Fi, and while scanning; no hostname sent to the network; temporary
# IPv6 addresses; internal DHCP, and lean keyfile plugin.
mkdir -p /etc/NetworkManager/conf.d
cat >/etc/NetworkManager/conf.d/60-ferro-heavy.conf <<'EOF'
[main]
# Names are looked up only through the encrypted resolvers set below, never
# through whatever the router hands out over DHCP.
dns=none
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

# ---- Xorg memory tuning: disable RECORD and XTEST.
# (Composite extension remains enabled so Chromium, Electron, and hardware-accelerated apps render without blank windows.)
mkdir -p /etc/X11/xorg.conf.d
cat >/etc/X11/xorg.conf.d/10-serverflags.conf <<'EOF'
Section "ServerFlags"
    Option "BlankTime"   "0"
    Option "StandbyTime" "0"
    Option "SuspendTime" "0"
    Option "OffTime"     "0"
    Option "DPMS"        "false"
EndSection
EOF

cat >/etc/X11/xorg.conf.d/60-ferro-heavy.conf <<'EOF'
Section "Extensions"
    Option "RECORD" "Disable"
    Option "XTEST" "Disable"
EndSection
EOF

# ---- Modesetting driver with Glamor acceleration and hardware TearFree VSync
cat >/etc/X11/xorg.conf.d/20-modesetting.conf <<'EOF'
Section "Device"
    Identifier  "Intel Graphics"
    Driver      "modesetting"
    Option      "TearFree"        "true"
    Option      "DRI"             "3"
    Option      "AccelMethod"     "glamor"
EndSection
EOF

# ---- Compositor: hardware VSync, tear-free video, and stable GLX sync fences
mkdir -p /etc/xdg
cat >/etc/xdg/picom.conf <<'EOF'
# FerroOS Compositor Configuration
# Hardware VSync with zero tearing for web video, games, and window moving.
# Shadows and fading disabled for crisp, authentic Windows 95 retro performance.

backend = "glx";
vsync = true;
use-damage = false;

# Intel UHD / Mesa Iris stability: prevent GLX sync stalls, race conditions, and black flashes
xrender-sync-fence = true;
glx-no-stencil = true;
glx-no-rebind-pixmap = true;
unredir-if-possible = false;

shadow = false;
fading = false;

wintypes:
{
  normal = { fade = false; shadow = false; }
  tooltip = { fade = false; shadow = false; opacity = 1.0; focus = true; full-shadow = false; }
  dock = { shadow = false; clip-shadow-above = true; }
  dnd = { shadow = false; }
  popup_menu = { opacity = 1.0; }
  dropdown_menu = { opacity = 1.0; }
};
EOF

# ---- DNS resolution: systemd-resolved, DNS-over-TLS only, to resolvers
# whose certificates are checked against their names. Never plain DNS: if
# they can't be reached, names don't resolve. A Wi-Fi sign-in page that
# blocks them has "Allow this network's sign-in page" (Network > Wi-Fi
# Networks, ferro-portal): plain lookups to that network's own server, for
# that link only, until the page's window closes, 10 minutes pass or the
# connection changes; never kept over a restart, never with Tor on.
# No DNSSEC validation: many home routers and public Wi-Fi strip or mangle the
# signatures, and "allow-downgrade" then fails whole domains (sites that load
# for everyone else come up as unreachable). DoT protects the lookups to the
# resolver, which validates.
mkdir -p /etc/systemd/resolved.conf.d
cat >/etc/systemd/resolved.conf.d/60-ferro-heavy.conf <<'EOF'
[Resolve]
DNS=9.9.9.9#dns.quad9.net 149.112.112.112#dns.quad9.net 2620:fe::fe#dns.quad9.net 1.1.1.1#cloudflare-dns.com 2606:4700:4700::1111#cloudflare-dns.com
FallbackDNS=
Domains=~.
DNSOverTLS=yes
DNSSEC=no
LLMNR=no
MulticastDNS=no
Cache=yes
CacheFromLocalhost=no
EOF
systemctl enable systemd-resolved

# ---- The clock sets itself from internet time servers: Cloudflare's,
# by name and by address, so it works even when the clock is so far off
# that encrypted DNS (which checks certificates against it) can't start.
# While Tor is on, ferro-tor sets the clock from the Tor network instead.
mkdir -p /etc/systemd/timesyncd.conf.d
cat >/etc/systemd/timesyncd.conf.d/60-ferro-heavy.conf <<'EOF'
[Time]
NTP=time.cloudflare.com 162.159.200.1 162.159.200.123
FallbackNTP=0.debian.pool.ntp.org 1.debian.pool.ntp.org
EOF
systemctl enable systemd-timesyncd >/dev/null 2>&1 || true

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
APT::Periodic::Download-Upgradeable-Packages "0";
APT::Periodic::AutocleanInterval "0";
EOF
# ---- Security updates install themselves, daily: Debian's security
# archive only (nothing else is upgraded behind the user's back), and never
# a restart. With Tor on, apt goes through Tor like everything else.
cat >/etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF
cat >/etc/apt/apt.conf.d/52ferro-unattended-upgrades <<'EOF'
Unattended-Upgrade::Origins-Pattern {
    "origin=Debian,codename=${distro_codename}-security,label=Debian-Security";
};
Unattended-Upgrade::Automatic-Reboot "false";
Unattended-Upgrade::Remove-Unused-Kernel-Packages "true";
EOF
# The updates run from apt's daily timer; unattended-upgrades.service only
# waits all day (31 MB) to finish one at shutdown, should one be running.
systemctl disable unattended-upgrades.service >/dev/null 2>&1 || true

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
# No sleep: a sleeping machine keeps the disk's keys in memory, and a seized
# one can be read out (cold boot, DMA) without the passphrase. Closing the
# lid or the sleep key switches off instead; powered off, only the
# passphrase opens the disk.
HandleLidSwitch=poweroff
HandleLidSwitchExternalPower=poweroff
HandleLidSwitchDocked=ignore
HandleSuspendKey=poweroff
HandleHibernateKey=poweroff
HandleSleepKey=poweroff
EOF

# ---- No sleep at all, so the disk's keys never sit in a sleeping
# machine's memory (logind above powers off instead).
mkdir -p /etc/systemd/sleep.conf.d
cat >/etc/systemd/sleep.conf.d/60-ferro-sleep.conf <<'EOF'
[Sleep]
AllowSuspend=no
AllowHibernation=no
AllowSuspendThenHibernate=no
AllowHybridSleep=no
EOF
# Nor waking from hibernation: nothing in the boot looks for a hibernation
# image to resume from. There's no swap on the disk either (ferro-install
# makes none): what memory holds never gets written out unencrypted.
mkdir -p /etc/initramfs-tools/conf.d
echo 'RESUME=none' >/etc/initramfs-tools/conf.d/resume


# ---- Mask unused virtual consoles and periodic background scrubbers.
systemctl mask getty@tty3.service getty@tty4.service getty@tty5.service getty@tty6.service >/dev/null 2>&1 || true
systemctl mask e2scrub_all.timer e2scrub_all.service >/dev/null 2>&1 || true

# ---- Firmware updates (fwupd): only when asked. Its daemon isn't started at
# boot: D-Bus starts it when fwupdmgr needs it, and it quits again after two
# idle minutes (its own default is two hours), so it costs no memory the
# rest of the time. Its daily timer, which fetches the firmware list from
# the LVFS by itself, is off: nothing is fetched until `fwupdmgr refresh`.
systemctl disable fwupd-refresh.timer >/dev/null 2>&1 || true
systemctl mask fwupd-refresh.timer >/dev/null 2>&1 || true
systemctl disable fwupd.service >/dev/null 2>&1 || true
if [ -d /etc/fwupd ] && ! grep -q '^IdleTimeout=' /etc/fwupd/fwupd.conf 2>/dev/null; then
    if grep -q '^\[fwupd\]' /etc/fwupd/fwupd.conf 2>/dev/null; then
        sed -i '/^\[fwupd\]/a IdleTimeout=120' /etc/fwupd/fwupd.conf
    else
        printf '[fwupd]\nIdleTimeout=120\n\n' | cat - /etc/fwupd/fwupd.conf 2>/dev/null >/etc/fwupd/fwupd.conf.new || true
        mv -f /etc/fwupd/fwupd.conf.new /etc/fwupd/fwupd.conf
    fi
fi

# ---- Restrictive default umask (077): files created by users are private by default.
mkdir -p /etc/profile.d
echo 'umask 077' >/etc/profile.d/60-ferro-umask.sh
if [ -f /etc/login.defs ]; then
    sed -i 's/^UMASK.*/UMASK\t\t077/' /etc/login.defs 2>/dev/null || true
fi

# ---- No root password; accounts in the sudo group use sudo.
passwd -l root >/dev/null

# ---- Passwordless power management: accounts in the sudo group may shut down and restart
mkdir -p /etc/sudoers.d
cat >/etc/sudoers.d/60-ferro-power <<'EOF'
%sudo ALL=(ALL) NOPASSWD: /bin/systemctl poweroff, /bin/systemctl reboot, /sbin/poweroff, /sbin/reboot, /sbin/shutdown
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
         action.id == "org.freedesktop.login1.reboot-ignore-inhibit") &&
        subject.isInGroup("sudo")) {
        return polkit.Result.YES;
    }
});
EOF
chmod 0644 /etc/polkit-1/rules.d/60-ferro-power.rules

# ---- Removable storage permissions: allow users in sudo to mount, unmount, and eject disks
# Date & Time: administrators set the time zone and automatic time.
cat >/etc/polkit-1/rules.d/60-ferro-time.rules <<'EOF'
polkit.addRule(function(action, subject) {
    if ((action.id == "org.freedesktop.timedate1.set-timezone" ||
         action.id == "org.freedesktop.timedate1.set-ntp") &&
        subject.isInGroup("sudo")) {
        return polkit.Result.YES;
    }
});
EOF
chmod 0644 /etc/polkit-1/rules.d/60-ferro-time.rules

cat >/etc/polkit-1/rules.d/60-ferro-storage.rules <<'EOF'
polkit.addRule(function(action, subject) {
    if (subject.isInGroup("sudo") &&
        (action.id == "org.freedesktop.udisks2.filesystem-mount" ||
         action.id == "org.freedesktop.udisks2.filesystem-mount-system" ||
         action.id == "org.freedesktop.udisks2.filesystem-mount-other-seat" ||
         action.id == "org.freedesktop.udisks2.filesystem-unmount-others" ||
         action.id == "org.freedesktop.udisks2.encrypted-unlock" ||
         action.id == "org.freedesktop.udisks2.encrypted-unlock-system" ||
         action.id == "org.freedesktop.udisks2.eject-media" ||
         action.id == "org.freedesktop.udisks2.power-off-drive")) {
        return polkit.Result.YES;
    }
});
EOF
chmod 0644 /etc/polkit-1/rules.d/60-ferro-storage.rules

# ---- Drives pulled out without Eject still mount (see the file).
mkdir -p /etc/udisks2
cat >/etc/udisks2/mount_options.conf <<'EOF'
# FerroOS: mount NTFS drives and cards with ntfs-3g first. It recovers a
# volume that was unplugged without ejecting (clears the leftover journal),
# where the ntfs3 kernel driver refuses to mount it at all.
[defaults]
ntfs_drivers=ntfs,ntfs3
ntfs:ntfs_defaults=uid=$UID,gid=$GID,windows_names
ntfs:ntfs_allow=uid=$UID,gid=$GID,umask,dmask,fmask,locale,norecover,ignore_case,windows_names,compression,nocompression,big_writes
ntfs:ntfs3_defaults=uid=$UID,gid=$GID
ntfs:ntfs3_allow=uid=$UID,gid=$GID,umask,dmask,fmask,iocharset,discard,nodiscard,sparse,nosparse,hidden,nohidden,sys_immutable,nosys_immutable,showmeta,noshowmeta,prealloc,noprealloc,hide_dot_files,nohide_dot_files,windows_names,nocase,case
EOF
chmod 0644 /etc/udisks2/mount_options.conf

# ---- Tor: off until it's switched on (Network > VPN & Tor). ferro-tor starts it
# and routes everything through it; accounts in sudo may flip that switch.
systemctl disable tor.service >/dev/null 2>&1 || true
mkdir -p /var/lib/ferro/programs
# Others may pass through to the installed-programs record (Uninstall and
# Properties read it), not list what else is here.
chmod 711 /var/lib/ferro
chmod 755 /var/lib/ferro/programs
# The lock screen checks passwords (and the duress PIN) as root. Through
# sudo, check-password answers only for the asking account, one check at a
# time machine-wide, slower after each wrong answer.
echo '%sudo ALL=(root) NOPASSWD: /usr/local/lib/ferro-heavy/check-password' >/etc/sudoers.d/60-ferro-lock
chmod 0440 /etc/sudoers.d/60-ferro-lock

cat >/etc/sudoers.d/60-ferro-tor <<'EOF'
%sudo ALL=(ALL) NOPASSWD: /usr/local/sbin/ferro-tor on, /usr/local/sbin/ferro-tor on bridges, /usr/local/sbin/ferro-tor off, /usr/local/sbin/ferro-tor status
EOF
chmod 0440 /etc/sudoers.d/60-ferro-tor

# A Wi-Fi sign-in page (Network > Wi-Fi Networks): plain lookups for the
# connection in use, for a while. ferro-portal finds the link itself, so
# these three exact commands are all it takes.
cat >/etc/sudoers.d/60-ferro-portal <<'EOF'
%sudo ALL=(root) NOPASSWD: /usr/local/sbin/ferro-portal on, /usr/local/sbin/ferro-portal off, /usr/local/sbin/ferro-portal status
EOF
chmod 0440 /etc/sudoers.d/60-ferro-portal

# Settings > Updates: holding a part of FerroOS back from updates, and
# letting it go again (ferro-update-hold checks the name; uninstalling a
# part asks for the password).
cat >/etc/sudoers.d/60-ferro-update-hold <<'EOF'
%sudo ALL=(root) NOPASSWD: /usr/local/sbin/ferro-update-hold hold *, /usr/local/sbin/ferro-update-hold release *
EOF
chmod 0440 /etc/sudoers.d/60-ferro-update-hold

# The privacy switches (Network window, taskbar, flyout): each command line
# they use, written out, and nothing else. (sudo matches a rule's arguments
# exactly; a rule with none would allow any.) The script refuses other
# arguments too. apply-all sets microphone, camera and network at once.
S=/usr/local/sbin/ferro-switch-hardware
rule="%sudo ALL=(root) NOPASSWD: $S mic on, $S mic off, $S cam on, $S cam off, $S net on, $S net off"
for mic in on off; do
    for cam in on off; do
        for net in on off; do
            rule="$rule, $S apply-all $mic $cam $net"
        done
    done
done
echo "$rule" >/etc/sudoers.d/60-ferro-switches
chmod 0440 /etc/sudoers.d/60-ferro-switches
# The rule that keeps a switched-off microphone or camera off over restarts.
/usr/local/sbin/ferro-switch-hardware rules

# Encrypted extra drives: opening one this PC keeps a key for (Unlock
# automatically) needs no password, so it opens by itself when plugged in.
# Only `open`: ferro-drive checks the id it's given and opens nothing it
# doesn't keep a key for. Encrypting, remembering and forgetting ask.
echo '%sudo ALL=(root) NOPASSWD: /usr/local/sbin/ferro-drive open *' >/etc/sudoers.d/60-ferro-drives
chmod 0440 /etc/sudoers.d/60-ferro-drives

# The VPN switch. import takes the config on standard input, not a file name
# (so root never opens a file the caller chose), and ferro-vpn leaves out of
# every config the lines wg-quick would run as root commands.
cat >/etc/sudoers.d/60-ferro-vpn <<'EOF'
%sudo ALL=(root) NOPASSWD: /usr/local/sbin/ferro-vpn on, /usr/local/sbin/ferro-vpn off, /usr/local/sbin/ferro-vpn status, /usr/local/sbin/ferro-vpn import, /usr/local/sbin/ferro-vpn killswitch on, /usr/local/sbin/ferro-vpn killswitch off
EOF
chmod 0440 /etc/sudoers.d/60-ferro-vpn

# Restricted folders (Properties > Restricted): /usr/local/sbin/ferro-restrict
# deliberately has no NOPASSWD rule. Unlocking without the password would let
# any program running as the user (an AI assistant among them) open the
# folder again; so both ways go through %sudo's ordinary rule and ask.
# What each folder was is kept here: looked up by anyone, listed by none.
mkdir -p /var/lib/ferro/restricted
chmod 711 /var/lib/ferro/restricted

# End Process (Task Manager). Your own processes need no sudo at all; this
# lets ferro-endtask end them for scripts too, without a password, and it
# ends only the asker's (as them, so the kernel checks), never the lock
# screen's. Another account's, or root's, is `sudo ferro-endtask --other`,
# which matches none of these and so asks the password. (There used to be
# a rule here letting any program run /bin/kill as root, on anything.)
E=/usr/local/sbin/ferro-endtask
echo "%sudo ALL=(root) NOPASSWD: $E TERM *, $E KILL *, $E STOP *, $E CONT *" >/etc/sudoers.d/60-ferro-endtask
chmod 0440 /etc/sudoers.d/60-ferro-endtask

# USB while the screen is locked (ferro-usb-lock): Win+L may stop new USB
# devices without a password. Only check-password, after the right
# password, starts them again.
echo '%sudo ALL=(root) NOPASSWD: /usr/local/sbin/ferro-usb-lock lock' >/etc/sudoers.d/60-ferro-usb-lock
chmod 0440 /etc/sudoers.d/60-ferro-usb-lock

# The card the desktop is drawn on (Display Properties, Graphics): only
# `ferro-gpu desktop`, which takes auto or gpuN and nothing else, and
# `ferro-gpu close`, which closes the cards nothing is set to use.
echo '%sudo ALL=(root) NOPASSWD: /usr/local/bin/ferro-gpu desktop *, /usr/local/bin/ferro-gpu close' >/etc/sudoers.d/60-ferro-gpu
chmod 0440 /etc/sudoers.d/60-ferro-gpu
# A USB controller that turns up while locked (a dock) starts out locked
# too, and a device that slipped in under one before that is turned off.
# Never a controller's own root hub (usb1...): that would cut off what's
# already plugged in.
cat >/etc/udev/rules.d/71-ferro-usb-lock.rules <<'EOF'
# FerroOS: no new USB devices while the screen is locked (ferro-usb-lock).
ACTION=="add", SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", KERNEL=="usb[0-9]*", TEST=="/run/ferro/usb-locked", ATTR{authorized_default}="0"
ACTION=="add", SUBSYSTEM=="usb", ENV{DEVTYPE}=="usb_device", KERNEL!="usb[0-9]*", TEST=="/run/ferro/usb-locked", ATTR{authorized}="0"
EOF
chmod 644 /etc/udev/rules.d/71-ferro-usb-lock.rules
visudo -cq >/dev/null 2>&1 || echo "harden: the sudo rules don't check out (visudo -c)" >&2

# ---- Shared configuration and choices folders
mkdir -p /ProgramData/ferro /etc/ferro
chmod 1777 /ProgramData /ProgramData/ferro
chmod 755 /etc/ferro

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
x-scheme-handler/http=wisp.desktop
x-scheme-handler/https=wisp.desktop
text/html=wisp.desktop
application/xhtml+xml=wisp.desktop
EOF
cp /etc/xdg/mimeapps.list /etc/skel/.config/mimeapps.list

cat >/etc/profile.d/60-ferro-browser.sh <<'EOF'
if [ -x /usr/local/bin/wisp ]; then export BROWSER=wisp; fi
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
    http://*|https://*)
        if command -v wisp >/dev/null 2>&1; then
            exec wisp "$TARGET"
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

# ---- The desktop session (/etc/X11/xinit/xinitrc) is written by
# tools/heavy/builder-base.sh, after this script: one place, so what runs is
# what's written there. (A session written here, starting picom and
# ferro-inhibit, was always replaced by it, so neither has ever run; the
# desktop's smoothness comes from ferro-wm itself. Screen blanking is off in
# 10-serverflags.conf above.)



