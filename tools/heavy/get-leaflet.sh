#!/bin/sh
# Fetches Leaflet, the PDF and document app, at the commit in
# heavy/leaflet.commit into OUT (default build/leaflet): leaflet.py,
# leaflet_formats.py, its ui/ and icons/, `commit` and `built`.
# Leaflet is Python and a web page, so there's nothing to build: what goes
# into the image is the release Leaflet published for that commit
# (build-COMMIT, github.com/anti-ghostsec/leaflet), and only if it's signed
# (minisign) with Leaflet's release key as the build of that very commit,
# the same check ferro-leaflet-update makes on an installed system.
#
#   tools/heavy/get-leaflet.sh [OUT]
set -eu
cd "$(dirname "$0")/../.."
out=${1:-build/leaflet}
commit=$(tr -d ' \n' <heavy/leaflet.commit)
printf '%s' "$commit" | grep -Eqx '[0-9a-f]{40}' || { echo "get-leaflet: heavy/leaflet.commit isn't a commit" >&2; exit 1; }
short=${commit%"${commit#???????}"}
releases=${FERRO_LEAFLET_RELEASES:-https://github.com/anti-ghostsec/leaflet/releases/download/build-$short}
archive=leaflet-linux.tar.gz
# Leaflet's release signing key (minisign.pub in github.com/anti-ghostsec/leaflet).
pubkey=RWRr5HQeZWdeynbMq70ZUR+UoIUCA0D1E8dE41R9OBeK2FsT8QMP1s4W
fail() { echo "get-leaflet: $*" >&2; exit 1; }
command -v minisign >/dev/null 2>&1 || fail "minisign is needed to check Leaflet's signature (sudo apt install minisign)"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
get() { # NAME
    curl --proto '=https' --tlsv1.2 -fsSL --retry 2 --connect-timeout 30 --max-time 600 -o "$work/$1" "$releases/$1"
}
get "$archive" && get "$archive.minisig" || fail "no signed release of Leaflet for $short (is build-$short published?)"

# Signed with Leaflet's key, with "leaflet COMMIT TIME" as its trusted comment.
trusted=$(minisign -V -Q -P "$pubkey" -m "$work/$archive" -x "$work/$archive.minisig" 2>/dev/null) ||
    fail "the release's signature doesn't check out"
set -f
# shellcheck disable=SC2086
set -- $trusted
set +f
[ "$#" = 3 ] && [ "$1" = leaflet ] && [ "$2" = "$commit" ] && printf '%s' "$3" | grep -Eqx '[0-9]{1,12}' ||
    fail "the signed release isn't the build of $commit"
when=$3

# Only leaflet/..., plain files and folders, nothing climbing out.
tar -tvzf "$work/$archive" >"$work/list" && tar -tzf "$work/$archive" >"$work/names" || fail "the release can't be read"
if grep -Evx 'leaflet/?|leaflet/.+' "$work/names" | grep -q . || grep -Eq '(^|/)\.\.(/|$)' "$work/names" ||
    grep -Eq '^[^-d]' "$work/list"; then
    fail "the release has things in it it shouldn't"
fi
mkdir "$work/new"
tar -xzf "$work/$archive" -C "$work/new" --no-same-owner --no-same-permissions
new=$work/new/leaflet
[ -f "$new/leaflet.py" ] && [ -f "$new/leaflet_formats.py" ] && [ -f "$new/ui/index.html" ] &&
    [ -f "$new/icons/hicolor/scalable/apps/leaflet.svg" ] && [ "$(tr -d ' \n' <"$new/commit")" = "$commit" ] ||
    fail "the release isn't a whole Leaflet"

rm -rf "$out"
mkdir -p "$out"
cp -r "$new/leaflet.py" "$new/leaflet_formats.py" "$new/ui" "$new/icons" "$new/LICENSE" "$out/"
find "$out" -name '__pycache__' -prune -exec rm -rf {} +
echo "$commit" >"$out/commit"
# The commit's time: ferro-leaflet-update never goes back past it.
echo "$when" >"$out/built"
echo "get-leaflet: Leaflet $short (signed) in $out"
