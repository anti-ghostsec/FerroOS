#!/bin/sh
# install-here.sh: puts FerroOS programs built from this checkout onto the
# PC it's on, for trying a change before it's released. Builds each program
# named (release build) and swaps it into /usr/local/bin the way ferro-update
# does: a copy beside it, then a rename, so a program that's running (the
# desktop itself) carries on with its old copy until it next starts.
#
#   tools/heavy/install-here.sh ferro-files ferro-wm ...
#
# What's shown to be installed is offered first with:
#   ferro-updates --ask "LINE1" "LINE2" sh tools/heavy/install-here.sh ...

set -eu
[ $# -ge 1 ] || { echo "usage: $0 PROGRAM..." >&2; exit 2; }
cd "$(dirname "$0")/../.."

args=""
for p in "$@"; do args="$args -p $p"; done
echo "Building $*..."
# shellcheck disable=SC2086
cargo build --release $args

for p in "$@"; do
    tmp="/usr/local/bin/.install-here.$$.$p"
    sudo cp "target/release/$p" "$tmp"
    sudo chmod 755 "$tmp"
    sudo mv -f "$tmp" "/usr/local/bin/$p"
    echo "Installed $p."
done
echo "Done. Programs that were open use the new build once they're started again."
