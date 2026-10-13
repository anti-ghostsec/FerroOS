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

## FerroOS Heavy

A second system from the same code, for when you want everything modern Linux runs
(games through Steam and Proton, NVIDIA graphics, CUDA for LLM training, full Debian package ecosystem)
while retaining uncompromising privacy and an exceptionally light footprint.

| Feature | FerroOS Lite | FerroOS Heavy |
|---|---|---|
| **Base System** | Custom Linux kernel + static Rust userspace | Hardened Debian stable minbase |
| **Idle RAM Usage** | ~29 MB (target < 50 MB) | **Under 100 MB** (previously < 400 MB) |
| **Display Server** | Linux DRM/KMS directly (software framebuffer) | Xorg with lean `ferro-wm` desktop (no Composite overhead) |
| **Out-of-the-Box Display** | bochs, virtio, standard KMS | Intel (Gen1–14), AMD Radeon, NVIDIA nouveau, UEFI GOP / fbdev & VESA |
| **Out-of-the-Box Network** | Wired + Intel/Realtek/MediaTek/Atheros Wi-Fi | Wired LAN + Wi-Fi (Intel, Realtek, MediaTek, Atheros, Broadcom) |
| **Disk Encryption** | Custom Argon2id + XChaCha20-Poly1305 vault | Full-disk LUKS2 (Argon2id PBKDF) |
| **Boot Chain** | Microsoft-signed shim + unified kernel image | Microsoft-signed shim + Debian signed GRUB & signed kernel |
| **Modern Drivers & Gaming** | Isolated Debian glibc container | On-demand native: Steam/Proton, Vulkan, NVIDIA, CUDA, LLM training |

```bash
cargo xtask heavy-iso        # build/ferroos-heavy.iso (~535 MB)
cargo xtask wm-test          # the desktop on a virtual screen, with screenshots
```

### 1. Privacy and Security by Default
- **Optional Full Disk Encryption (LUKS2 with Argon2id)**: `ferro-install` and the graphical installer window (`ferro-setup`) offer full-disk encryption with LUKS2 using Argon2id key derivation, enabled by default with a checkbox in the GUI. Users can uncheck the box if they prefer an unencrypted standard `ext4` installation (e.g. for testing or non-sensitive environments). When enabled, all system files and home directories are protected. Swap support is encrypted ephemerally with random `/dev/urandom` keys so power cuts wipe all residual data.
- **Duress Password & Anti-Coercion Self-Destruct**: Setup and vault creation support configuring an optional secondary Duress PIN. If entered under coercion at logon or system unlock, the system cryptographically nukes both the vault keyslots and LUKS2 encrypted partition headers with zeros, purges memory, and powers down, appearing to an adversary as a routine "wrong password" error while permanently destroying access keys.
- **Cold-Boot Memory Extraction Defense**: All kernel boot chains enforce `init_on_alloc=1` and `init_on_free=1`. Every page freed by processes or caches is immediately zeroed by the kernel memory allocator at runtime, eliminating residual plaintexts, keys, and credentials from physical RAM chips against cold-boot attacks.
- **Encrypted Suspend & Sleep (In-RAM Key Eviction & Session Locking)**:
  - Standard ACPI Suspend-to-RAM (S3) exposes plaintexts and master encryption keys in physical DRAM, allowing cold-boot, liquid-nitrogen, or PCIe DMA extraction attacks.
  - **FerroOS Lite**: Selecting `Standby` in the Shut Down dialog or entering `SLEEP` in the Command Prompt unmounts and wipes vault keys from active memory, flushes and compacts kernel page/inode caches (`/proc/sys/vm/drop_caches=3`, `/proc/sys/vm/compact_memory=1` zeroed via `init_on_free=1`), sets ACPI deep sleep (`/sys/power/mem_sleep=deep`), and enters suspend via `/sys/power/state`. Upon wake, PID 1 immediately restarts the logon screen, enforcing full passphrase re-authentication before vault keys can be re-derived.
  - **FerroOS Heavy**: Enforces deep sleep (`MemorySleepMode=deep` in `/etc/systemd/sleep.conf.d/60-ferro-sleep.conf`) and executes automated pre/post sleep hooks (`/lib/systemd/system-sleep/60-ferro-encrypted-sleep.sh`). On sleep or laptop lid close, disk buffers are synced, memory caches zeroed, and active dm-crypt LUKS2 mappings (`ferro-system`) are suspended with `--noflush` key eviction.
- **Tor with Pluggable Transports & Session/Persistent Modes**:
  - Tor-only routing is optional and disabled by default.
  - `TOR ON`: Activates Tor routing for the current session only; on reboot it safely reverts to disabled.
  - `TOR ON PERSIST`: Enables Tor and remembers the choice across reboots in encrypted configuration.
  - Native Pluggable Transports (obfs4, Snowflake, WebTunnel) integrated with Arti via static client binaries (`lyrebird`, `snowflake-client`, `webtunnel-client`). Manage bridges via `TOR BRIDGE ADD <line>`, `TOR BRIDGE LIST`, and `TOR BRIDGE CLEAR`.
- **Secure Boot**: Boots out of the box with Secure Boot enabled using Debian's Microsoft-signed shim and signed GRUB.
- **Kernel Lockdown & Hardening**: Boots with `lockdown=integrity`, `init_on_alloc=1`, `init_on_free=1` (complete memory zeroing on allocation and release), `page_alloc.shuffle=1` (freelist randomization), `slab_nomerge`, `randomize_kstack_offset=on`, and `vsyscall=none`.
- **Sysctl Lockdown**: Restricted `kptr_restrict=2`, `dmesg_restrict=1`, `yama.ptrace_scope=1`, `unprivileged_bpf_disabled=1`, core dumps disabled (`suid_dumpable=0`), maximum ASLR entropy (`mmap_rnd_bits=32`, `mmap_rnd_compat_bits=16`, `kernel.randomize_va_space=2`), and legacy TTY line discipline autoload blocked (`dev.tty.ldisc_autoload=0`).
- **Encrypted DNS with DNSSEC**: System-wide DNS-over-TLS (DoT) via Quad9 and Mullvad with certificate name checking and DNSSEC validation (`systemd-resolved`); ISP/local DNS is never queried.
- **Network Privacy**: Per-connection random MAC addresses for both Wi-Fi and Ethernet, MAC randomization during Wi-Fi scanning, disabled DHCP hostname broadcasting, kernel-enforced RFC 4941 temporary IPv6 addresses (`use_tempaddr=2`), and TCP SYN flood / TIME-WAIT protection (`tcp_syncookies=1`, `tcp_rfc1337=1`). Public Wi-Fi protection drops bogus gratuitous ARPs and unicast-in-multicast spoofing (`drop_gratuitous_arp=1`, `drop_unicast_in_l2_multicast=1`).
- **Default-Deny Firewall & Restrictive Umask**: `nftables` drops all incoming connections by default while allowing outbound connections and essential ICMP/DHCPv6. Files created by users default to private permissions (`umask 077`).

### 2. Under 100 MB Idle RAM
- **Pinned Kernel Hash Tables & Lean daemons**: Default Linux allocates 30–50+ MB of RAM on boot for dcache/icache tables. Heavy pins `dhash_entries=32768 ihash_entries=16384 thash_entries=4096 uhash_entries=512 udev.children_max=1 audit=0 cgroup_no_v1=all`, cutting kernel overhead immediately and eliminating audit buffer allocations.
- **No Transparent Hugepage Bloat**: Booting with `transparent_hugepage=never` prevents Linux from allocating 2 MB pages to tiny daemons, eliminating internal page table fragmentation across background services.
- **Watermark and Swap Tuning**: `vm.min_free_kbytes=16384` caps the kernel's reserved memory pool from locking dozens of megabytes, while `vm.vfs_cache_pressure=200` and `vm.swappiness=10` aggressively prioritize reclaiming unused directory/inode caches.
- **No Composite, RECORD, or XTEST Extension Overhead**: `ferro-wm` renders window frames directly in software. Disabling Xorg Composite (`Option "Composite" "Disable"`), RECORD, and XTEST saves 15–20+ MB of redundant off-screen backing pixmaps and event recording buffers.
- **Compressed Volatile Journal Buffer**: `systemd-journald` volatile memory is capped at 4 MB (`RuntimeMaxUse=4M`) with compression enabled (`Compress=yes`).
- **Lean NetworkManager**: Configured with internal C DHCP (`dhcp=internal`), file-only plugins (`plugins=keyfile`), `no-auto-default=*`, and disabled connectivity check polling.
- **Masked Idle Daemons**: Virtual consoles `getty@tty3` through `getty@tty6`, background APT timers (`apt-daily.timer`, `apt-daily-upgrade.timer`), and disk scrubbers (`e2scrub_all.timer`) are masked, preventing periodic CPU, RAM, and network spikes.
- **Minimalist Desktop & Authentic Windows 95 Start Menu**:
  - **Clean 2-Icon Desktop**: The desktop surface holds only **Files** (generic folder explorer to open and navigate anywhere on the filesystem) and **Recycle Bin** (trash management). Single click selects the icon with navy highlight, double click launches it.
  - **Authentic Windows 95 / Lite Start Menu**: Matches FerroOS Lite with a dark embossed vertical banner ("FerroOS"), generous 34px item heights with crisp retro icons (*Programs*, *Documents*, *Settings*, *Help*, *Run...*, *Log Off...*, *Restart...*, *Shut Down...*), and smooth cascading flyouts for *Programs* and *Settings*.
  - **Right-Click Context Menus**: Right-clicking empty space on the desktop or in the File Manager (`ferro-files`) offers *New Folder*, *New Document*, *Open in Terminal*, and *Task Manager* / *Refresh* with icons. Right-clicking a file offers *Open*, *Create Shortcut* (symlink), *Delete*, and *Properties*.
  - **Display Properties & Desktop Customization**: Native X11 `ferro-appearance` dialog accessible from `Start > Settings > Appearance...` or right-clicking the desktop. Includes a retro CRT monitor preview, six background colors (*Classic Teal* `#008080`, *Dark Slate* `#182026`, *Obsidian Black* `#000000`, *Retro Navy* `#000080`, *Plum* `#550055`, *Forest Green* `#005522`), and a *Dark Mode UI (Charcoal)* option for window frames and dialogs. Changes apply immediately via X11 atom events without requiring a desktop restart, and persist across reboots in `~/.config/ferro/appearance.conf`.
  - **Authentic Green Phosphor Command Prompt**: `ferro-cmd` launches in a customized terminal with phosphor green text and block cursor (`#33FF33` on `#000000` black background) matching FerroOS Lite's DOS terminal styling.
  - **Aero Snap & Window Management**: Dragging a window titlebar to the top screen edge maximizes it; dragging to the left or right edge tiles it to exactly 50% split screen. Dragging a maximized titlebar restores and moves the window.
  - **Windows Keyboard Shortcuts**: `Win` (Super) or `Ctrl+Esc` toggles the Start menu, `Win+E` opens File Manager, `Win+D` toggles Show Desktop (minimizes/restores all windows), `Alt+F4` closes the active window, and `Ctrl+Shift+Esc` launches Task Manager.
  - **Taskbar RAM Display**: The system tray displays real-time memory footprint beside the clock (`XX MB  HH:MM AM`, clicking it opens Task Manager), matching FerroOS Lite. Command Prompt's `MEM` dynamically checks footprint against the appropriate 100 MB (Heavy) or 50 MB (Lite) idle budget.

### 3. Modern Hardware Support & On-Demand Drivers
- **Out-of-the-Box LAN & Wi-Fi**:
  - Wi-Fi firmware included for Intel (`firmware-iwlwifi`), Realtek (`firmware-realtek`), MediaTek (`firmware-mediatek`), Qualcomm Atheros (`firmware-atheros`), and Broadcom (`firmware-brcm80211`).
  - Command Prompt commands: `WIFI`, `WIFI LIST`, `WIFI JOIN <ssid>`, `WIFI ON`, `WIFI OFF`, `BATTERY`, `BRIGHTNESS [0-100]`.
- **Out-of-the-Box Display & Audio**:
  - Intel Gen1–Gen14 integrated graphics (modesetting DDX + GuC/HuC microcode via `firmware-misc-nonfree`).
  - AMD Radeon and Ryzen APUs (native `amdgpu` DDX + `firmware-amd-graphics`).
  - NVIDIA GeForce, RTX, and Quadro (native `nouveau` DDX + in-kernel nouveau with GSP firmware via `firmware-misc-nonfree`).
  - Universal `fbdev` (UEFI GOP framebuffer) and `vesa` fallback drivers guarantee a visible desktop on any display.
  - Sound Open Firmware (`firmware-sof-signed`) included for modern Intel (Tiger/Alder/Raptor/Meteor Lake) and AMD laptop internal speakers and digital microphones.
  - Zero-daemon ALSA sound utilities (`alsa-utils`) included out of the box; on-demand PipeWire setup via `ferro-heavy audio`.
- **On-Demand Gaming & CUDA / LLM Training** (`ferro-heavy` / Start menu "Add Programs"):
  - `ferro-heavy steam`: Sets up 32-bit architecture, Vulkan loaders, and 32/64-bit Mesa/NVIDIA Vulkan drivers for Proton Windows gaming.
  - `ferro-heavy nvidia`: Builds DKMS kernel modules for proprietary NVIDIA drivers, configures `libcuda1`, and provides `ferro-heavy enroll-key` for Secure Boot MOK enrollment.
  - `ferro-heavy llm`: Sets up Python 3, venv, and build tools for PyTorch / LLM fine-tuning and inference (llama.cpp, Ollama).
  - `ferro-heavy cuda`: Full NVIDIA CUDA compiler toolkit (nvcc & headers).
  - `ferro-heavy drivers`: Mesa 3D acceleration and VA-API hardware video decode for Intel & AMD.
  - `ferro-heavy audio`: Unmutes ALSA audio devices and optionally configures PipeWire / PulseAudio emulation.
  - `ferro-heavy status`: Live report of RAM in use, Secure Boot, LUKS encryption, firewall, encrypted DNS, LAN/Wi-Fi connection, audio hardware, and active GPU/CUDA status.

The installer window is in the Start menu (`ferro-setup`: disk, passphrase, account), and an installed system starts at our own logon screen (`ferro-logon`). Ctrl+Shift+Esc opens Task Manager (`ferro-taskmgr`), Ctrl+Esc the Start menu, and Alt+F4 closes a window.

![FerroOS Heavy](docs/screenshots/heavy-desktop.png)

![Installing FerroOS Heavy](docs/screenshots/heavy-install.png)

![The logon screen](docs/screenshots/heavy-logon.png)

![Task Manager](docs/screenshots/heavy-taskmgr.png)

## License

MIT or Apache-2.0, whichever you prefer. The ISO also ships the Linux kernel
(GPL-2.0), GRUB (GPL-3.0), Arti (MIT/Apache-2.0) and, with the Debian layer,
Debian's base packages, each under their own licenses.
