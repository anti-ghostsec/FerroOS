#!/bin/sh
# FerroOS Heavy: what makes a fresh Debian into FerroOS Heavy. Debian's
# installer runs this once, inside the new system, as its last step
# (heavy/preseed.cfg). It carries over what FerroOS does where a full Debian
# can do it: encrypted DNS, random MAC addresses, no hostname on the network,
# a locked-down kernel, nothing listening. And it sets up the Windows 95 look.
#
# Nothing here may stop the installation: a step that fails says so in
# /var/log/ferro-heavy.log and the rest carries on.

HERE=/usr/local/share/ferro-heavy
LOG=/var/log/ferro-heavy.log
export DEBIAN_FRONTEND=noninteractive

step() { echo "== $*" >>"$LOG"; }
warn() { echo "FAILED: $*" >>"$LOG"; }

: >"$LOG"

step "kernel settings"
# The same list FerroOS's init applies (crates/ferro-init/src/system.rs).
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
# ptrace_scope is 1, not FerroOS's 2: Proton's and Steam's own crash handlers
# and some games' launchers attach to their children.

# Boot options: no raw kernel memory access and signed modules only (the
# NVIDIA driver is signed on this machine, see `ferro-heavy enroll-key`);
# kernel heap and stack hardening that costs games nothing measurable.
mkdir -p /etc/default/grub.d
cat >/etc/default/grub.d/60-ferro-heavy.cfg <<'EOF'
GRUB_CMDLINE_LINUX_DEFAULT="$GRUB_CMDLINE_LINUX_DEFAULT lockdown=integrity slab_nomerge randomize_kstack_offset=on vsyscall=none"
EOF
update-grub >>"$LOG" 2>&1 || warn "update-grub"

step "no memory dumps on disk"
mkdir -p /etc/systemd/coredump.conf.d
printf '[Coredump]\nStorage=none\nProcessSizeMax=0\n' >/etc/systemd/coredump.conf.d/60-ferro-heavy.conf
echo '* hard core 0' >/etc/security/limits.d/60-ferro-heavy.conf

step "firewall"
# Nothing gets in that this machine didn't ask for.
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
systemctl enable nftables >>"$LOG" 2>&1 || warn "enable nftables"

step "network privacy"
# A new random MAC address for every connection, wired and Wi-Fi, and while
# scanning; no hostname sent to the network; temporary IPv6 addresses.
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

step "encrypted DNS"
# DNS over TLS to the resolvers FerroOS uses, checked by name; the network's
# own DNS servers are never asked ("~." sends every name to these).
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
systemctl enable systemd-resolved >>"$LOG" 2>&1 || warn "enable systemd-resolved"

step "updates"
# Security updates install themselves.
cat >/etc/apt/apt.conf.d/60-ferro-heavy <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF

step "NVIDIA driver"
# Only where there's an NVIDIA card. It's built here by dkms and signed with
# a key made on this machine; until that key is approved (`ferro-heavy
# enroll-key`), Secure Boot keeps it from loading and the screen runs on the
# firmware's or the open driver's display.
if lspci -nn 2>/dev/null | grep -Ei 'vga|3d|display' | grep -qi nvidia; then
    apt-get install -y -q nvidia-driver firmware-misc-nonfree libcuda1 nvidia-smi >>"$LOG" 2>&1 || warn "install nvidia-driver"
else
    echo "no NVIDIA card found" >>"$LOG"
fi

step "Windows 95 look"
sh "$HERE/theme.sh" >>"$LOG" 2>&1 || warn "Chicago95 theme (the desktop keeps Xfce's own look; run 'sudo ferro-heavy theme' later)"

step "the ferro-heavy command"
install -m 755 "$HERE/ferro-heavy" /usr/local/bin/ferro-heavy || warn "install ferro-heavy"

echo "== done" >>"$LOG"
exit 0
