#!/usr/bin/env bash
# Builds the FerroOS kernel and modules on a Linux (or WSL) host.
# On Windows without WSL, use `cargo xtask kernel-build`, which runs the same
# steps inside a QEMU VM (tools/kernel/builder-init.sh).
# Needs: build-essential flex bison bc libelf-dev libssl-dev kmod curl xz-utils
set -euo pipefail

# Pinned with its SHA-256 (from kernel.org's PGP-signed sha256sums.asc);
# override both together. Same pin as xtask/src/builder.rs.
if [ -z "${KVER:-}" ]; then
    KVER=6.18.58
    KSHA=069b19f9032c4dac8149ae4b6520c35debca0f42a9f6d52ee55ba6ca9b818013
fi
: "${KSHA:?set KSHA to linux-$KVER.tar.xz's SHA-256 from kernel.org's signed sha256sums.asc}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/build/kernel-src"
SRC="$WORK/linux-$KVER"
OUT="$ROOT/build/kernel"

mkdir -p "$WORK"
if [ ! -d "$SRC" ]; then
    echo "Downloading linux-$KVER..."
    TARBALL="$WORK/linux-$KVER.tar.xz"
    curl -fL -o "$TARBALL" "https://cdn.kernel.org/pub/linux/kernel/v${KVER%%.*}.x/linux-$KVER.tar.xz"
    echo "$KSHA  $TARBALL" | sha256sum -c - || { rm -f "$TARBALL"; echo "checksum mismatch: not building from it" >&2; exit 1; }
    tar -xJf "$TARBALL" -C "$WORK"
    rm -f "$TARBALL"
fi

FRAGS=("$ROOT/tools/kernel/ferro.config")
[ "${FERRO_KERNEL_HW:-0}" = 1 ] && FRAGS+=("$ROOT/tools/kernel/ferro-hw.config")

cd "$SRC"
# FerroOS doesn't enable AMD memory encryption, so the (empty) decrypted-BSS
# section needs no 2 MB alignment. Left in, it pads the kernel's BSS up to the
# next 2 MB boundary: up to 2 MB of RAM wasted, depending on where BSS ends.
sed -i '/#define BSS_DECRYPTED/,/__pi___end_bss_decrypted/ s/ALIGN(PMD_SIZE)/ALIGN(PAGE_SIZE)/' arch/x86/kernel/vmlinux.lds.S
make ARCH=x86_64 tinyconfig
scripts/kconfig/merge_config.sh -m .config "${FRAGS[@]}"
make ARCH=x86_64 olddefconfig
for f in "${FRAGS[@]}"; do
    grep -E '^CONFIG_[A-Z0-9_]+=' "$f" | while read -r line; do
        grep -qx "$line" .config || echo "NOT APPLIED: $line (dependency missing or renamed)"
    done
done

make ARCH=x86_64 -j"$(nproc)" bzImage modules
rm -rf "$OUT" && mkdir -p "$OUT"
cp arch/x86/boot/bzImage .config "$OUT/"
make ARCH=x86_64 modules_install INSTALL_MOD_PATH="$OUT" INSTALL_MOD_STRIP=1
echo "$KVER" > "$OUT/VERSION"
cp "$OUT/bzImage" "$ROOT/build/bzImage"
echo custom > "$ROOT/build/kernel-source"
echo "Kernel: $ROOT/build/bzImage ($(du -h "$ROOT/build/bzImage" | cut -f1)), modules in $OUT/lib/modules"
