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
net.ipv4.icmp_echo_ignore_broadcasts = 1
EOF
# ptrace_scope is 1, not FerroOS's 2: Proton's and Steam's crash handlers and
# some games' launchers attach to their own children.

# ---- Boot options for installed systems: no raw kernel memory access and
# signed modules only (a driver built on the machine, like NVIDIA's, is signed
# there; see `ferro-heavy enroll-key`), plus heap and stack hardening that
# costs games nothing measurable.
mkdir -p /etc/default/grub.d
cat >/etc/default/grub.d/60-ferro-heavy.cfg <<'EOF'
GRUB_CMDLINE_LINUX_DEFAULT="quiet lockdown=integrity slab_nomerge randomize_kstack_offset=on vsyscall=none"
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
    }
    chain forward {
        type filter hook forward priority filter; policy drop;
    }
}
EOF
systemctl enable nftables

# ---- Network privacy: a new random MAC address for every connection, wired
# and Wi-Fi, and while scanning; no hostname sent to the network; temporary
# IPv6 addresses.
mkdir -p /etc/NetworkManager/conf.d
cat >/etc/NetworkManager/conf.d/60-ferro-heavy.conf <<'EOF'
[main]
dns=systemd-resolved
hostname-mode=none

[device]
wifi.scan-rand-mac-address=yes

[connection]
wifi.cloned-mac-address=random
ethernet.cloned-mac-address=random
ipv4.dhcp-send-hostname=0
ipv6.dhcp-send-hostname=0
ipv6.ip6-privacy=2
connection.llmnr=0
connection.mdns=0
EOF

# ---- Encrypted DNS: DNS over TLS to the resolvers FerroOS uses, checked by
# name; the network's own DNS servers are never asked ("~." sends every name
# to these).
mkdir -p /etc/systemd/resolved.conf.d
cat >/etc/systemd/resolved.conf.d/60-ferro-heavy.conf <<'EOF'
[Resolve]
DNS=9.9.9.9#dns.quad9.net 149.112.112.112#dns.quad9.net 194.242.2.2#dns.mullvad.net
FallbackDNS=
Domains=~.
DNSOverTLS=yes
LLMNR=no
MulticastDNS=no
EOF
systemctl enable systemd-resolved

# ---- Packages over HTTPS, so the network can't see which ones; with the
# sections the NVIDIA driver, Steam and firmware live in.
cat >/etc/apt/sources.list <<'EOF'
deb https://deb.debian.org/debian @SUITE@ main contrib non-free non-free-firmware
deb https://deb.debian.org/debian @SUITE@-updates main contrib non-free non-free-firmware
deb https://security.debian.org/debian-security @SUITE@-security main contrib non-free non-free-firmware
EOF
# Only what's asked for: no "recommended" extras.
printf 'APT::Install-Recommends "false";\n' >/etc/apt/apt.conf.d/60-ferro-heavy

# ---- One machine name for everyone, so it identifies nobody.
echo ferro >/etc/hostname
printf '127.0.0.1 localhost ferro\n::1 localhost ip6-localhost ip6-loopback\n' >/etc/hosts

# ---- No root password; accounts in the sudo group use sudo.
passwd -l root >/dev/null
