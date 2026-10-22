#!/bin/sh
# Builder script for static Pluggable Transport client binaries (lyrebird/obfs4, snowflake-client, webtunnel-client).
# Cross-compiles with Go (CGO_ENABLED=0) for pure static execution on FerroOS.
set -eu

fail() { echo "FERRO-PT-BUILD-FAILED: $*"; exit 1; }

mkdir -p /build /out

echo "== installing Go toolchain"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    golang-go git ca-certificates file curl >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}

export GOPATH=/build/go
export CGO_ENABLED=0
export GOOS=linux
export GOARCH=amd64

# Each transport at a reviewed release, checked out by its commit: a tag or
# branch could be moved to point at something else.
LYREBIRD_COMMIT=0b10edbb61e0ca6fb70c7d57aeaabf315f1fade1  # lyrebird-0.8.1
SNOWFLAKE_COMMIT=1f472b16a662fddd297e343acf7e5c820a713c90 # v2.15.2
WEBTUNNEL_COMMIT=11334a222c1beeb45759d436b7aef963a10600b2 # v0.0.7

# fetch <repo> <commit>: exactly that commit, into ./<repo>.
fetch() {
    git init -q "$1" && git -C "$1" fetch -q --depth 1 "https://gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/$1.git" "$2" &&
        git -C "$1" checkout -q FETCH_HEAD && [ "$(git -C "$1" rev-parse HEAD)" = "$2" ]
}

echo "== building lyrebird (obfs4proxy)"
mkdir -p /build/src
cd /build/src
fetch lyrebird "$LYREBIRD_COMMIT" || fail "fetch lyrebird"
cd lyrebird
go build -ldflags="-s -w" -o /out/lyrebird ./cmd/lyrebird || fail "build lyrebird"

echo "== building snowflake-client"
cd /build/src
fetch snowflake "$SNOWFLAKE_COMMIT" || fail "fetch snowflake"
cd snowflake/client
go build -ldflags="-s -w" -o /out/snowflake-client . || fail "build snowflake-client"

echo "== building webtunnel-client"
cd /build/src
fetch webtunnel "$WEBTUNNEL_COMMIT" || fail "fetch webtunnel"
cd webtunnel/main/client
go build -ldflags="-s -w" -o /out/webtunnel-client . || fail "build webtunnel-client"

file /out/lyrebird | grep -q "statically linked" || fail "lyrebird not static"
file /out/snowflake-client | grep -q "statically linked" || fail "snowflake-client not static"
file /out/webtunnel-client | grep -q "statically linked" || fail "webtunnel-client not static"

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-PT-BUILD-OK: Built static lyrebird, snowflake-client, and webtunnel-client"
