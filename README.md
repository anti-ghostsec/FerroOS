# FerroOS

A small Linux-based OS with a Windows 95 desktop, written in Rust. It idles at
about 29 MB of RAM and tries hard not to leak anything about you.

![FerroOS desktop](docs/screenshots/terminal.png)

The Linux kernel only does drivers. Everything on top of it (init, the
desktop, the network stack, the command prompt) is one static Rust binary.
Paths look like `C:\home` because someone asked me to.

> **Heads up:** this is a hobby project. It hasn't been audited and has mostly
> been tested in QEMU. If your safety depends on it, use Tails or Whonix.

## Try it

Grab `ferroos.iso` from [Releases](https://github.com/anti-ghostsec/FerroOS/releases),
flash it to a USB stick with [Rufus](https://rufus.ie) (pick **DD image
mode** when it asks: ISO mode leaves out the partitions Setup installs from)
or [balenaEtcher](https://etcher.balena.io), and boot it. The menu lets you run it
live (nothing is saved) or install it. Installing wipes the disk you pick.

The live USB boots on UEFI or old BIOS machines. Installed systems need UEFI.
Secure Boot can stay on: the first start shows a blue "Verification failed"
screen, where you pick *Enroll key from disk* and `ferro-mok.der` once. After
that the firmware checks FerroOS (kernel, system and boot options, one signed
file) at every start.

## What it does

- Your settings and files live in an encrypted vault (Argon2id +
  XChaCha20-Poly1305). Everything else is in RAM and gone at shutdown. No swap,
  no hibernation, no core dumps.
- Apps run sandboxed (Landlock + seccomp + cgroups) and ask before going
  online. You can give each one a RAM limit like `512M` or `5G`.
- Tray switches kill the network, mic and camera at the kernel level.
- DNS goes over HTTPS, TLS uses Encrypted Client Hello, and your MAC address is
  random every boot.
- Wired networking, and Wi-Fi (WPA2/WPA3) on Intel, Realtek, MediaTek and
  Qualcomm cards: `WIFI LIST`, `WIFI JOIN <network>`.
- Optional WireGuard VPN and Tor, both off by default. When they're on, the
  routing table makes it impossible to leak around them, not just unlikely.
  Tor can run through the VPN.
- Right-click a photo to strip its metadata (GPS, camera, etc.).
- Programs built for ordinary Linux (glibc) run too, in the same sandbox: a
  minimal Debian rides along. It's on the ISO, so installing needs no
  internet. `DEBIAN apt install <package>` adds to it: on an installed disk
  that's kept on a LUKS2 partition opened with your vault password; in a live
  or save-nothing session it's kept in RAM and gone at shutdown.

| | | |
|---|---|---|
| ![Task Manager](docs/screenshots/processes.png) | ![RAM limits](docs/screenshots/budgets.png) | ![VPN and Tor](docs/screenshots/vpn.png) |
| ![Remove metadata](docs/screenshots/privacy.png) | ![Performance](docs/screenshots/performance.png) | ![Setup](docs/screenshots/setup.png) |

## Using it

The Command Prompt is DOS-flavoured: `DIR`, `CD`, `TYPE`, `MEM`, `PS`, `RUN`.
A few FerroOS-specific ones:

```
MEM /DETAIL           where every KB is going
RUN /NET app          run something with network allowed up front
PERMS /FORGET ALL     forget every remembered app choice
VPN IMPORT D:\x.conf  use your provider's WireGuard file
VPN ON / TOR ON       turn them on (also in Start > Settings > VPN and Tor)
DEBIAN apt install -y nano   install a Debian package (runs as root in Debian)
```

Shortcuts: Ctrl+Esc for Start, Ctrl+Shift+Esc for Task Manager, Alt+F4 to
close a window.

## Building

You need [Rust](https://rustup.rs) and [QEMU](https://www.qemu.org). That's it,
even on Windows. The kernel, Tor and the ISO are built inside throwaway QEMU
VMs, so there's no WSL or cross-compiler to set up. Each VM boots Debian's
network installer and installs a minimal Debian stable into its RAM.

```bash
cargo run -p ferro-preview   # the desktop in a window, any OS
cargo xtask kernel-build     # build Linux (~7 min, once)
cargo xtask run              # boot FerroOS in QEMU with 64 MB
cargo xtask tor-build        # optional: build Tor (~7 min)
cargo xtask debian-build     # optional: the Debian layer, for glibc programs
cargo xtask iso              # build/ferroos.iso
```

The first `iso` makes a signing key in `build/secureboot`. Keep it private and
keep it: it's what your machine's firmware is told to trust, and later builds
reuse it so you approve it only once.

For VPN testing without a provider, `cargo xtask wg-test-server` spins up a
throwaway WireGuard server and drops its profile in `build/import/`, which shows
up as `D:` inside FerroOS. Delete it before building an ISO.

## How it's put together

| | |
|---|---|
| `crates/ferro` | the single binary; it runs whatever program it's started as |
| `ferro-init` | PID 1: mounts, loads only the drivers your hardware needs, supervises services |
| `ferro-shell` | the desktop, drawn in software straight into a DRM buffer |
| `ferro-gfx` | Win95 widgets, font and icons |
| `ferro-cmd` | the command prompt |
| `ferro-system` | the only thing running as root: vault, kill switches, Tor, the Debian layer, the installer |
| `ferro-vault` | the encrypted storage format |
| `ferro-sandbox` | `ferro-run`, the app sandbox |
| `ferro-net` | DHCP, DNS over HTTPS, TLS/ECH, VPN and Tor routing |
| `ferro-meta` | metadata stripping for JPEG, PNG and WebP |
| `ferro-path`, `ferro-sys` | `C:\` path mapping, `/proc` readers |
| `ferro-boot` | a tiny UEFI boot loader; no longer what installed disks start (that's Debian's shim and a signed kernel image, see `tools/boot`) |
| `ferro-preview` | runs the desktop on your normal OS for quick iteration |
| `xtask`, `tools/` | build scripts, kernel config, the build VMs |

## Why it's so small

Most of the RAM savings came from a few boring decisions: one binary instead of
six (they each carried their own copy of the Rust standard library), no kernel
framebuffer console (a second copy of the screen nobody looks at), no kernel
symbol table, and loading only the drivers that match your hardware.

The kernel also likes to size some tables from how much RAM you have. On a
16 GB machine that's around 30 MB of hash tables for an OS that's barely
running anything, so FerroOS pins them on the kernel command line. Run
`MEM /DETAIL` to see the full breakdown.

## Roadmap

- Mute just the mic without killing the speakers
- A window system, so Debian's graphical programs have somewhere to draw
- Page flipping and a hardware cursor on real GPUs
- Steam/Proton in a container

## License

MIT or Apache-2.0, whichever you prefer. The ISO also ships the Linux kernel
(GPL-2.0), GRUB (GPL-3.0), Arti (MIT/Apache-2.0) and, with the Debian layer,
Debian's base packages, each under their own licenses.
