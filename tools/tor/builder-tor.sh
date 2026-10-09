#!/bin/sh
# /init of the Tor build VM, run by `cargo xtask tor-build`. Builds Arti (the
# Tor Project's Rust implementation of Tor) as one static binary, and writes
# it as a tar stream onto the raw disk /dev/vda for the host to read.

export PATH=/usr/sbin:/usr/bin:/sbin:/bin

fail() { echo "FERRO-BUILD-FAILED: $*"; sync; poweroff -f; }

mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev
mkdir -p /dev/pts /build /out && mount -t devpts devpts /dev/pts
for m in failover net_failover virtio_net virtio_blk; do
    insmod /builder/modules/$m.ko || fail "insmod $m"
done

ip link set lo up
ip link set eth0 up
ip addr add 10.0.2.15/24 dev eth0
ip route add default via 10.0.2.2
echo "nameserver 10.0.2.3" > /etc/resolv.conf

echo "== installing build tools"
# The build needs more room than RAM: the scratch disk becomes swap, so the
# RAM-backed build directory can spill onto it.
mkswap /dev/vdb >/dev/null && swapon /dev/vdb || fail "swap"
mount -t tmpfs -o size=14G tmpfs /build || fail "tmpfs"
apk add --no-cache -q build-base rust cargo perl linux-headers pkgconf cmake clang >/dev/null || fail "apk add"
echo "== $(rustc --version)"

export CARGO_HOME=/build/cargo CARGO_TARGET_DIR=/build/target
# Static, so it runs on FerroOS (which has no C library to load). An explicit
# --target keeps this off the build-time proc-macros, which must be dynamic.
export RUSTFLAGS="-C target-feature=+crt-static"
export CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_STRIP=true
# The client proxy only: SOCKS, DNS, onion sites; TLS via rustls; SQLite
# compiled in; hardening (no core dumps, no ptrace) on.
FEATURES="tokio,rustls,dns-proxy,harden,compression,static-sqlite,onion-service-client"

echo "== building arti with $(nproc) CPUs"
START=$(date +%s)
cargo install --locked --target "$(rustc -vV | sed -n "s/^host: //p")" --root /build/inst --no-default-features --features "$FEATURES" arti >/build/cargo.log 2>&1 || {
    grep -v "^ *\(Downloaded\|Compiling\|Downloading\)" /build/cargo.log | tail -40
    fail "cargo install arti"
}
echo "== built in $(( $(date +%s) - START ))s"

cp /build/inst/bin/arti /out/arti || fail "copy"
grep -oE "Installed package .arti v[0-9.]+" /build/cargo.log | sed 's/.*arti /arti /' > /out/arti.version
file /out/arti 2>/dev/null | grep -q "dynamically" && fail "arti came out dynamically linked"
tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/arti) byte arti ($(cat /out/arti.version))"
poweroff -f
