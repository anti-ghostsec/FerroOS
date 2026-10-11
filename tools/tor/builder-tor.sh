#!/bin/sh
# The Tor build VM's job, run by `cargo xtask tor-build` inside the minimal
# Debian that tools/vm/init.sh installs. Builds Arti (the Tor Project's Rust
# implementation of Tor) as one static binary, and writes it as a tar stream
# onto the raw disk /dev/vda for the host to read. The build needs more room
# than RAM: the scratch disk /dev/vdb is already swap.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /build /out

echo "== installing build tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    build-essential musl-tools perl pkgconf cmake clang libclang-dev file \
    curl ca-certificates >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

# Rust from rustup: Arti wants a newer compiler than a stable distribution
# carries.
export RUSTUP_HOME=/build/rustup
export CARGO_HOME=/build/cargo CARGO_TARGET_DIR=/build/target
# Static musl, so it runs on FerroOS (which has no C library to load). An
# explicit --target keeps this off the build-time proc-macros, which must be
# dynamic. musl-gcc compiles the C parts (SQLite, compression).
TARGET=x86_64-unknown-linux-musl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs >/build/rustup-init.sh || fail "download rustup"
sh /build/rustup-init.sh -y -q --profile minimal --target "$TARGET" >/build/rustup.log 2>&1 || {
    tail -20 /build/rustup.log
    fail "rustup"
}
export PATH="$CARGO_HOME/bin:$PATH"
echo "== $(rustc --version)"
export RUSTFLAGS="-C target-feature=+crt-static"
export CC_x86_64_unknown_linux_musl=musl-gcc
export CARGO_PROFILE_RELEASE_OPT_LEVEL=s CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_STRIP=true
# The client proxy only: SOCKS, DNS, onion sites; TLS via rustls; SQLite
# compiled in; hardening (no core dumps, no ptrace) on.
FEATURES="tokio,rustls,dns-proxy,harden,compression,static-sqlite,onion-service-client"

echo "== building arti with $(nproc) CPUs"
START=$(date +%s)
cargo install --locked --target "$TARGET" --root /build/inst --no-default-features --features "$FEATURES" arti >/build/cargo.log 2>&1 || {
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
