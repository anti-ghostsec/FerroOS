#!/bin/sh
# The boot-file build VM's job, run by `cargo xtask iso` inside the minimal
# Debian that tools/vm/init.sh installs. Makes what firmware with Secure Boot
# on will start:
#
#   shimx64.efi   Debian's shim, signed by Microsoft; firmware trusts it
#   mmx64.efi     shim's key manager, where you approve FerroOS's key once
#   ferro-*.efi   the kernel, FerroOS (the initramfs) and the kernel command
#                 line as one file, signed with this build's key; shim checks
#                 it, so none of the three can be changed on the disk
#   ferro-mok.*   that key: made here the first time, kept by the host in
#                 build/secureboot and reused, so it's approved only once
#
# and writes them as a tar stream onto the raw disk /dev/vda for the host.

fail() { echo "FERRO-BUILD-FAILED: $*"; exit 1; }

mkdir -p /build /out
cd /build || fail "cd"

echo "== installing signing tools"
apt-get update -q >/tmp/apt.log 2>&1 && apt-get install -y -q --no-install-recommends \
    sbsigntool systemd-ukify systemd-boot-efi openssl >>/tmp/apt.log 2>&1 || {
    tail -20 /tmp/apt.log
    fail "apt-get install"
}
# Unpacked, not installed: installing shim tries to set up this VM's own boot.
apt-get download -q shim-signed shim-helpers-amd64-signed >>/tmp/apt.log 2>&1 || {
    tail -5 /tmp/apt.log
    fail "download shim"
}
for deb in shim-*.deb; do
    dpkg-deb -x "$deb" /build/shim || fail "unpack $deb"
done
SHIM=$(find /build/shim -name 'shimx64.efi.signed*' | head -n1)
MM=$(find /build/shim -name 'mmx64.efi.signed*' | head -n1)
[ -n "$SHIM" ] && [ -n "$MM" ] || fail "shim files not found"
cp "$SHIM" /out/shimx64.efi && cp "$MM" /out/mmx64.efi || fail "copy shim"

if [ -f /in/ferro-mok.key ] && [ -f /in/ferro-mok.crt ]; then
    cp /in/ferro-mok.key /in/ferro-mok.crt /out/ || fail "copy key"
else
    echo "== making this build's signing key"
    openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
        -subj "/CN=FerroOS build key/" -addext "extendedKeyUsage=codeSigning" \
        -keyout /out/ferro-mok.key -out /out/ferro-mok.crt >/tmp/openssl.log 2>&1 || {
        tail -5 /tmp/openssl.log
        fail "make key"
    }
fi
openssl x509 -in /out/ferro-mok.crt -outform DER -out /out/ferro-mok.der || fail "export certificate"

echo "== signing"
printf 'NAME=FerroOS\nID=ferroos\n' > /build/os-release
for kind in installed live; do
    ukify build --linux=/in/bzImage --initrd=/in/initramfs.cpio \
        --cmdline="$(cat /in/cmdline-$kind)" --os-release=@/build/os-release \
        --secureboot-private-key=/out/ferro-mok.key --secureboot-certificate=/out/ferro-mok.crt \
        --output=/out/ferro-$kind.efi >/tmp/ukify.log 2>&1 || {
        tail -20 /tmp/ukify.log
        fail "ukify ($kind)"
    }
    sbverify --cert /out/ferro-mok.crt /out/ferro-$kind.efi >/dev/null 2>&1 || fail "signature check ($kind)"
done

tar -cf /dev/vda -C /out . || fail "write result"
sync
echo "FERRO-BUILD-OK $(wc -c < /out/ferro-installed.efi) byte signed FerroOS"
