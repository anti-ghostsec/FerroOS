#!/bin/sh
# Fetches Folio, the text editor, at the commit in heavy/folio.commit into
# OUT (default build/folio): folio.py, its ui/ and icons/, and `commit`.
# Folio is Python and a web page, so there's nothing to build.
#
#   tools/heavy/get-folio.sh [OUT]
set -eu
cd "$(dirname "$0")/../.."
out=${1:-build/folio}
commit=$(tr -d ' \n' <heavy/folio.commit)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git clone -q https://github.com/anti-ghostsec/folio "$work/src"
git -C "$work/src" checkout -q "$commit"
[ "$(git -C "$work/src" rev-parse HEAD)" = "$commit" ] || { echo "get-folio: not the commit asked for" >&2; exit 1; }
rm -rf "$out"
mkdir -p "$out"
cp -r "$work/src/folio.py" "$work/src/ui" "$work/src/icons" "$work/src/LICENSE" "$out/"
find "$out" -name '__pycache__' -prune -exec rm -rf {} +
echo "$commit" >"$out/commit"
echo "get-folio: Folio ${commit%"${commit#???????}"} in $out"
