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

echo "== building lyrebird (obfs4proxy)"
mkdir -p /build/src
cd /build/src
git clone --depth 1 https://gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird.git || fail "clone lyrebird"
cd lyrebird
go build -ldflags="-s -w" -o /out/lyrebird ./cmd/lyrebird || fail "build lyrebird"

echo "== building snowflake-client"
cd /build/src
git clone --depth 1 https://gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/snowflake.git || fail "clone snowflake"
cd snowflake/client
go build -ldflags="-s -w" -o /out/snowflake-client . || fail "build snowflake-client"

echo "== building webtunnel-client"
cd /build/src
git clone --depth 1 https://gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/webtunnel.git || fail "clone webtunnel"
cd webtunnel/main/client
go build -ldflags="-s -w" -o /out/webtunnel-client . || fail "build webtunnel-client"

file /out/lyrebird | grep -q "statically linked" || fail "lyrebird not static"
file /out/snowflake-client | grep -q "statically linked" || fail "snowflake-client not static"
file /out/webtunnel-client | grep -q "statically linked" || fail "webtunnel-client not static"

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-PT-BUILD-OK: Built static lyrebird, snowflake-client, and webtunnel-client"
