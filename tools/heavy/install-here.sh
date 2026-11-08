#!/bin/sh
# install-here.sh: puts FerroOS programs built from this checkout onto the
# PC it's on, for trying a change before it's released. Builds each program
# named (release build) and swaps it into /usr/local/bin the way ferro-update
# does: a copy beside it, then a rename, so a program that's running (the
# desktop itself) carries on with its old copy until it next starts.
#
#   tools/heavy/install-here.sh ferro-files ferro-wm ...
#
# Two names aren't programs: ferro-ladspa is the Sound Enhancer's plugin
# (built for Debian's own programs, into /usr/local/lib/ferro-heavy/ladspa),
# and ferro-audio is the script in heavy/.
#
# What's shown to be installed is offered first with:
#   ferro-updates --ask "LINE1" "LINE2" sh tools/heavy/install-here.sh ...

set -eu
[ $# -ge 1 ] || { echo "usage: $0 PROGRAM..." >&2; exit 2; }
cd "$(dirname "$0")/../.."

args=""
for p in "$@"; do
    case "$p" in
        ferro-ladspa | ferro-audio) ;;
        *) args="$args -p $p" ;;
    esac
done
echo "Building $*..."
# shellcheck disable=SC2086
[ -z "$args" ] || cargo build --release $args
case " $* " in
    *" ferro-ladspa "*) cargo build --release --target x86_64-unknown-linux-gnu -p ferro-ladspa ;;
esac

for p in "$@"; do
    case "$p" in
        ferro-ladspa)
            dir=/usr/local/lib/ferro-heavy/ladspa
            sudo mkdir -p "$dir"
            sudo chmod 755 "$dir"
            sudo cp target/x86_64-unknown-linux-gnu/release/libferro_ladspa.so "$dir/.install-here.$$"
            sudo chmod 644 "$dir/.install-here.$$"
            sudo mv -f "$dir/.install-here.$$" "$dir/ferro-enhancer.so"
            echo "Installed the Sound Enhancer's plugin (ferro-audio apply puts it in use)."
            continue ;;
        ferro-audio)
            sudo install -m 755 heavy/ferro-audio /usr/local/bin/.install-here.$$.ferro-audio
            sudo mv -f /usr/local/bin/.install-here.$$.ferro-audio /usr/local/bin/ferro-audio
            echo "Installed ferro-audio."
            continue ;;
    esac
    tmp="/usr/local/bin/.install-here.$$.$p"
    sudo cp "target/release/$p" "$tmp"
    sudo chmod 755 "$tmp"
    sudo mv -f "$tmp" "/usr/local/bin/$p"
    echo "Installed $p."
done
echo "Done. Programs that were open use the new build once they're started again."
