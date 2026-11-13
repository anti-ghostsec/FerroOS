#!/bin/sh
# Builds Wisp, FerroOS Heavy's web browser (github.com/anti-ghostsec/wisp),
# for the Heavy image: against the same Debian release's WebKitGTK and GTK,
# so it runs on the libraries the image installs (heavy/packages.txt).
#
#   tools/heavy/build-wisp.sh OUT
#
# The commit built is the one in heavy/wisp.commit: a newer Wisp gets into
# FerroOS by changing that line. Run as root on Debian trixie (the release
# workflow runs it in a debian:trixie container):
#
#   docker run --rm -v "$PWD:/src" -w /src debian:trixie tools/heavy/build-wisp.sh build/wisp
#
# OUT gets `wisp` (the program) and `plugins/` (its bundled plugins).

set -eu

out=${1:?usage: build-wisp.sh OUT}
here=$(cd "$(dirname "$0")/../.." && pwd)
commit=$(tr -d ' \n' <"$here/heavy/wisp.commit")
case "$commit" in
    *[!0-9a-f]* | "") echo "build-wisp: heavy/wisp.commit isn't a commit hash" >&2; exit 1 ;;
esac

export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q --no-install-recommends ca-certificates curl git build-essential pkg-config \
    libwebkitgtk-6.0-dev libgtk-4-dev >/dev/null

# Debian's Rust can be older than Wisp's libraries need. Rust's installer is
# fetched at a set version and run only if it's the file expected (its
# SHA-256, from static.rust-lang.org's rustup-init.sha256 for that version),
# as Wisp's own build does; it installs a set Rust. To move on, change all three.
rustup_version=1.29.3
rustup_sha256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55
rust_version=1.99.1
if ! command -v cargo >/dev/null 2>&1; then
    [ "$(uname -m)" = x86_64 ] || { echo "build-wisp: no Rust, and its installer is only known here for x86_64" >&2; exit 1; }
    # Under its own name: the installer takes any other for one of Rust's tools.
    tmp=$(mktemp -d)
    init=$tmp/rustup-init
    curl --proto '=https' --tlsv1.2 -fsSL -o "$init" \
        "https://static.rust-lang.org/rustup/archive/$rustup_version/x86_64-unknown-linux-gnu/rustup-init"
    echo "$rustup_sha256  $init" | sha256sum -c - >/dev/null || {
        echo "build-wisp: rustup-init $rustup_version isn't the file expected" >&2
        rm -rf "$tmp"
        exit 1
    }
    chmod +x "$init"
    "$init" -y -q --profile minimal --default-toolchain "$rust_version"
    rm -rf "$tmp"
    . "$HOME/.cargo/env"
fi

work=$(mktemp -d)
git clone -q https://github.com/anti-ghostsec/wisp.git "$work/wisp"
git -C "$work/wisp" -c advice.detachedHead=false checkout -q "$commit"
# Exactly the commit asked for (a branch or tag name could move).
[ "$(git -C "$work/wisp" rev-parse HEAD)" = "$commit" ] || { echo "build-wisp: $commit isn't in Wisp's history" >&2; exit 1; }
cargo build --release --locked --manifest-path "$work/wisp/Cargo.toml"

mkdir -p "$out"
rm -rf "$out/plugins"
install -m 755 "$work/wisp/target/release/wisp" "$out/wisp"
cp -r "$work/wisp/plugins" "$out/plugins"
echo "$commit" >"$out/commit"
rm -rf "$work"
echo "build-wisp: Wisp $commit in $out"
