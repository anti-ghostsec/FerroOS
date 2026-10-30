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

# Debian's Rust can be older than Wisp's libraries need.
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable
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
